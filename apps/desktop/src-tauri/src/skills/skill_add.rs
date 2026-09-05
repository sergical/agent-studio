// ============================================================================
// Skills Module - skill_add
// `add_skill`: the Add-skill sheet's submit action. Installs through one of
// three methods - `dotagents` (writes `~/.agents/agents.toml`/`.lock`,
// updates with `dotagents sync`), `skills.sh` (writes `.skill-lock.json`),
// or `copy` (a plain, unmanaged folder, GitHub via tarball or a local path
// copy) - then, for `dotagents`/`copy` (both of which only ever write the
// shared `~/.agents/skills` folder), symlinks the new skill into
// `~/.claude/skills` when Claude Code was selected and that directory is a
// real one rather than the whole-dir symlink to the shared root. The
// CLI-shelling and GitHub-fetching bits are behind small traits, reusing
// `skill_fork`'s `UpstreamFetch`/`CommitLookup`, so the method dispatch is
// testable with fakes.
// ============================================================================

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::Manager;

use super::agents::AgentId;
use super::github_skill_listing::GithubSkillEntry;
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_deployment::{deployment_id, universal_skills_dir, SkillDestination};
use super::skill_dto::{
    AddSkillOutcome, AddSkillRequest, AddSkillResult, AddSkillsRequest, InstallScope,
    ParsedSkillSource, ParsedSkillSourceKind,
};
use super::skill_fork::{ForkMutationLock, RealUpstreamFetch, RepoSnapshot, UpstreamFetch};
use super::skill_fork_registry::{AddMethod, CopyDeploymentRecord, TrialScope};
use super::skill_fs::copy_dir_all;
use super::skill_harness_disable::set_new_universal_reader_enabled;
use super::skill_install_plan::{
    allowed_method, per_harness_copy_targets, skills_sh_universal_add_args, SkillInstallSpec,
};
use super::skill_refresh::{self, SkillRefreshState};
use super::skill_trial;
use super::skill_update_check::{self, CommitLookup, GhCommitLookup};

// ============================================================================
// Traits - the real implementation shells out; tests use a fake.
// ============================================================================

/// Runs an external CLI (`npx ...`), optionally in `cwd`. The real
/// implementation always runs `npx`, since both `dotagents` and `skills.sh`
/// are invoked through it.
pub trait CommandRunner {
    fn run_npx(&self, args: &[String], cwd: Option<&Path>) -> Result<(), String>;
}

pub struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn run_npx(&self, args: &[String], cwd: Option<&Path>) -> Result<(), String> {
        let mut cmd = Command::new("npx");
        cmd.args(args);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let output = cmd
            .output()
            .map_err(|e| format!("Failed to execute npx: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            Err(if stderr.is_empty() { stdout } else { stderr })
        }
    }
}

/// A `CommitLookup`/`UpstreamFetch` pair that always fails - used when `gh`
/// isn't resolvable, so a "Copy" of a GitHub source without a pinned `ref`
/// fails with a clear message instead of a confusing one further down.
struct Unavailable(String);

impl CommitLookup for Unavailable {
    fn latest_commit(
        &self,
        _repo: &str,
        _path: &str,
        _until: Option<&str>,
    ) -> Result<Option<(String, String)>, String> {
        Err(self.0.clone())
    }
}

impl UpstreamFetch for Unavailable {
    fn fetch_skill_dir(
        &self,
        _repo: &str,
        _path: &str,
        _commit: &str,
        _into: &Path,
    ) -> Result<(), String> {
        Err(self.0.clone())
    }
}

// ============================================================================
// Shared-folder / Claude Code symlink rule
// ============================================================================

/// `~/.claude/skills/<name>` -> `../../.agents/skills/<name>`, relative -
/// only when Claude Code is one of `agents`, `shared_skills_dir`'s sibling
/// `claude_skills_dir` exists as a *real* directory (not the whole-dir
/// symlink some setups use instead), and no entry named `name` is already
/// there. Returns the created symlink's path, or `None` when nothing was
/// created (Claude Code not selected, or the whole-dir symlink already
/// covers it). Used for `dotagents` and "Copy", which only ever write the
/// shared folder directly - `skills.sh`'s own `--agent claude-code` deploys
/// straight into `claude_skills_dir` itself, so it never needs this.
pub(crate) fn maybe_claude_code_symlink(
    claude_skills_dir: &Path,
    shared_skills_dir: &Path,
    name: &str,
    agents: &[AgentId],
) -> Result<Option<String>, String> {
    if !agents.contains(&AgentId::ClaudeCode) {
        return Ok(None);
    }
    if let Ok(meta) = fs::symlink_metadata(claude_skills_dir) {
        if meta.file_type().is_symlink() {
            // The whole-dir symlink to the shared root already covers it.
            return Ok(None);
        }
    } else {
        fs::create_dir_all(claude_skills_dir)
            .map_err(|e| format!("Failed to create {}: {e}", claude_skills_dir.display()))?;
    }

    let link_path = claude_skills_dir.join(name);
    if fs::symlink_metadata(&link_path).is_ok() {
        return Ok(None);
    }
    let target = relative_path_between(claude_skills_dir, shared_skills_dir).join(name);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link_path)
        .map_err(|e| format!("Failed to symlink {}: {e}", link_path.display()))?;
    #[cfg(not(unix))]
    return Err("Symlinking is only supported on Unix".to_string());
    Ok(Some(link_path.to_string_lossy().to_string()))
}

/// A relative path that leads from inside `from` to `to`: one `..` per
/// component of `from` below the common prefix, then the rest of `to`.
/// For `~/.claude/skills` and `~/.agents/skills` that is
/// `../../.agents/skills`.
fn relative_path_between(from: &Path, to: &Path) -> PathBuf {
    let from_parts: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut rel = PathBuf::new();
    for _ in common..from_parts.len() {
        rel.push("..");
    }
    for part in &to_parts[common..] {
        rel.push(part);
    }
    rel
}

// ============================================================================
// IPC-boundary validation - `request.source` comes straight off the wire,
// so every field that ends up in a filesystem path is checked here before
// any method dispatch runs.
// ============================================================================

/// `value` must be a relative path with no `..` component - used for
/// `source.path` (the repo subpath), which is later joined onto a fetch
/// destination and must not be able to walk it outside the target folder.
fn validate_relative_no_traversal(value: &str, field: &str) -> Result<(), String> {
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(format!("{field} must be a relative path"));
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("{field} must not contain `..`"));
    }
    Ok(())
}

/// Validates every `ParsedSkillSource` field that can influence a filesystem
/// path, before any method runs: `skill_name` (the final directory name)
/// must pass `validate_skill_dir_name`; `path` must be relative with no
/// `..`; a `local` source's `local_path` must canonicalize to an existing
/// directory.
fn validate_parsed_source(source: &ParsedSkillSource) -> Result<(), String> {
    if let Some(name) = &source.skill_name {
        validate_skill_dir_name(name)?;
    }
    if let Some(path) = &source.path {
        validate_relative_no_traversal(path, "path")?;
    }
    if source.kind == ParsedSkillSourceKind::Local {
        let local_path = source
            .local_path
            .as_deref()
            .ok_or("A local source needs a path")?;
        let canonical = fs::canonicalize(local_path)
            .map_err(|e| format!("Could not resolve {local_path}: {e}"))?;
        if !canonical.is_dir() {
            return Err(format!("{local_path} is not a directory"));
        }
    }
    Ok(())
}

// ============================================================================
// Method dispatch
// ============================================================================

/// Every entry name directly under `dir`, or an empty set when `dir` doesn't
/// exist yet.
fn dir_entry_names(dir: &Path) -> std::collections::BTreeSet<String> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// The `<repo | git:url>` argument dotagents' `add` takes for `source`.
fn dotagents_source_arg(source: &ParsedSkillSource) -> Result<String, String> {
    match source.kind {
        ParsedSkillSourceKind::Github => {
            // dotagents only accepts `owner/repo`; the skill inside it is
            // selected with `--name`, never as a path suffix (which it rejects).
            source
                .repo
                .clone()
                .ok_or("A GitHub source needs a repo".to_string())
        }
        ParsedSkillSourceKind::Git => {
            let url = source.url.clone().ok_or("A git source needs a url")?;
            Ok(format!("git:{url}"))
        }
        ParsedSkillSourceKind::Local => {
            Err("Local sources can only be added with the Copy method".to_string())
        }
    }
}

/// Records a 24 h trial for each `(name, skill_dir, claude_link)` in
/// `installs`, when `request.trial` is set - one record per skill, keyed by
/// `request.scope`, never a single comma-joined name (a multi-skill
/// `dotagents add` produces one `installs` entry per newly created folder).
/// Returns a warning message when any recording failed; the install itself
/// already succeeded, so this is reported to the caller, not turned into an
/// error that would make a successful install look failed.
fn maybe_record_trials(
    home: &Path,
    request: &AddSkillRequest,
    installs: &[(String, PathBuf, Option<PathBuf>, String)],
) -> Option<String> {
    if !request.trial {
        return None;
    }
    let scope = match request.scope {
        InstallScope::Global => TrialScope::Global,
        InstallScope::Project => TrialScope::Project,
    };
    let now = chrono::Utc::now();
    let mut failures = Vec::new();
    for (name, skill_dir, claude_link, deployment_id) in installs {
        if let Err(e) = skill_trial::record_trial(
            home,
            deployment_id,
            scope,
            request.project_path.as_deref(),
            request.method,
            skill_dir.clone(),
            claude_link.clone(),
            now,
        ) {
            failures.push(format!("{name}: {e}"));
        }
    }
    if failures.is_empty() {
        None
    } else {
        Some(format!(
            "Installed, but the 24 h trial could not be recorded: {}. Remove the skill by hand when you are done.",
            failures.join("; ")
        ))
    }
}

fn installed_deployment_id(
    request: &AddSkillRequest,
    name: &str,
    path: &Path,
    slot: &str,
) -> String {
    let scope = match request.scope {
        InstallScope::Global => "global",
        InstallScope::Project => "project",
    };
    deployment_id(
        name,
        scope,
        request.destination,
        slot,
        request.project_path.as_deref(),
        path,
    )
}

fn add_via_dotagents(
    home: &Path,
    request: &AddSkillRequest,
    runner: &dyn CommandRunner,
) -> Result<AddSkillResult, String> {
    let source_arg = dotagents_source_arg(&request.source)?;
    let is_project = request.scope == InstallScope::Project;
    let project_path = request.project_path.as_deref();
    if is_project && project_path.is_none() {
        return Err("Project scope needs a project path".to_string());
    }

    let mut args = vec!["-y".to_string(), "@sentry/dotagents".to_string()];
    if is_project {
        args.push("--project".to_string());
    }
    args.push("add".to_string());
    args.push(source_arg);
    if let Some(name) = &request.source.skill_name {
        args.push("--name".to_string());
        args.push(name.clone());
    }
    if let Some(r#ref) = &request.source.git_ref {
        args.push("--ref".to_string());
        args.push(r#ref.clone());
    }

    let cwd = if is_project {
        project_path.map(PathBuf::from)
    } else {
        None
    };
    let shared_dir = shared_skills_dir(home, request);
    let claude_dir = claude_skills_dir(home, request);

    let before = dir_entry_names(&shared_dir);
    runner.run_npx(&args, cwd.as_deref())?;
    let after = dir_entry_names(&shared_dir);
    let mut new_names: Vec<String> = after.difference(&before).cloned().collect();
    new_names.sort();
    if new_names.is_empty() {
        if let Some(name) = &request.source.skill_name {
            new_names.push(name.clone());
        }
    }
    if new_names.is_empty() {
        return Err("dotagents did not create any new skill directories".to_string());
    }

    let mut deployments_created: Vec<String> = new_names
        .iter()
        .map(|n| shared_dir.join(n).to_string_lossy().to_string())
        .collect();
    let mut installs = Vec::new();
    for name in &new_names {
        let claude_link =
            maybe_claude_code_symlink(&claude_dir, &shared_dir, name, &request.agents)?
                .map(PathBuf::from);
        if let Some(link) = &claude_link {
            deployments_created.push(link.to_string_lossy().to_string());
        }
        let skill_dir = shared_dir.join(name);
        installs.push((
            name.clone(),
            skill_dir.clone(),
            claude_link,
            installed_deployment_id(request, name, &skill_dir, "universal"),
        ));
    }
    let warning = maybe_record_trials(home, request, &installs);

    Ok(AddSkillResult {
        name: new_names.join(", "),
        tool: "dotagents".to_string(),
        command: format!("npx {}", args.join(" ")),
        deployments_created,
        warning,
    })
}

fn add_via_skills_sh(
    home: &Path,
    request: &AddSkillRequest,
    runner: &dyn CommandRunner,
) -> Result<AddSkillResult, String> {
    let repo = request
        .source
        .repo
        .clone()
        .ok_or("The skills.sh method needs a GitHub source")?;
    let skill_name = request.source.skill_name.clone();
    let spec = SkillInstallSpec {
        scope: request.scope.clone(),
        destination: request.destination,
        project_path: request.project_path.clone(),
        harnesses: request.agents.clone(),
    };
    let args = skills_sh_universal_add_args(&repo, skill_name.as_deref(), &spec)?;

    runner.run_npx(&args, None)?;

    let result_name =
        skill_name.unwrap_or_else(|| repo.split('/').next_back().unwrap_or(&repo).to_string());

    let project = request.project_path.as_deref().map(Path::new);
    let mut deployment_dirs = vec![universal_skills_dir(home, request.scope.clone(), project)];
    if request.agents.contains(&AgentId::ClaudeCode) {
        deployment_dirs.push(claude_skills_dir(home, request));
    }
    let deployments_created = deployment_dirs
        .iter()
        .map(|dir| dir.join(&result_name).to_string_lossy().to_string())
        .collect();

    // Universal is the canonical trial directory. skills.sh may also create
    // Claude Code's selected link or copy, but trial expiry remains owned by
    // the Universal deployment.
    let claude_link = deployment_dirs
        .get(1)
        .map(|dir| dir.join(&result_name))
        .filter(|path| {
            fs::symlink_metadata(path)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
        });
    let warning = deployment_dirs.first().and_then(|dir| {
        maybe_record_trials(
            home,
            request,
            &[{
                let skill_dir = dir.join(&result_name);
                (
                    result_name.clone(),
                    skill_dir.clone(),
                    claude_link,
                    installed_deployment_id(request, &result_name, &skill_dir, "universal"),
                )
            }],
        )
    });

    Ok(AddSkillResult {
        name: result_name,
        tool: "skills-sh".to_string(),
        command: format!("npx {}", args.join(" ")),
        deployments_created,
        warning,
    })
}

fn derive_copy_name(source: &ParsedSkillSource) -> Result<String, String> {
    if let Some(name) = &source.skill_name {
        return Ok(name.clone());
    }
    match source.kind {
        ParsedSkillSourceKind::Github => source
            .path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .map(|s| s.to_string())
            .or_else(|| {
                source
                    .repo
                    .as_deref()
                    .and_then(|r| r.rsplit('/').next())
                    .map(|s| s.to_string())
            })
            .ok_or_else(|| "Could not determine a skill name".to_string()),
        ParsedSkillSourceKind::Local => source
            .local_path
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Could not determine a skill name".to_string()),
        ParsedSkillSourceKind::Git => Err("Copy is not supported for git sources".to_string()),
    }
}

fn remove_install_paths(paths: &[PathBuf]) {
    for path in paths {
        if fs::symlink_metadata(path).is_ok() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn lexical_absolute_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("Could not resolve current directory: {error}"))?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

fn resolve_existing_path_prefix(path: &Path) -> Result<PathBuf, String> {
    let mut existing = lexical_absolute_path(path)?;
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(&existing) {
            Ok(mut canonical) => {
                for part in missing.iter().rev() {
                    canonical.push(part);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let part = existing.file_name().ok_or_else(|| {
                    format!("Could not resolve Copy destination {}", path.display())
                })?;
                missing.push(part.to_os_string());
                existing.pop();
            }
            Err(error) => {
                return Err(format!(
                    "Could not resolve Copy destination {}: {error}",
                    path.display()
                ));
            }
        }
    }
}

fn path_is_within(path: &Path, directory: &Path) -> bool {
    path == directory || path.starts_with(directory)
}

/// Reject local Copy destinations that resolve within the source before creating paths.
fn validate_local_copy_destinations(
    source: &Path,
    targets: &[PathBuf],
    staging: &[PathBuf],
) -> Result<(), String> {
    let source_lexical = lexical_absolute_path(source)?;
    let source_canonical = fs::canonicalize(source)
        .map_err(|error| format!("Could not resolve {}: {error}", source.display()))?;
    for candidate in targets
        .iter()
        .filter_map(|target| target.parent())
        .chain(targets.iter().map(PathBuf::as_path))
        .chain(staging.iter().map(PathBuf::as_path))
    {
        let candidate_lexical = lexical_absolute_path(candidate)?;
        let candidate_canonical = resolve_existing_path_prefix(candidate)?;
        if path_is_within(&candidate_lexical, &source_lexical)
            || path_is_within(&candidate_lexical, &source_canonical)
            || path_is_within(&candidate_canonical, &source_canonical)
        {
            return Err(format!(
                "Local Copy destination must be outside the source directory: {}",
                candidate.display()
            ));
        }
    }
    Ok(())
}

/// Stage every Copy target beside its destination, then rename each complete
/// directory into place. A failure removes all staging and committed targets.
fn commit_copy_install<F, C>(
    targets: &[PathBuf],
    local_source: Option<&Path>,
    acquire: F,
    copy: C,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
    C: Fn(&Path, &Path) -> Result<(), String>,
{
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging: Vec<PathBuf> = targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            target
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(format!(
                    ".skill-studio-install-{}-{nonce}-{index}",
                    std::process::id()
                ))
        })
        .collect();

    if let Some(source) = local_source {
        validate_local_copy_destinations(source, targets, &staging)?;
    }

    for target in targets {
        let parent = target
            .parent()
            .ok_or_else(|| format!("Copy destination has no parent: {}", target.display()))?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
        if fs::symlink_metadata(target).is_ok() {
            remove_install_paths(&staging);
            return Err(format!(
                "Copy destination already exists: {}",
                target.display()
            ));
        }
    }

    if let Err(error) = acquire(&staging[0]) {
        remove_install_paths(&staging);
        return Err(error);
    }
    for stage in staging.iter().skip(1) {
        if let Err(error) = copy(&staging[0], stage) {
            remove_install_paths(&staging);
            return Err(error);
        }
    }

    let mut committed = Vec::new();
    for (stage, target) in staging.iter().zip(targets) {
        if fs::symlink_metadata(target).is_ok() {
            remove_install_paths(&staging);
            remove_install_paths(&committed);
            return Err(format!(
                "Copy destination already exists: {}",
                target.display()
            ));
        }
        if let Err(error) = fs::rename(stage, target) {
            remove_install_paths(&staging);
            remove_install_paths(&committed);
            return Err(format!(
                "Failed to commit Copy destination {}: {error}",
                target.display()
            ));
        }
        committed.push(target.clone());
    }
    Ok(())
}

fn add_via_copy(
    home: &Path,
    request: &AddSkillRequest,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
    snapshot: Option<&dyn RepoSnapshot>,
) -> Result<AddSkillResult, String> {
    // Read before creating files. A malformed registry must fail closed, not
    // let an install succeed without the ownership record needed to remove it.
    let mut registry = super::skill_fork_registry::read_fork_registry(home)?;
    if request.scope == InstallScope::Project {
        let project_path = request
            .project_path
            .as_deref()
            .ok_or("Project scope needs a project path")?;
        let canonical = fs::canonicalize(project_path)
            .map_err(|e| format!("Could not resolve project path {project_path}: {e}"))?;
        if !canonical.is_dir() {
            return Err(format!("{project_path} is not a directory"));
        }
    }

    let name = derive_copy_name(&request.source)?;
    validate_skill_dir_name(&name)?;
    let project = request.project_path.as_deref().map(Path::new);
    let per_harness_agents = if request.destination == SkillDestination::PerHarness {
        per_harness_copy_targets(&SkillInstallSpec {
            scope: request.scope.clone(),
            destination: request.destination,
            project_path: request.project_path.clone(),
            harnesses: request.agents.clone(),
        })?
    } else {
        Vec::new()
    };
    let target_roots = match request.destination {
        SkillDestination::Universal => {
            vec![universal_skills_dir(home, request.scope.clone(), project)]
        }
        SkillDestination::PerHarness => per_harness_agents
            .iter()
            .map(|agent| match request.scope {
                InstallScope::Global => agent.global_skills_dir(home),
                InstallScope::Project => {
                    agent.project_skills_dir(project.unwrap_or_else(|| Path::new("")))
                }
            })
            .collect(),
    };
    let targets: Vec<PathBuf> = target_roots.iter().map(|root| root.join(&name)).collect();
    let target = targets[0].clone();
    let local_copy_source = if request.source.kind == ParsedSkillSourceKind::Local {
        let path = request
            .source
            .local_path
            .as_deref()
            .ok_or("A local source needs a path")?;
        Some(fs::canonicalize(path).map_err(|error| format!("Could not resolve {path}: {error}"))?)
    } else {
        None
    };

    commit_copy_install(
        &targets,
        local_copy_source.as_deref(),
        |staging_target| match request.source.kind {
            ParsedSkillSourceKind::Github => {
                let repo = request
                    .source
                    .repo
                    .clone()
                    .ok_or("A GitHub source needs a repo")?;
                let path = request.source.path.clone().unwrap_or_default();
                // A batch install passes the snapshot it already downloaded, so
                // the tarball is fetched once for the whole picker selection.
                match snapshot {
                    Some(snapshot) => snapshot.copy_dir(&path, staging_target)?,
                    None => {
                        let commit = match &request.source.git_ref {
                            Some(r) => r.clone(),
                            None => lookup
                                .latest_commit(&repo, &path, None)?
                                .map(|(sha, _)| sha)
                                .ok_or_else(|| {
                                    format!("Could not determine {name}'s latest commit")
                                })?,
                        };
                        fetch.fetch_skill_dir(&repo, &path, &commit, staging_target)?;
                    }
                }
                Ok(())
            }
            ParsedSkillSourceKind::Local => {
                let local_path = local_copy_source
                    .as_deref()
                    .ok_or("A local source needs a path")?;
                copy_dir_all(local_path, staging_target)
            }
            ParsedSkillSourceKind::Git => {
                Err("Copy is not supported for git sources; use dotagents".to_string())
            }
        },
        copy_dir_all,
    )?;
    let mut deployments_created: Vec<String> = targets
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    let claude_link = if request.destination == SkillDestination::Universal {
        match maybe_claude_code_symlink(
            &claude_skills_dir(home, request),
            &target_roots[0],
            &name,
            &request.agents,
        ) {
            Ok(link) => link.map(PathBuf::from),
            Err(error) => {
                for created in &targets {
                    let _ = fs::remove_dir_all(created);
                }
                return Err(error);
            }
        }
    } else {
        None
    };
    if let Some(link) = &claude_link {
        deployments_created.push(link.to_string_lossy().to_string());
    }

    let ownership_records = if request.destination == SkillDestination::Universal {
        let mut records = vec![copy_deployment_record(request, &name, &target, "universal")];
        if let Some(link) = &claude_link {
            records.push(copy_deployment_record(
                request,
                &name,
                link,
                AgentId::ClaudeCode.cli_name(),
            ));
        }
        records.into_iter().collect::<Result<Vec<_>, _>>()
    } else {
        per_harness_agents
            .iter()
            .zip(&targets)
            .map(|(agent, path)| copy_deployment_record(request, &name, path, agent.cli_name()))
            .collect::<Result<Vec<_>, _>>()
    };
    let ownership_records = match ownership_records {
        Ok(records) => records,
        Err(error) => {
            if let Some(link) = &claude_link {
                let _ = fs::remove_file(link);
            }
            for created in &targets {
                let _ = fs::remove_dir_all(created);
            }
            return Err(format!(
                "Failed to fingerprint Copy ownership; rolled back the install: {error}"
            ));
        }
    };
    for record in &ownership_records {
        registry
            .copies
            .insert(record.deployment_id.clone(), record.clone());
    }
    registry.version = super::skill_fork_registry::CURRENT_REGISTRY_VERSION;
    if let Err(error) = super::skill_fork_registry::write_fork_registry(home, &registry) {
        if let Some(link) = &claude_link {
            let _ = fs::remove_file(link);
        }
        for created in &targets {
            let _ = fs::remove_dir_all(created);
        }
        return Err(format!(
            "Failed to record Copy ownership; rolled back the install: {error}"
        ));
    }
    let trial_installs: Vec<_> = if request.destination == SkillDestination::Universal {
        vec![(
            name.clone(),
            target.clone(),
            claude_link,
            installed_deployment_id(request, &name, &target, "universal"),
        )]
    } else {
        per_harness_agents
            .iter()
            .zip(targets.iter())
            .map(|(agent, path)| {
                (
                    name.clone(),
                    path.clone(),
                    None,
                    installed_deployment_id(request, &name, path, agent.cli_name()),
                )
            })
            .collect()
    };
    let warning = maybe_record_trials(home, request, &trial_installs);

    Ok(AddSkillResult {
        name,
        tool: "copy".to_string(),
        command: format!("copy -> {}", target.display()),
        deployments_created,
        warning,
    })
}

fn copy_deployment_record(
    request: &AddSkillRequest,
    name: &str,
    path: &Path,
    slot: &str,
) -> Result<CopyDeploymentRecord, String> {
    Ok(CopyDeploymentRecord {
        deployment_id: installed_deployment_id(request, name, path, slot),
        name: name.to_string(),
        path: path.to_path_buf(),
        scope: request.scope.clone(),
        destination: request.destination,
        slot: slot.to_string(),
        project_path: request.project_path.clone(),
        content_hash: super::skill_discovery::live_skill_content_hash(path)?,
        disabled: false,
    })
}

/// The shared skills folder `add_via_dotagents` writes into - the home root
/// for global scope, `<project>/.agents/skills` for project scope.
fn shared_skills_dir(home: &Path, request: &AddSkillRequest) -> PathBuf {
    if request.scope == InstallScope::Global {
        home.join(".agents").join("skills")
    } else {
        Path::new(request.project_path.as_deref().unwrap_or(""))
            .join(".agents")
            .join("skills")
    }
}

/// The Claude Code skills directory the symlink rule targets - the home root
/// for global scope, `<project>/.claude/skills` for project scope.
fn claude_skills_dir(home: &Path, request: &AddSkillRequest) -> PathBuf {
    if request.scope == InstallScope::Global {
        home.join(".claude").join("skills")
    } else {
        Path::new(request.project_path.as_deref().unwrap_or(""))
            .join(".claude")
            .join("skills")
    }
}

/// The `SKILL.md` paths Codex can see among the deployments an install just
/// created: its own skills directory, plus the shared folder it reads
/// natively. Codex's per-skill switch keys off the path, not the name, so
/// anything else that was created is irrelevant to it.
fn codex_visible_skill_mds(
    home: &Path,
    request: &AddSkillRequest,
    deployments: &[String],
) -> Vec<PathBuf> {
    let project = PathBuf::from(request.project_path.as_deref().unwrap_or(""));
    let roots = [
        shared_skills_dir(home, request),
        if request.scope == InstallScope::Global {
            AgentId::Codex.global_skills_dir(home)
        } else {
            AgentId::Codex.project_skills_dir(&project)
        },
    ];
    deployments
        .iter()
        .map(PathBuf::from)
        .filter(|path| {
            path.parent()
                .is_some_and(|parent| roots.contains(&parent.to_path_buf()))
        })
        .map(|path| path.join("SKILL.md"))
        .collect()
}

/// Switches the freshly installed skill off for every harness in
/// `request.disabled_harnesses` - the readers the install itself couldn't
/// avoid reaching, since every method writes the shared folder they all
/// read. A failure folds into `result.warning`: the skill is on disk and
/// usable, so it must not be reported as a failed install.
fn apply_disabled_harnesses(home: &Path, request: &AddSkillRequest, result: &mut AddSkillResult) {
    if request.disabled_harnesses.is_empty() {
        return;
    }
    let codex_paths = codex_visible_skill_mds(home, request, &result.deployments_created);
    let universal_root = shared_skills_dir(home, request);
    let project_path = request.project_path.as_deref().map(Path::new);
    let claude_root = super::skill_lifecycle::claude_skills_dir_for_scope(
        home,
        request.scope.clone(),
        project_path,
    );
    let mut failures = Vec::new();
    // One `dotagents add` can create several folders, joined into `name`.
    for name in result.name.split(", ") {
        for agent in &request.disabled_harnesses {
            let universal_path = universal_root.join(name);
            let deployment_id =
                installed_deployment_id(request, name, &universal_path, "universal");
            let target = super::skill_dto::HarnessVisibilityTarget {
                deployment_id,
                reader_agent: *agent,
            };
            if let Err(e) = set_new_universal_reader_enabled(
                home,
                name,
                &target,
                false,
                &universal_path,
                &claude_root.join(name),
                &codex_paths,
            ) {
                failures.push(format!(
                    "Installed, but could not turn it off for {}: {e}",
                    agent.display_name()
                ));
            }
        }
    }
    if failures.is_empty() {
        return;
    }
    let appended = failures.join(" ");
    result.warning = Some(match result.warning.take() {
        Some(existing) => format!("{existing} {appended}"),
        None => appended,
    });
}

/// `add_skill`'s logic, taking `home`/traits directly so it's testable
/// without a Tauri `AppHandle` or a network call.
pub fn add_skill_with(
    home: &Path,
    request: &AddSkillRequest,
    runner: &dyn CommandRunner,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
) -> Result<AddSkillResult, String> {
    if request.destination == SkillDestination::PerHarness && request.trial {
        return Err("Trials require the Universal destination".to_string());
    }
    validate_parsed_source(&request.source)?;
    allowed_method(request.destination, request.method)?;
    if request.destination == SkillDestination::PerHarness && !request.disabled_harnesses.is_empty()
    {
        return Err("Per harness installs cannot include disabled harnesses".to_string());
    }
    let mut result = match request.method {
        AddMethod::Dotagents => {
            if request.destination != SkillDestination::Universal {
                return Err("dotagents installs require the Universal destination".to_string());
            }
            add_via_dotagents(home, request, runner)
        }
        AddMethod::SkillsSh => add_via_skills_sh(home, request, runner),
        AddMethod::Copy => add_via_copy(home, request, fetch, lookup, None),
    }?;
    apply_disabled_harnesses(home, request, &mut result);
    Ok(result)
}

// ============================================================================
// Batch install - one picker selection, many skills
// ============================================================================

/// The single-skill request `entry` implies, with everything else copied
/// from the batch. An entry at the repo root has an empty `path`, which
/// stays `None` so the copy method treats it as "the whole repo".
fn request_for_entry(batch: &AddSkillsRequest, entry: &GithubSkillEntry) -> AddSkillRequest {
    let mut source = batch.source.clone();
    source.path = Some(entry.path.clone()).filter(|p| !p.is_empty());
    source.skill_name = Some(entry.name.clone());
    AddSkillRequest {
        source,
        method: batch.method,
        destination: batch.destination,
        agents: batch.agents.clone(),
        disabled_harnesses: batch.disabled_harnesses.clone(),
        scope: batch.scope.clone(),
        project_path: batch.project_path.clone(),
        trial: batch.trial,
    }
}

/// `add_skills`' logic, taking `home`/traits directly so it's testable
/// without a Tauri `AppHandle` or a network call. One skill's failure never
/// stops the rest: each entry gets its own `AddSkillOutcome`. The Copy
/// method downloads the repo once up front and extracts every selected
/// folder out of that one snapshot.
pub fn add_skills_with(
    home: &Path,
    request: &AddSkillsRequest,
    runner: &dyn CommandRunner,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
) -> Result<Vec<AddSkillOutcome>, String> {
    if request.destination == SkillDestination::PerHarness && request.trial {
        return Err("Trials require the Universal destination".to_string());
    }
    if request.skills.is_empty() {
        return Err("Select at least one skill".to_string());
    }
    allowed_method(request.destination, request.method)?;
    if request.method == AddMethod::Dotagents && request.destination != SkillDestination::Universal
    {
        return Err("dotagents installs require the Universal destination".to_string());
    }
    if request.destination == SkillDestination::PerHarness && !request.disabled_harnesses.is_empty()
    {
        return Err("Per harness installs cannot include disabled harnesses".to_string());
    }

    let snapshot = if request.method == AddMethod::Copy
        && request.source.kind == ParsedSkillSourceKind::Github
    {
        open_repo_snapshot(request, fetch, lookup)?
    } else {
        None
    };

    let mut outcomes = Vec::with_capacity(request.skills.len());
    for entry in &request.skills {
        let single = request_for_entry(request, entry);
        let mut result = match validate_parsed_source(&single.source) {
            Ok(()) => match request.method {
                AddMethod::Dotagents => add_via_dotagents(home, &single, runner),
                AddMethod::SkillsSh => add_via_skills_sh(home, &single, runner),
                AddMethod::Copy => add_via_copy(home, &single, fetch, lookup, snapshot.as_deref()),
            },
            Err(e) => Err(e),
        };
        if let Ok(result) = &mut result {
            apply_disabled_harnesses(home, &single, result);
        }
        outcomes.push(match result {
            Ok(result) => AddSkillOutcome {
                name: entry.name.clone(),
                result: Some(result),
                error: None,
            },
            Err(error) => AddSkillOutcome {
                name: entry.name.clone(),
                result: None,
                error: Some(error),
            },
        });
    }
    Ok(outcomes)
}

/// Resolves the commit the batch pins to and downloads the repo once.
/// `None` means the fetch implementation has no bulk mode, so each skill
/// falls back to its own `fetch_skill_dir`.
fn open_repo_snapshot(
    request: &AddSkillsRequest,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
) -> Result<Option<Box<dyn RepoSnapshot>>, String> {
    let repo = request
        .source
        .repo
        .clone()
        .ok_or("A GitHub source needs a repo")?;
    let path = request.source.path.clone().unwrap_or_default();
    let commit = match &request.source.git_ref {
        Some(r) => r.clone(),
        None => lookup
            .latest_commit(&repo, &path, None)?
            .map(|(sha, _)| sha)
            .ok_or_else(|| format!("Could not determine {repo}'s latest commit"))?,
    };
    fetch.open_repo(&repo, &commit)
}

/// The `UpstreamFetch`/`CommitLookup` pair both add commands run with.
type GithubTools = (Box<dyn UpstreamFetch>, Box<dyn CommitLookup>);

/// The GitHub-facing pair both add commands run with: the real `gh`-backed
/// implementations, or ones that fail with "Run Check now first" when `gh`
/// isn't resolvable.
fn resolve_fetch_and_lookup(app: &tauri::AppHandle) -> Result<GithubTools, String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not resolve app data dir: {e}"))?;
    Ok(match skill_update_check::resolve_gh_binary() {
        Some(gh_bin) => (
            Box::new(RealUpstreamFetch {
                gh_bin: gh_bin.clone(),
                cache_dir: app_data.join("skill-studio").join("cache"),
            }),
            Box::new(GhCommitLookup { gh_bin }),
        ),
        None => {
            let message = "Run Check now first".to_string();
            (
                Box::new(Unavailable(message.clone())),
                Box::new(Unavailable(message)),
            )
        }
    })
}

/// Installs every skill picked out of one source - see `add_skills_with`.
/// The whole batch fails only when nothing could be attempted; a single
/// skill's failure comes back in its own `AddSkillOutcome`.
#[tauri::command]
pub fn add_skills(
    request: AddSkillsRequest,
    app: tauri::AppHandle,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<Vec<AddSkillOutcome>, String> {
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let runner = RealCommandRunner;
    let (fetch, lookup) = resolve_fetch_and_lookup(&app)?;
    let result = add_skills_with(&home, &request, &runner, fetch.as_ref(), lookup.as_ref());
    skill_refresh::request_snapshot_rebuild(&app);
    result
}

#[tauri::command]
pub fn add_skill(
    request: AddSkillRequest,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<AddSkillResult, String> {
    let _guard = fork_lock.try_acquire()?;
    let _ = &refresh_state;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let runner = RealCommandRunner;
    let (fetch, lookup) = resolve_fetch_and_lookup(&app)?;

    // Trial recording happens inside each `add_via_*` method (it needs the
    // exact per-skill directories only they know), and surfaces as
    // `AddSkillResult.warning` rather than an error - the install already
    // succeeded by the time it runs.
    let result = add_skill_with(&home, &request, &runner, fetch.as_ref(), lookup.as_ref());
    skill_refresh::request_snapshot_rebuild(&app);
    result
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<(Vec<String>, Option<PathBuf>)>>,
        /// Directories to create under the run's cwd (or absolute), simulating
        /// the CLI actually writing skill folders.
        creates: Vec<PathBuf>,
        fail: Option<String>,
    }

    impl CommandRunner for FakeRunner {
        fn run_npx(&self, args: &[String], cwd: Option<&Path>) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push((args.to_vec(), cwd.map(PathBuf::from)));
            if let Some(err) = &self.fail {
                return Err(err.clone());
            }
            for dir in &self.creates {
                fs::create_dir_all(dir).unwrap();
            }
            Ok(())
        }
    }

    struct NeverCalledFetch;
    impl UpstreamFetch for NeverCalledFetch {
        fn fetch_skill_dir(&self, _: &str, _: &str, _: &str, _: &Path) -> Result<(), String> {
            panic!("fetch should not have been called");
        }
    }
    struct NeverCalledLookup;
    impl CommitLookup for NeverCalledLookup {
        fn latest_commit(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> Result<Option<(String, String)>, String> {
            panic!("lookup should not have been called");
        }
    }

    fn github_source(
        repo: &str,
        path: Option<&str>,
        skill_name: Option<&str>,
    ) -> ParsedSkillSource {
        ParsedSkillSource {
            kind: ParsedSkillSourceKind::Github,
            repo: Some(repo.to_string()),
            path: path.map(String::from),
            git_ref: None,
            skill_name: skill_name.map(String::from),
            url: None,
            local_path: None,
        }
    }

    fn base_request(source: ParsedSkillSource, method: AddMethod) -> AddSkillRequest {
        AddSkillRequest {
            source,
            method,
            destination: SkillDestination::Universal,
            agents: vec![],
            disabled_harnesses: vec![],
            scope: InstallScope::Global,
            project_path: None,
            trial: false,
        }
    }

    fn local_source(path: &Path, skill_name: &str) -> ParsedSkillSource {
        ParsedSkillSource {
            kind: ParsedSkillSourceKind::Local,
            repo: None,
            path: None,
            git_ref: None,
            skill_name: Some(skill_name.to_string()),
            url: None,
            local_path: Some(path.to_string_lossy().to_string()),
        }
    }

    #[test]
    fn single_per_harness_trial_is_rejected_before_creating_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let mut request = base_request(
            github_source("getsentry/find-bugs", None, Some("find-bugs")),
            AddMethod::Copy,
        );
        request.destination = SkillDestination::PerHarness;
        request.agents = vec![AgentId::ClaudeCode];
        request.trial = true;

        let error = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();

        assert_eq!(error, "Trials require the Universal destination");
        assert!(!home.exists());
    }

    #[test]
    fn batch_per_harness_trial_is_rejected_before_creating_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let mut request = batch_request(
            github_source("getsentry/find-bugs", None, None),
            AddMethod::Copy,
            vec![entry("find-bugs", "skills/find-bugs")],
        );
        request.destination = SkillDestination::PerHarness;
        request.agents = vec![AgentId::ClaudeCode];
        request.trial = true;

        let error = add_skills_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();

        assert_eq!(error, "Trials require the Universal destination");
        assert!(!home.exists());
    }

    #[test]
    fn dotagents_argv_includes_name_and_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut source = github_source(
            "getsentry/find-bugs",
            Some("skills/find-bugs"),
            Some("find-bugs"),
        );
        source.git_ref = Some("v2".to_string());
        let request = base_request(source, AddMethod::Dotagents);

        let runner = FakeRunner {
            creates: vec![home.join(".agents/skills/find-bugs")],
            ..Default::default()
        };
        let result = add_skill_with(
            &home,
            &request,
            &runner,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        assert_eq!(result.tool, "dotagents");
        assert_eq!(result.name, "find-bugs");
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (args, cwd) = &calls[0];
        assert_eq!(
            args,
            &vec![
                "-y",
                "@sentry/dotagents",
                "add",
                "getsentry/find-bugs",
                "--name",
                "find-bugs",
                "--ref",
                "v2",
            ]
        );
        assert!(cwd.is_none());
    }

    #[test]
    fn dotagents_project_scope_passes_project_flag_and_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let source = github_source("getsentry/find-bugs", None, Some("find-bugs"));
        let mut request = base_request(source, AddMethod::Dotagents);
        request.scope = InstallScope::Project;
        request.project_path = Some(project.to_string_lossy().to_string());

        let runner = FakeRunner {
            creates: vec![project.join(".agents/skills/find-bugs")],
            ..Default::default()
        };
        add_skill_with(
            &home,
            &request,
            &runner,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        let calls = runner.calls.lock().unwrap();
        assert!(calls[0].0.contains(&"--project".to_string()));
        assert_eq!(calls[0].1, Some(project.clone()));
    }

    #[test]
    fn dotagents_without_a_skill_name_diffs_the_shared_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        fs::create_dir_all(home.join(".agents/skills/existing")).unwrap();
        let source = github_source("getsentry/many-skills", None, None);
        let request = base_request(source, AddMethod::Dotagents);

        let runner = FakeRunner {
            creates: vec![
                home.join(".agents/skills/one"),
                home.join(".agents/skills/two"),
            ],
            ..Default::default()
        };
        let result = add_skill_with(
            &home,
            &request,
            &runner,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        assert_eq!(result.name, "one, two");
    }

    #[test]
    fn skills_sh_argv_uses_skill_and_agent_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let source = github_source("getsentry/find-bugs", None, Some("find-bugs"));
        let mut request = base_request(source, AddMethod::SkillsSh);
        request.agents = vec![AgentId::ClaudeCode, AgentId::GrokBuild];

        let runner = FakeRunner::default();
        let result = add_skill_with(
            &home,
            &request,
            &runner,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        assert_eq!(result.tool, "skills-sh");
        let calls = runner.calls.lock().unwrap();
        let args = &calls[0].0;
        assert!(args.contains(&"--skill".to_string()));
        assert!(args.contains(&"--agent".to_string()));
        assert!(args.contains(&"claude-code".to_string()));
        // Grok Build must never reach the CLI's argv.
        assert!(!args.contains(&"grok-build".to_string()));
    }

    #[test]
    fn copy_via_github_fetches_into_the_shared_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let source = github_source(
            "getsentry/find-bugs",
            Some("skills/find-bugs"),
            Some("find-bugs"),
        );
        let request = base_request(source, AddMethod::Copy);

        struct FakeFetch;
        impl UpstreamFetch for FakeFetch {
            fn fetch_skill_dir(
                &self,
                _repo: &str,
                _path: &str,
                _commit: &str,
                into: &Path,
            ) -> Result<(), String> {
                fs::create_dir_all(into).unwrap();
                fs::write(into.join("SKILL.md"), "body").unwrap();
                Ok(())
            }
        }
        struct FakeLookup;
        impl CommitLookup for FakeLookup {
            fn latest_commit(
                &self,
                _: &str,
                _: &str,
                _: Option<&str>,
            ) -> Result<Option<(String, String)>, String> {
                Ok(Some(("a".repeat(40), "2026-01-01T00:00:00Z".to_string())))
            }
        }

        let result = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &FakeFetch,
            &FakeLookup,
        )
        .unwrap();
        assert_eq!(result.tool, "copy");
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").exists());
        let registry = super::super::skill_fork_registry::read_fork_registry(&home).unwrap();
        assert_eq!(
            registry.version,
            super::super::skill_fork_registry::CURRENT_REGISTRY_VERSION
        );
        let record = registry.copies.values().next().unwrap();
        assert_eq!(record.path, home.join(".agents/skills/find-bugs"));
        assert_eq!(record.scope, InstallScope::Global);
        assert_eq!(record.destination, SkillDestination::Universal);
        assert_eq!(record.slot, "universal");
    }

    #[test]
    fn copy_refuses_an_existing_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        fs::create_dir_all(home.join(".agents/skills/find-bugs")).unwrap();
        let source = github_source(
            "getsentry/find-bugs",
            Some("skills/find-bugs"),
            Some("find-bugs"),
        );
        let request = base_request(source, AddMethod::Copy);

        let err = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn copy_acquisition_failure_removes_partial_stage_and_allows_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("skills/foo");
        let targets = vec![target.clone()];

        let error = commit_copy_install(
            &targets,
            None,
            |stage| {
                fs::create_dir_all(stage).unwrap();
                fs::write(stage.join("partial"), "partial").unwrap();
                Err("injected mid-copy failure".to_string())
            },
            copy_dir_all,
        )
        .unwrap_err();

        assert!(error.contains("mid-copy"));
        assert!(!target.exists());
        assert!(fs::read_dir(target.parent().unwrap())
            .unwrap()
            .next()
            .is_none());
        commit_copy_install(
            &targets,
            None,
            |stage| {
                fs::create_dir_all(stage).map_err(|error| error.to_string())?;
                fs::write(stage.join("SKILL.md"), "body").map_err(|error| error.to_string())
            },
            copy_dir_all,
        )
        .unwrap();
        assert!(target.join("SKILL.md").exists());
    }

    #[test]
    fn later_copy_target_failure_leaves_no_destination_or_stage() {
        let tmp = tempfile::tempdir().unwrap();
        let targets = vec![
            tmp.path().join("claude/foo"),
            tmp.path().join("codex/foo"),
            tmp.path().join("pi/foo"),
        ];
        let copy_count = Mutex::new(0);

        let error = commit_copy_install(
            &targets,
            None,
            |stage| {
                fs::create_dir_all(stage).map_err(|error| error.to_string())?;
                fs::write(stage.join("SKILL.md"), "body").map_err(|error| error.to_string())
            },
            |source, destination| {
                let mut count = copy_count.lock().unwrap();
                *count += 1;
                if *count == 2 {
                    fs::create_dir_all(destination).unwrap();
                    fs::write(destination.join("partial"), "partial").unwrap();
                    return Err("injected later-target failure".to_string());
                }
                copy_dir_all(source, destination)
            },
        )
        .unwrap_err();

        assert!(error.contains("later-target"));
        assert!(targets.iter().all(|target| !target.exists()));
        for parent in targets.iter().filter_map(|target| target.parent()) {
            assert!(fs::read_dir(parent).unwrap().next().is_none());
        }
    }

    #[test]
    fn copy_via_local_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let local = tmp.path().join("my-skill");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join("SKILL.md"), "body").unwrap();
        let source = ParsedSkillSource {
            kind: ParsedSkillSourceKind::Local,
            repo: None,
            path: None,
            git_ref: None,
            skill_name: None,
            url: None,
            local_path: Some(local.to_string_lossy().to_string()),
        };
        let request = base_request(source, AddMethod::Copy);
        let result = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        assert_eq!(result.name, "my-skill");
        assert!(home.join(".agents/skills/my-skill/SKILL.md").exists());
    }

    #[test]
    fn local_copy_rejects_home_ancestor_for_universal_and_per_harness() {
        for destination in [SkillDestination::Universal, SkillDestination::PerHarness] {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            fs::create_dir_all(&home).unwrap();
            fs::write(home.join("SKILL.md"), "body").unwrap();
            let mut request = base_request(local_source(&home, "nested-copy"), AddMethod::Copy);
            request.destination = destination;
            request.agents = vec![AgentId::ClaudeCode];

            let error = add_skill_with(
                &home,
                &request,
                &FakeRunner::default(),
                &NeverCalledFetch,
                &NeverCalledLookup,
            )
            .unwrap_err();

            assert!(error.contains("outside the source directory"));
            assert!(!home.join(".agents").exists());
            assert!(!home.join(".claude").exists());
        }
    }

    #[test]
    fn local_copy_rejects_project_ancestor_for_universal_and_per_harness() {
        for destination in [SkillDestination::Universal, SkillDestination::PerHarness] {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            let project = tmp.path().join("project");
            fs::create_dir_all(&project).unwrap();
            fs::write(project.join("SKILL.md"), "body").unwrap();
            let mut request = base_request(local_source(&project, "nested-copy"), AddMethod::Copy);
            request.scope = InstallScope::Project;
            request.project_path = Some(project.to_string_lossy().to_string());
            request.destination = destination;
            request.agents = vec![AgentId::ClaudeCode];

            let error = add_skill_with(
                &home,
                &request,
                &FakeRunner::default(),
                &NeverCalledFetch,
                &NeverCalledLookup,
            )
            .unwrap_err();

            assert!(error.contains("outside the source directory"));
            assert!(!project.join(".agents").exists());
            assert!(!project.join(".claude").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn local_copy_rejects_a_symlink_source_that_resolves_to_destination_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let source_link = tmp.path().join("source-link");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("SKILL.md"), "body").unwrap();
        std::os::unix::fs::symlink(&home, &source_link).unwrap();
        let request = base_request(local_source(&source_link, "nested-copy"), AddMethod::Copy);

        let error = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();

        assert!(error.contains("outside the source directory"));
        assert!(!home.join(".agents").exists());
    }

    #[cfg(unix)]
    #[test]
    fn local_copy_rejects_a_destination_parent_symlinked_into_the_source() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let source = tmp.path().join("source");
        let linked_parent = source.join("linked-parent");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&linked_parent).unwrap();
        fs::write(source.join("SKILL.md"), "body").unwrap();
        std::os::unix::fs::symlink(&linked_parent, home.join(".agents")).unwrap();
        let request = base_request(local_source(&source, "nested-copy"), AddMethod::Copy);

        let error = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();

        assert!(error.contains("outside the source directory"));
        assert!(!linked_parent.join("skills").exists());
        assert!(fs::read_dir(&linked_parent).unwrap().next().is_none());
    }

    #[test]
    fn local_copy_rejects_source_equal_to_target_without_modifying_it() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let source = home.join(".agents/skills/existing");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("SKILL.md"), "body").unwrap();
        let request = base_request(local_source(&source, "existing"), AddMethod::Copy);

        let error = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();

        assert!(error.contains("outside the source directory"));
        assert_eq!(fs::read_to_string(source.join("SKILL.md")).unwrap(), "body");
        assert_eq!(fs::read_dir(source.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn per_harness_copy_records_each_exact_created_deployment() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let local = tmp.path().join("my-skill");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join("SKILL.md"), "body").unwrap();
        let source = ParsedSkillSource {
            kind: ParsedSkillSourceKind::Local,
            repo: None,
            path: None,
            git_ref: None,
            skill_name: None,
            url: None,
            local_path: Some(local.to_string_lossy().to_string()),
        };
        let mut request = base_request(source, AddMethod::Copy);
        request.destination = SkillDestination::PerHarness;
        request.agents = vec![
            AgentId::ClaudeCode,
            AgentId::Codex,
            AgentId::OpenCode,
            AgentId::Pi,
            AgentId::Cursor,
            AgentId::GrokBuild,
        ];

        add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();

        let registry = super::super::skill_fork_registry::read_fork_registry(&home).unwrap();
        assert_eq!(registry.copies.len(), 6);
        for agent in &request.agents {
            let path = agent.global_skills_dir(&home).join("my-skill");
            let id = installed_deployment_id(&request, "my-skill", &path, agent.cli_name());
            let record = registry.copies.get(&id).unwrap();
            assert_eq!(record.path, path);
            assert_eq!(record.slot, agent.cli_name());
            assert_eq!(record.destination, SkillDestination::PerHarness);
        }
    }

    #[test]
    fn copy_refuses_malformed_existing_registry_without_creating_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let local = tmp.path().join("my-skill");
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(home.join(".agents/skill-studio.json"), "not json").unwrap();
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join("SKILL.md"), "body").unwrap();
        let source = ParsedSkillSource {
            kind: ParsedSkillSourceKind::Local,
            repo: None,
            path: None,
            git_ref: None,
            skill_name: None,
            url: None,
            local_path: Some(local.to_string_lossy().to_string()),
        };
        let request = base_request(source, AddMethod::Copy);

        assert!(add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .is_err());
        assert!(!home.join(".agents/skills/my-skill").exists());
    }

    #[test]
    fn claude_code_symlink_created_only_for_a_real_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        fs::create_dir_all(home.join(".agents/skills")).unwrap();
        // A real (not symlinked) `.claude/skills` directory.
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        let source = github_source("getsentry/find-bugs", None, Some("find-bugs"));
        let mut request = base_request(source, AddMethod::Dotagents);
        request.agents = vec![AgentId::ClaudeCode];

        let runner = FakeRunner {
            creates: vec![home.join(".agents/skills/find-bugs")],
            ..Default::default()
        };
        let result = add_skill_with(
            &home,
            &request,
            &runner,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        let link = home.join(".claude/skills/find-bugs");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("../../.agents/skills/find-bugs")
        );
        assert_eq!(
            link.canonicalize().unwrap(),
            home.join(".agents/skills/find-bugs")
                .canonicalize()
                .unwrap()
        );
        assert!(result
            .deployments_created
            .iter()
            .any(|d| d.contains(".claude/skills/find-bugs")));
    }

    #[test]
    fn relative_path_between_walks_up_to_the_common_parent() {
        assert_eq!(
            relative_path_between(
                Path::new("/h/.claude/skills"),
                Path::new("/h/.agents/skills")
            ),
            PathBuf::from("../../.agents/skills")
        );
        assert_eq!(
            relative_path_between(
                Path::new("/p/.claude/skills"),
                Path::new("/p/.agents/skills")
            ),
            PathBuf::from("../../.agents/skills")
        );
    }

    #[test]
    fn claude_code_symlink_skipped_when_claude_skills_is_the_whole_dir_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        fs::create_dir_all(home.join(".agents/skills")).unwrap();
        fs::create_dir_all(home.join(".claude")).unwrap();
        std::os::unix::fs::symlink(home.join(".agents/skills"), home.join(".claude/skills"))
            .unwrap();
        let source = github_source("getsentry/find-bugs", None, Some("find-bugs"));
        let mut request = base_request(source, AddMethod::Dotagents);
        request.agents = vec![AgentId::ClaudeCode];

        let runner = FakeRunner {
            creates: vec![home.join(".agents/skills/find-bugs")],
            ..Default::default()
        };
        let result = add_skill_with(
            &home,
            &request,
            &runner,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        // No per-skill symlink created - the whole-dir symlink already covers it.
        assert!(!result
            .deployments_created
            .iter()
            .any(|d| d.contains("find-bugs")
                && d != &home
                    .join(".agents/skills/find-bugs")
                    .to_string_lossy()
                    .to_string()));
    }

    #[test]
    fn skills_sh_reports_universal_and_selected_claude_deployments() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        fs::create_dir_all(home.join(".claude/skills")).unwrap();
        let source = github_source("getsentry/find-bugs", None, Some("find-bugs"));
        let mut request = base_request(source, AddMethod::SkillsSh);
        request.agents = vec![AgentId::ClaudeCode];

        let result = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        assert!(fs::symlink_metadata(home.join(".claude/skills/find-bugs")).is_err());
        assert_eq!(result.deployments_created.len(), 2);
    }

    #[test]
    fn skills_sh_trial_records_the_exact_claude_link_created_by_the_cli() {
        struct SkillsShLinkRunner {
            home: PathBuf,
        }

        impl CommandRunner for SkillsShLinkRunner {
            fn run_npx(&self, _args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
                let skill_dir = self.home.join(".agents/skills/find-bugs");
                fs::create_dir_all(&skill_dir).unwrap();
                fs::write(skill_dir.join("SKILL.md"), "body").unwrap();
                let link = self.home.join(".claude/skills/find-bugs");
                fs::create_dir_all(link.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink("../../.agents/skills/find-bugs", link).unwrap();
                Ok(())
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let source = github_source("getsentry/find-bugs", None, Some("find-bugs"));
        let mut request = base_request(source, AddMethod::SkillsSh);
        request.agents = vec![AgentId::ClaudeCode];
        request.trial = true;

        add_skill_with(
            &home,
            &request,
            &SkillsShLinkRunner { home: home.clone() },
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();

        let registry = super::super::skill_fork_registry::read_fork_registry(&home).unwrap();
        let trial = registry.trials.values().next().unwrap();
        assert_eq!(
            trial.claude_link,
            Some(home.join(".claude/skills/find-bugs"))
        );
        assert_eq!(
            trial.claude_link_target,
            Some(PathBuf::from("../../.agents/skills/find-bugs"))
        );
    }

    #[test]
    fn copy_rejects_a_path_traversal_skill_name() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut source = github_source(
            "getsentry/find-bugs",
            Some("skills/find-bugs"),
            Some("../../etc"),
        );
        source.skill_name = Some("../evil".to_string());
        let request = base_request(source, AddMethod::Copy);

        let err = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();
        assert!(!home.join(".agents/skills/evil").exists());
        assert!(!err.is_empty());
    }

    #[test]
    fn copy_rejects_a_path_traversal_repo_subpath() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let source = github_source(
            "getsentry/find-bugs",
            Some("../../../etc"),
            Some("find-bugs"),
        );
        let request = base_request(source, AddMethod::Copy);

        let err = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();
        assert!(err.contains("..") || err.contains("path"));
    }

    #[test]
    fn copy_rejects_a_local_path_that_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let source = ParsedSkillSource {
            kind: ParsedSkillSourceKind::Local,
            repo: None,
            path: None,
            git_ref: None,
            skill_name: None,
            url: None,
            local_path: Some(
                tmp.path()
                    .join("does-not-exist")
                    .to_string_lossy()
                    .to_string(),
            ),
        };
        let request = base_request(source, AddMethod::Copy);

        let err = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();
        assert!(err.contains("Could not resolve"));
    }

    #[test]
    fn copy_project_scope_lands_under_the_project_and_expires_from_there() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let local = tmp.path().join("my-skill");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join("SKILL.md"), "body").unwrap();

        let source = ParsedSkillSource {
            kind: ParsedSkillSourceKind::Local,
            repo: None,
            path: None,
            git_ref: None,
            skill_name: None,
            url: None,
            local_path: Some(local.to_string_lossy().to_string()),
        };
        let mut request = base_request(source, AddMethod::Copy);
        request.scope = InstallScope::Project;
        request.project_path = Some(project.to_string_lossy().to_string());
        request.trial = true;

        let result = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        assert!(result.warning.is_none());
        let installed = project.join(".agents/skills/my-skill");
        assert!(installed.join("SKILL.md").exists());
        // Global shared folder is untouched.
        assert!(!home.join(".agents/skills/my-skill").exists());

        // Expiry later resolves this exact deployment from a fresh snapshot.
        let registry = super::super::skill_fork_registry::read_fork_registry(&home).unwrap();
        let trial = registry.trials.values().next().unwrap();
        assert_eq!(trial.skill_dir, installed);
        assert_eq!(trial.scope, TrialScope::Project);
        assert!(!trial.deployment_id.is_empty());
        assert!(!trial.deployment_fingerprint.is_empty());
    }

    #[test]
    fn multi_skill_dotagents_add_records_one_trial_per_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let source = github_source("getsentry/many-skills", None, None);
        let mut request = base_request(source, AddMethod::Dotagents);
        request.trial = true;

        let runner = FakeRunner {
            creates: vec![
                home.join(".agents/skills/one"),
                home.join(".agents/skills/two"),
            ],
            ..Default::default()
        };
        let result = add_skill_with(
            &home,
            &request,
            &runner,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        assert!(result.warning.is_none());

        let registry = super::super::skill_fork_registry::read_fork_registry(&home).unwrap();
        assert_eq!(registry.trials.len(), 2);
        for name in ["one", "two"] {
            let skill_dir = home.join(".agents/skills").join(name);
            let deployment_id = installed_deployment_id(&request, name, &skill_dir, "universal");
            let key = super::super::skill_fork_registry::deployment_trial_key(&deployment_id);
            assert_eq!(registry.trials[&key].deployment_id, deployment_id);
        }
    }

    fn entry(name: &str, path: &str) -> GithubSkillEntry {
        GithubSkillEntry {
            name: name.to_string(),
            path: path.to_string(),
        }
    }

    fn batch_request(
        source: ParsedSkillSource,
        method: AddMethod,
        skills: Vec<GithubSkillEntry>,
    ) -> AddSkillsRequest {
        AddSkillsRequest {
            source,
            skills,
            method,
            destination: SkillDestination::Universal,
            agents: vec![],
            disabled_harnesses: vec![],
            scope: InstallScope::Global,
            project_path: None,
            trial: false,
        }
    }

    /// An `UpstreamFetch` with a bulk mode, counting how many times the repo
    /// was downloaded.
    #[derive(Default)]
    struct CountingFetch {
        downloads: Mutex<usize>,
    }

    struct FakeSnapshot;
    impl RepoSnapshot for FakeSnapshot {
        fn copy_dir(&self, _path: &str, into: &Path) -> Result<(), String> {
            fs::create_dir_all(into).unwrap();
            fs::write(into.join("SKILL.md"), "body").unwrap();
            Ok(())
        }
    }

    impl UpstreamFetch for CountingFetch {
        fn fetch_skill_dir(&self, _: &str, _: &str, _: &str, _: &Path) -> Result<(), String> {
            panic!("a batch copy must extract from the snapshot, not refetch");
        }

        fn open_repo(&self, _: &str, _: &str) -> Result<Option<Box<dyn RepoSnapshot>>, String> {
            *self.downloads.lock().unwrap() += 1;
            Ok(Some(Box::new(FakeSnapshot)))
        }
    }

    #[test]
    fn copy_batch_downloads_the_repo_once_for_every_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let mut source = github_source("kentcdodds/kcd-skills", Some("skills"), None);
        source.git_ref = Some("main".to_string());
        let request = batch_request(
            source,
            AddMethod::Copy,
            vec![
                entry("visual-recap", "skills/visual-recap"),
                entry("other", "skills/other"),
                entry("third", "skills/third"),
            ],
        );

        let fetch = CountingFetch::default();
        let outcomes = add_skills_with(
            &home,
            &request,
            &FakeRunner::default(),
            &fetch,
            &NeverCalledLookup,
        )
        .unwrap();
        assert_eq!(outcomes.len(), 3);
        assert!(outcomes.iter().all(|o| o.error.is_none()));
        assert_eq!(*fetch.downloads.lock().unwrap(), 1);
        assert!(home.join(".agents/skills/visual-recap/SKILL.md").exists());
        assert!(home.join(".agents/skills/third/SKILL.md").exists());
    }

    #[test]
    fn a_failed_skill_does_not_stop_the_rest_of_the_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        fs::create_dir_all(home.join(".agents/skills/other")).unwrap();
        let mut source = github_source("kentcdodds/kcd-skills", Some("skills"), None);
        source.git_ref = Some("main".to_string());
        let request = batch_request(
            source,
            AddMethod::Copy,
            vec![
                entry("other", "skills/other"),
                entry("visual-recap", "skills/visual-recap"),
            ],
        );

        let outcomes = add_skills_with(
            &home,
            &request,
            &FakeRunner::default(),
            &CountingFetch::default(),
            &NeverCalledLookup,
        )
        .unwrap();
        assert!(outcomes[0]
            .error
            .as_deref()
            .unwrap()
            .contains("already exists"));
        assert!(outcomes[1].error.is_none());
        assert!(home.join(".agents/skills/visual-recap/SKILL.md").exists());
    }

    #[test]
    fn dotagents_batch_runs_one_cli_call_per_skill_with_its_own_name() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let request = batch_request(
            github_source("kentcdodds/kcd-skills", Some("skills"), None),
            AddMethod::Dotagents,
            vec![
                entry("visual-recap", "skills/visual-recap"),
                entry("other", "skills/other"),
            ],
        );

        let runner = FakeRunner {
            creates: vec![
                home.join(".agents/skills/visual-recap"),
                home.join(".agents/skills/other"),
            ],
            ..Default::default()
        };
        let outcomes = add_skills_with(
            &home,
            &request,
            &runner,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        assert_eq!(outcomes.len(), 2);
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].0.contains(&"--name".to_string()));
        assert!(calls[0].0.contains(&"visual-recap".to_string()));
        assert!(calls[1].0.contains(&"other".to_string()));
    }

    #[test]
    fn skills_sh_batch_passes_each_skill_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let request = batch_request(
            github_source("kentcdodds/kcd-skills", Some("skills"), None),
            AddMethod::SkillsSh,
            vec![
                entry("visual-recap", "skills/visual-recap"),
                entry("other", "skills/other"),
            ],
        );

        let runner = FakeRunner::default();
        add_skills_with(
            &home,
            &request,
            &runner,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].0.contains(&"--skill".to_string()));
        assert!(calls[0].0.contains(&"visual-recap".to_string()));
        assert!(calls[1].0.contains(&"other".to_string()));
    }

    #[test]
    fn trial_recording_failure_surfaces_as_a_warning_and_keeps_the_install() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        // Make the registry file itself a directory, so writing it fails.
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::create_dir_all(home.join(".agents/skill-studio.json")).unwrap();

        let source = github_source(
            "getsentry/find-bugs",
            Some("skills/find-bugs"),
            Some("find-bugs"),
        );
        let mut request = base_request(source, AddMethod::Dotagents);
        request.trial = true;

        let runner = FakeRunner {
            creates: vec![home.join(".agents/skills/find-bugs")],
            ..Default::default()
        };
        let result = add_skill_with(
            &home,
            &request,
            &runner,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap();
        assert!(home.join(".agents/skills/find-bugs").exists());
        let warning = result.warning.expect("expected a trial warning");
        assert!(warning.contains("24 h trial could not be recorded"));
    }

    #[test]
    fn copy_fingerprint_failure_rolls_back_installed_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let source_dir = tmp.path().join("source");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("README.md"), "missing SKILL.md").unwrap();
        let source = ParsedSkillSource {
            kind: ParsedSkillSourceKind::Local,
            repo: None,
            path: None,
            git_ref: None,
            skill_name: Some("find-bugs".to_string()),
            url: None,
            local_path: Some(source_dir.to_string_lossy().to_string()),
        };
        let request = base_request(source, AddMethod::Copy);

        let error = add_skill_with(
            &home,
            &request,
            &FakeRunner::default(),
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();

        assert!(
            error.contains("Failed to fingerprint Copy ownership"),
            "{error}"
        );
        assert!(!home.join(".agents/skills/find-bugs").exists());
        assert!(super::super::skill_fork_registry::read_fork_registry(&home)
            .unwrap()
            .copies
            .is_empty());
    }

    /// Installs `find-bugs` through dotagents with `disabled_harnesses` set,
    /// and hands back the result for the caller to assert on.
    fn add_with_disabled(home: &Path, disabled: Vec<AgentId>) -> AddSkillResult {
        let source = github_source(
            "getsentry/find-bugs",
            Some("skills/find-bugs"),
            Some("find-bugs"),
        );
        let mut request = base_request(source, AddMethod::Dotagents);
        request.disabled_harnesses = disabled;
        let runner = FakeRunner {
            creates: vec![home.join(".agents/skills/find-bugs")],
            ..Default::default()
        };
        add_skill_with(
            home,
            &request,
            &runner,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap()
    }

    #[test]
    fn a_disabled_codex_reader_is_written_to_codex_config() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        let result = add_with_disabled(home, vec![AgentId::Codex]);

        assert!(result.warning.is_none());
        assert_eq!(
            super::super::codex_skill_config::read_disabled_skill_md_paths(home),
            vec![home.join(".agents/skills/find-bugs/SKILL.md")]
        );
    }

    #[test]
    fn a_disabled_claude_code_under_a_whole_dir_link_becomes_a_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".agents/skills")).unwrap();
        fs::create_dir_all(home.join(".claude")).unwrap();
        std::os::unix::fs::symlink(home.join(".agents/skills"), home.join(".claude/skills"))
            .unwrap();

        let result = add_with_disabled(home, vec![AgentId::ClaudeCode]);

        assert!(home.join(".agents/skills/find-bugs").exists());
        let warning = result.warning.expect("expected a disable warning");
        assert!(
            warning.starts_with("Installed, but could not turn it off for Claude Code:"),
            "{warning}"
        );
        assert!(warning.contains("whole shared folder"), "{warning}");
    }

    #[test]
    fn a_harness_that_cannot_be_disabled_becomes_a_warning_not_a_failed_install() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        let result = add_with_disabled(home, vec![AgentId::Pi]);

        assert!(home.join(".agents/skills/find-bugs").exists());
        let warning = result.warning.expect("expected a disable warning");
        assert!(
            warning.contains("could not turn it off for pi: pi has no per-skill disable"),
            "{warning}"
        );
    }
}
