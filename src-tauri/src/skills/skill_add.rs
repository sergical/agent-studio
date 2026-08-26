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
use std::path::{Path, PathBuf};
use std::process::Command;

use tauri::Manager;

use super::agents::AgentId;
use super::commands::{push_agent_args, skills_sh_add_args};
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_dto::{
    AddSkillRequest, AddSkillResult, InstallScope, ParsedSkillSource, ParsedSkillSourceKind,
};
use super::skill_fork::{copy_dir_all, ForkMutationLock, RealUpstreamFetch, UpstreamFetch};
use super::skill_fork_registry::{AddMethod, TrialScope};
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
            let repo = source.repo.clone().ok_or("A GitHub source needs a repo")?;
            Ok(match &source.path {
                Some(path) => format!("{repo}/{path}"),
                None => repo,
            })
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
    installs: &[(String, PathBuf, Option<PathBuf>)],
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
    for (name, skill_dir, claude_link) in installs {
        if let Err(e) = skill_trial::record_trial(
            home,
            name,
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
    let mut installs: Vec<(String, PathBuf, Option<PathBuf>)> = Vec::new();
    for name in &new_names {
        let claude_link =
            maybe_claude_code_symlink(&claude_dir, &shared_dir, name, &request.agents)?
                .map(PathBuf::from);
        if let Some(link) = &claude_link {
            deployments_created.push(link.to_string_lossy().to_string());
        }
        installs.push((name.clone(), shared_dir.join(name), claude_link));
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
    let global = request.scope == InstallScope::Global;

    let mut args = skills_sh_add_args(
        &repo,
        skill_name.as_deref(),
        global,
        request.project_path.as_deref(),
    );
    // Grok Build isn't an `npx skills` install target - it only reads the
    // shared folder - so it's dropped before `push_agent_args`, which would
    // otherwise refuse the whole request over it.
    let installable: Vec<AgentId> = request
        .agents
        .iter()
        .copied()
        .filter(|a| *a != AgentId::GrokBuild)
        .collect();
    push_agent_args(&mut args, &installable)?;

    runner.run_npx(&args, None)?;

    let result_name =
        skill_name.unwrap_or_else(|| repo.split('/').next_back().unwrap_or(&repo).to_string());

    let deployment_dirs: Vec<PathBuf> = installable
        .iter()
        .map(|agent| {
            if global {
                agent.global_skills_dir(home)
            } else {
                agent.project_skills_dir(Path::new(request.project_path.as_deref().unwrap_or("")))
            }
        })
        .collect();
    let deployments_created = deployment_dirs
        .iter()
        .map(|dir| dir.join(&result_name).to_string_lossy().to_string())
        .collect();

    // `skills.sh` writes into each selected agent's own directory, not a
    // single shared one - there's no per-app symlink to track here, so the
    // trial only ever has a `skill_dir` (the first installed agent's copy)
    // and no `claude_link`.
    let warning = deployment_dirs.first().and_then(|dir| {
        maybe_record_trials(
            home,
            request,
            &[(result_name.clone(), dir.join(&result_name), None)],
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

fn add_via_copy(
    home: &Path,
    request: &AddSkillRequest,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
) -> Result<AddSkillResult, String> {
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
    let shared_dir = shared_skills_dir(home, request);
    let claude_dir = claude_skills_dir(home, request);
    let target = shared_dir.join(&name);
    if target.parent() != Some(shared_dir.as_path()) {
        return Err("Refusing to write outside the skills folder".to_string());
    }
    if target.exists() {
        return Err(format!(
            "`{name}` already exists in {}",
            shared_dir.display()
        ));
    }

    match request.source.kind {
        ParsedSkillSourceKind::Github => {
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
                    .ok_or_else(|| format!("Could not determine {name}'s latest commit"))?,
            };
            fetch.fetch_skill_dir(&repo, &path, &commit, &target)?;
        }
        ParsedSkillSourceKind::Local => {
            let local_path = request
                .source
                .local_path
                .clone()
                .ok_or("A local source needs a path")?;
            copy_dir_all(Path::new(&local_path), &target)?;
        }
        ParsedSkillSourceKind::Git => {
            return Err("Copy is not supported for git sources; use dotagents".to_string());
        }
    }

    let mut deployments_created = vec![target.to_string_lossy().to_string()];
    let claude_link = maybe_claude_code_symlink(&claude_dir, &shared_dir, &name, &request.agents)?
        .map(PathBuf::from);
    if let Some(link) = &claude_link {
        deployments_created.push(link.to_string_lossy().to_string());
    }
    let warning = maybe_record_trials(
        home,
        request,
        &[(name.clone(), target.clone(), claude_link)],
    );

    Ok(AddSkillResult {
        name,
        tool: "copy".to_string(),
        command: format!("copy -> {}", target.display()),
        deployments_created,
        warning,
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

/// `add_skill`'s logic, taking `home`/traits directly so it's testable
/// without a Tauri `AppHandle` or a network call.
pub fn add_skill_with(
    home: &Path,
    request: &AddSkillRequest,
    runner: &dyn CommandRunner,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
) -> Result<AddSkillResult, String> {
    validate_parsed_source(&request.source)?;
    match request.method {
        AddMethod::Dotagents => add_via_dotagents(home, request, runner),
        AddMethod::SkillsSh => add_via_skills_sh(home, request, runner),
        AddMethod::Copy => add_via_copy(home, request, fetch, lookup),
    }
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
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not resolve app data dir: {e}"))?;
    let runner = RealCommandRunner;
    let (fetch, lookup): (Box<dyn UpstreamFetch>, Box<dyn CommitLookup>) =
        match skill_update_check::resolve_gh_binary() {
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
        };

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
            agents: vec![],
            scope: InstallScope::Global,
            project_path: None,
            trial: false,
        }
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
                "getsentry/find-bugs/skills/find-bugs",
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
    fn claude_code_symlink_not_created_for_skills_sh_method() {
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
        assert_eq!(result.deployments_created.len(), 1);
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

        // The trial's `skill_dir` is the project copy, so expiry removes it,
        // not anything under the home directory.
        let expired = skill_trial::run_trial_expiry_pass(
            &home,
            chrono::Utc::now() + chrono::Duration::hours(25),
            &FakeRunner::default(),
        );
        assert_eq!(expired.len(), 1);
        assert!(!installed.exists());
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
        assert!(registry.trials.contains_key("global/one"));
        assert!(registry.trials.contains_key("global/two"));
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
}
