// ============================================================================
// Skills Module - skill_pack
// "Share a pack": bundles every chosen member into a plain,
// dotagents-compatible git repo under `~/.agents/packs/<name>/` - every
// member's own files under `skills/<name>/`, plus an `agents.toml`
// `[[skills]]` row for provenance on the ones dotagents, skills.sh, or a
// fork manages (fork = both a row for the origin and a bundled copy of the
// edits), plus a generated `README.md`. `create`/`update`/`publish`/`delete`
// all take `ForkMutationLock` and write the registry
// (`~/.agents/skill-studio.json`) last, temp+rename via
// `skill_fork_registry::write_fork_registry`. `import_skill_pack` is the
// read side: given "owner/repo", it fetches that repo's `agents.toml`
// (read-only `gh api`) and installs through dotagents, same as any other
// GitHub source - see `skill_add`.
//
// GitHub rule: this module never creates a repo or pushes except from
// `publish_skill_pack`, which itself confirms with the user through
// `PublishConfirm` (a native `tauri_plugin_dialog` message box) right before
// any `gh`/`git` call - the frontend no longer confirms this one itself (see
// `SkillDetailActions`'s Un-fork dialog for the pattern still used
// elsewhere). `delete_skill_pack` never touches GitHub at all.
// ============================================================================

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

use super::agents::AgentId;
use super::commands::dotagents_add_args;
use super::dotagents_ledger;
use super::gh_cli::{run_gh, GhError};
use super::lock_file;
use super::skill_add::{maybe_claude_code_symlink, CommandRunner, RealCommandRunner};
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_fork::ForkMutationLock;
use super::skill_fork_registry::{self, PackMember, PackRecord};
use super::skill_fs::copy_dir_all;
use super::skill_refresh;
use super::skill_update_check;

// ============================================================================
// DTOs
// ============================================================================

/// One skill pack, as sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackInfo {
    pub name: String,
    pub created_at: String,
    pub dir: String,
    pub repo: Option<String>,
    pub skills: Vec<String>,
}

impl PackInfo {
    fn from_record(home: &Path, name: &str, record: &PackRecord) -> Self {
        PackInfo {
            name: name.to_string(),
            created_at: record.created_at.clone(),
            dir: record.dir.to_string_lossy().to_string(),
            repo: record.repo.clone(),
            skills: record_members(home, record)
                .into_iter()
                .map(|m| m.name)
                .collect(),
        }
    }
}

/// One pack member, as sent from the frontend: `name` is the skill's
/// directory name, `path` is the exact deployment directory to bundle from
/// (the row's `Deployment.path`) - see `resolve_members`.
#[derive(Debug, Clone, Deserialize)]
pub struct PackMemberInput {
    pub name: String,
    pub path: String,
}

/// Result of `update_skill_pack`: whether the rebuilt tree actually differed
/// from the pack's last commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePackResult {
    pub changed: bool,
    pub pack: PackInfo,
}

/// Result of `import_skill_pack`: which names came from the repo's own
/// `skills/` tree (`--all`) versus a `[[skills]]` row pointing elsewhere,
/// and any per-row failures (a partial import still reports what worked).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImportResult {
    pub bundled: Vec<String>,
    pub referenced: Vec<String>,
    pub errors: Vec<String>,
}

// ============================================================================
// Pack name validation - checked at the IPC boundary before `name` is
// joined into `~/.agents/packs/<name>`.
// ============================================================================

/// `^[a-z0-9][a-z0-9-]{0,63}$` - must start with a letter or digit, so a
/// pack name can never itself look like a flag when it ends up in an argv
/// somewhere. Mirrors `src/lib/skill-pack-name.ts`; keep both in sync.
pub(crate) fn validate_pack_name(name: &str) -> Result<&str, String> {
    if name.is_empty() || name.len() > 64 {
        return Err(format!("Invalid pack name: {name:?}"));
    }
    let first = name.chars().next().unwrap();
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!(
            "Pack name must start with a lowercase letter or digit, got {name:?}"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!("Pack name must match [a-z0-9-]+, got {name:?}"));
    }
    Ok(name)
}

fn packs_root(home: &Path) -> PathBuf {
    home.join(".agents").join("packs")
}

fn pack_dir(home: &Path, name: &str) -> PathBuf {
    packs_root(home).join(name)
}

/// Refuses when `record_dir` isn't exactly the directory `pack_dir` would
/// compute for `name` - a tampered `~/.agents/skill-studio.json` is the only
/// way this can happen, since `create_skill_pack` always writes `pack_dir`.
fn require_pack_dir(home: &Path, name: &str, record_dir: &Path) -> Result<(), String> {
    if record_dir != pack_dir(home, name) {
        return Err(format!(
            "Pack record for {name} points outside ~/.agents/packs; fix ~/.agents/skill-studio.json by hand"
        ));
    }
    Ok(())
}

/// Before any `remove_dir_all(target)` under a pack's directory: canonicalize
/// `target`'s parent and the packs root, and refuse unless the parent
/// actually resolves inside it. `require_pack_dir` already caught a tampered
/// path string; this catches a symlink planted somewhere in between.
fn assert_removable_inside_packs_root(home: &Path, target: &Path) -> Result<(), String> {
    let root = packs_root(home);
    fs::create_dir_all(&root).map_err(|e| format!("Failed to create {}: {e}", root.display()))?;
    let canonical_root = fs::canonicalize(&root)
        .map_err(|e| format!("Failed to resolve {}: {e}", root.display()))?;
    let parent = target.parent().unwrap_or(target);
    if !parent.exists() {
        return Ok(());
    }
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|e| format!("Failed to resolve {}: {e}", parent.display()))?;
    if canonical_parent != canonical_root && !canonical_parent.starts_with(&canonical_root) {
        return Err(format!(
            "Refusing to remove {} - it resolves outside ~/.agents/packs",
            target.display()
        ));
    }
    Ok(())
}

/// A record's members, filling in the legacy `skills: Vec<String>` shape
/// (pre-`members`) by pointing each name at the shared skills folder - the
/// only place those old records' files could have come from. `members` wins
/// whenever it's non-empty, since new writes always populate it.
fn record_members(home: &Path, record: &PackRecord) -> Vec<PackMember> {
    if !record.members.is_empty() || record.skills.is_empty() {
        return record.members.clone();
    }
    record
        .skills
        .iter()
        .map(|name| PackMember {
            name: name.clone(),
            path: shared_skills_dir(home).join(name),
        })
        .collect()
}

/// The shared skills folder every "manual"/fork skill's own files live
/// under - the same location `skill_add`'s `dotagents`/`Copy` methods and
/// `skill_park` read and write.
fn shared_skills_dir(home: &Path) -> PathBuf {
    home.join(".agents").join("skills")
}

/// Validates and canonicalizes every requested member before anything is
/// built: `name` must pass `validate_skill_dir_name` and be unique, `path`
/// must canonicalize to an existing directory containing `SKILL.md` whose
/// final path component is exactly `name` - so a pack can never be told to
/// bundle from somewhere its own name doesn't match, or from a path that
/// doesn't exist.
fn resolve_members(members: &[PackMemberInput]) -> Result<Vec<PackMember>, String> {
    let mut seen = HashSet::new();
    let mut resolved = Vec::with_capacity(members.len());
    for member in members {
        validate_skill_dir_name(&member.name)?;
        if !seen.insert(member.name.clone()) {
            return Err(format!("duplicate skill name: {}", member.name));
        }
        let canonical = fs::canonicalize(&member.path)
            .map_err(|e| format!("{}: failed to resolve {}: {e}", member.name, member.path))?;
        if !canonical.is_dir() {
            return Err(format!(
                "{}: {} is not a directory",
                member.name, member.path
            ));
        }
        if !canonical.join("SKILL.md").is_file() {
            return Err(format!("{}: {} has no SKILL.md", member.name, member.path));
        }
        let final_component = canonical.file_name().and_then(|n| n.to_str());
        if final_component != Some(member.name.as_str()) {
            return Err(format!(
                "{}: {} doesn't end in a directory named {:?}",
                member.name, member.path, member.name
            ));
        }
        resolved.push(PackMember {
            name: member.name.clone(),
            path: canonical,
        });
    }
    Ok(resolved)
}

// ============================================================================
// Member classification - fork > dotagents > skills.sh > manual, same
// precedence `skill_update_check::build_candidates` uses.
// ============================================================================

/// How one pack member's files ended up on disk, driving the
/// manifest-row/bundle decision.
enum MemberKind {
    Fork {
        repo: String,
        path: String,
        base_commit: String,
    },
    Dotagents {
        source: String,
        path: String,
        r#ref: Option<String>,
    },
    SkillsSh {
        repo: String,
        path: String,
        r#ref: Option<String>,
    },
    Manual,
}

/// Looks up dotagents/skills.sh/fork provenance by name only when `member`'s
/// path is the shared skills root's own copy of that name - a project
/// deployment or a plugin-cache copy is never one of those tools' own
/// managed folder, so it's `Manual` (bundle-only) regardless of what a
/// same-named shared install might be.
fn classify_member(home: &Path, app_data: &Path, member: &PackMember) -> MemberKind {
    // `member.path` was canonicalized in `resolve_members`; the shared dir
    // has to go through the same canonicalization before comparing, or a
    // symlinked `$TMPDIR` (common in tests, and on macOS's `/tmp` ->
    // `/private/tmp`) makes every shared member look like a project one.
    let shared_path = shared_skills_dir(home).join(&member.name);
    let is_shared = fs::canonicalize(&shared_path)
        .map(|canonical| canonical == member.path)
        .unwrap_or(false);
    if !is_shared {
        return MemberKind::Manual;
    }
    classify_shared_member(home, app_data, &member.name)
}

fn classify_shared_member(home: &Path, app_data: &Path, name: &str) -> MemberKind {
    let registry = skill_fork_registry::read_fork_registry_or_default(home);
    if let Some(fork) = registry.forks.get(name) {
        return MemberKind::Fork {
            repo: fork.repo.clone(),
            path: fork.path.clone(),
            base_commit: fork.base_commit.clone(),
        };
    }

    let agents_dir = home.join(".agents");
    let dotagents_skills = dotagents_ledger::read_dotagents_ledger(&agents_dir).unwrap_or_default();
    if let Some(skill) = dotagents_skills.into_iter().find(|s| s.name == name) {
        // A resolved commit pins the pack to exactly what's installed;
        // `declared_ref` (a branch, or nothing at all) is only a fallback.
        let r#ref = skill.installed_commit.or(skill.declared_ref);
        return MemberKind::Dotagents {
            source: skill.source,
            path: skill.path,
            r#ref,
        };
    }

    let lock_path = agents_dir.join(".skill-lock.json");
    let lock =
        lock_file::read_lock_file_at(&lock_path).unwrap_or_else(|_| lock_file::SkillLockFile {
            version: 3,
            skills: std::collections::HashMap::new(),
        });
    if let Some(entry) = lock.skills.get(name) {
        let path = entry
            .skill_path
            .clone()
            .unwrap_or_default()
            .trim_end_matches("/SKILL.md")
            .to_string();
        let store = skill_update_check::read_update_check_store(app_data);
        let owner_id = format!("owner:v1/global/{name}");
        let r#ref = store
            .owners
            .get(&owner_id)
            .and_then(|s| s.installed_commit.clone());
        return MemberKind::SkillsSh {
            repo: entry.source.clone(),
            path,
            r#ref,
        };
    }

    MemberKind::Manual
}

// ============================================================================
// Building the pack tree
// ============================================================================

/// One `agents.toml` `[[skills]]` row - matches the shape
/// `dotagents_ledger`'s own manifest reader expects, so a pack repo is a
/// plain dotagents-compatible multi-skill repo.
#[derive(Debug, Clone, Serialize)]
struct ManifestRow {
    name: String,
    source: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    r#ref: Option<String>,
}

#[derive(Debug, Serialize)]
struct AgentsManifestOut {
    skills: Vec<ManifestRow>,
}

fn bundle_skill(source: &Path, skills_root: &Path, name: &str) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!(
            "{name} has no files under {} to bundle",
            source.display()
        ));
    }
    copy_dir_all(source, &skills_root.join(name))
}

/// Builds (or rebuilds) `pack_dir`'s `agents.toml`, `README.md`, and
/// `skills/` tree for `members`. Rebuilding starts from a clean `skills/`
/// tree so a member removed since the last build doesn't linger. Every
/// member is bundled under `skills/<name>/` regardless of provenance - a
/// manifest row is provenance, not a substitute for the files (F5).
fn build_pack_tree(
    home: &Path,
    app_data: &Path,
    dir: &Path,
    pack_name: &str,
    members: &[PackMember],
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;

    let skills_root = dir.join("skills");
    if skills_root.exists() {
        assert_removable_inside_packs_root(home, &skills_root)?;
        fs::remove_dir_all(&skills_root)
            .map_err(|e| format!("Failed to clear {}: {e}", skills_root.display()))?;
    }

    let mut rows: Vec<ManifestRow> = Vec::new();
    let mut readme_lines: Vec<String> = Vec::new();

    for member in members {
        validate_skill_dir_name(&member.name)?;
        let name = &member.name;
        match classify_member(home, app_data, member) {
            MemberKind::Dotagents {
                source,
                path,
                r#ref,
            } => {
                readme_lines.push(format!(
                    "- `{name}` - managed by dotagents, `{source}`; also bundled under `skills/{name}/`"
                ));
                rows.push(ManifestRow {
                    name: name.clone(),
                    source,
                    path,
                    r#ref,
                });
            }
            MemberKind::SkillsSh { repo, path, r#ref } => {
                readme_lines.push(format!(
                    "- `{name}` - managed by skills.sh, `{repo}`; also bundled under `skills/{name}/`"
                ));
                rows.push(ManifestRow {
                    name: name.clone(),
                    source: repo,
                    path,
                    r#ref,
                });
            }
            MemberKind::Fork {
                repo,
                path,
                base_commit,
            } => {
                readme_lines.push(format!(
                    "- `{name}` - a fork of `{repo}`; the edited copy is bundled under `skills/{name}/`, and the original is referenced at `{repo}` (commit `{base_commit}`)"
                ));
                rows.push(ManifestRow {
                    name: name.clone(),
                    source: repo,
                    path,
                    r#ref: Some(base_commit),
                });
            }
            MemberKind::Manual => {
                readme_lines.push(format!(
                    "- `{name}` - bundled copy (not managed by dotagents or skills.sh)"
                ));
            }
        }
        bundle_skill(&member.path, &skills_root, name)?;
    }

    let manifest = AgentsManifestOut { skills: rows };
    let toml_text = toml::to_string_pretty(&manifest)
        .map_err(|e| format!("Failed to serialize agents.toml: {e}"))?;
    fs::write(dir.join("agents.toml"), toml_text)
        .map_err(|e| format!("Failed to write agents.toml: {e}"))?;

    let readme = format!(
        "# {pack_name}\n\n{} skill{}:\n\n{}\n\n## Install\n\nWith dotagents (installs every `[[skills]]` row above, plus the bundled skills under `skills/`):\n\n```\nnpx -y @sentry/dotagents add <owner>/<repo> --all\n```\n\nOr with `npx skills`:\n\n```\nnpx skills add <owner>/<repo>\n```\n",
        members.len(),
        if members.len() == 1 { "" } else { "s" },
        readme_lines.join("\n"),
    );
    fs::write(dir.join("README.md"), readme)
        .map_err(|e| format!("Failed to write README.md: {e}"))?;

    Ok(())
}

// ============================================================================
// Traits - the real implementation shells out; tests use fakes.
// ============================================================================

/// Runs `git` in `cwd`.
pub trait GitRunner {
    fn run(&self, cwd: &Path, args: &[&str]) -> Result<String, String>;
}

pub struct RealGitRunner;

impl GitRunner for RealGitRunner {
    fn run(&self, cwd: &Path, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("Failed to run git: {e}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Creates (and pushes to) a new GitHub repo for a pack - the only place
/// `gh repo create` runs, always from `publish_skill_pack` behind its
/// confirm dialog.
pub trait GhRepoCreate {
    /// `visibility` is `"private"` or `"public"`. Returns `"owner/repo"` on
    /// success.
    fn create(&self, dir: &Path, name: &str, visibility: &str) -> Result<String, GhError>;
}

pub struct RealGhRepoCreate {
    pub gh_bin: PathBuf,
}

impl GhRepoCreate for RealGhRepoCreate {
    fn create(&self, dir: &Path, name: &str, visibility: &str) -> Result<String, GhError> {
        let flag = format!("--{visibility}");
        let dir_str = dir.to_string_lossy().to_string();
        run_gh(
            &self.gh_bin,
            &[
                "repo", "create", name, &flag, "--source", &dir_str, "--remote", "origin", "--push",
            ],
            None,
        )?;
        let login = run_gh(&self.gh_bin, &["api", "user", "--jq", ".login"], None)?;
        let login = String::from_utf8_lossy(&login).trim().to_string();
        Ok(format!("{login}/{name}"))
    }
}

/// Reads a repo's `agents.toml`, read-only.
pub trait GhContentsFetch {
    /// `Ok(None)` when the repo has no `agents.toml` (a 404, not an error -
    /// most repos this imports are plain multi-skill repos with no
    /// manifest at all).
    fn fetch_agents_toml(&self, owner_repo: &str) -> Result<Option<String>, String>;
}

pub struct RealGhContentsFetch {
    pub gh_bin: PathBuf,
}

impl GhContentsFetch for RealGhContentsFetch {
    fn fetch_agents_toml(&self, owner_repo: &str) -> Result<Option<String>, String> {
        let api_path = format!("repos/{owner_repo}/contents/agents.toml");
        match run_gh(
            &self.gh_bin,
            &["api", "-H", "Accept: application/vnd.github.raw", &api_path],
            None,
        ) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).to_string())),
            Err(GhError::Failed(message)) if message.contains("404") => Ok(None),
            Err(e) => Err(e.message()),
        }
    }
}

/// The publish confirm dialog, injectable so tests never pop a real one. The
/// real implementation is a native `tauri_plugin_dialog` message box, run
/// from `publish_skill_pack` right before any `gh`/`git` call.
pub(crate) trait PublishConfirm {
    fn confirm(&self, message: &str) -> bool;
}

/// `app.dialog().message(...).blocking_show()` - `publish_skill_pack` stays a
/// sync Tauri command, which Tauri already runs off the main thread, so
/// blocking here doesn't freeze the UI.
pub(crate) struct RealPublishConfirm<'a> {
    pub app: &'a tauri::AppHandle,
}

impl PublishConfirm for RealPublishConfirm<'_> {
    fn confirm(&self, message: &str) -> bool {
        self.app
            .dialog()
            .message(message)
            .title("Publish pack to GitHub")
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Publish".to_string(),
                "Cancel".to_string(),
            ))
            .blocking_show()
    }
}

// ============================================================================
// Commands' testable cores
// ============================================================================

/// `create_skill_pack`'s core: resolves and validates every member, builds
/// the tree, `git init` + `add` + commit locally (no remote -
/// `publish_skill_pack` adds one later), then records the pack in the
/// registry last.
pub(crate) fn create_skill_pack_with(
    home: &Path,
    app_data: &Path,
    name: &str,
    members: &[PackMemberInput],
    git: &dyn GitRunner,
) -> Result<PackInfo, String> {
    validate_pack_name(name)?;
    if members.is_empty() {
        return Err("A pack needs at least one skill".to_string());
    }
    let members = resolve_members(members)?;
    let mut registry = skill_fork_registry::read_fork_registry(home)?;
    if registry.packs.contains_key(name) {
        return Err(format!("Pack {name:?} already exists"));
    }
    let dir = pack_dir(home, name);
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }

    build_pack_tree(home, app_data, &dir, name, &members)?;
    git.run(&dir, &["init"])?;
    git.run(&dir, &["add", "-A"])?;
    git.run(&dir, &["commit", "-m", &format!("Create pack {name}")])?;

    let record = PackRecord {
        created_at: chrono::Utc::now().to_rfc3339(),
        dir,
        repo: None,
        members,
        skills: Vec::new(),
    };
    registry.packs.insert(name.to_string(), record.clone());
    skill_fork_registry::write_fork_registry(home, &registry)?;

    Ok(PackInfo::from_record(home, name, &record))
}

/// `update_skill_pack`'s core: rebuilds the tree from the pack's own
/// recorded members and commits only when the rebuild actually changed
/// something (`git status --porcelain` non-empty).
pub(crate) fn update_skill_pack_with(
    home: &Path,
    app_data: &Path,
    name: &str,
    git: &dyn GitRunner,
) -> Result<UpdatePackResult, String> {
    validate_pack_name(name)?;
    let registry = skill_fork_registry::read_fork_registry(home)?;
    let record = registry
        .packs
        .get(name)
        .ok_or_else(|| format!("Pack {name:?} not found"))?
        .clone();
    require_pack_dir(home, name, &record.dir)?;
    let members = record_members(home, &record);

    build_pack_tree(home, app_data, &record.dir, name, &members)?;
    let status = git.run(&record.dir, &["status", "--porcelain"])?;
    let changed = !status.trim().is_empty();
    if changed {
        git.run(&record.dir, &["add", "-A"])?;
        git.run(
            &record.dir,
            &["commit", "-m", &format!("Update pack {name}")],
        )?;
    }

    Ok(UpdatePackResult {
        changed,
        pack: PackInfo::from_record(home, name, &record),
    })
}

/// `publish_skill_pack`'s core - confirms with `confirm` right before any
/// `gh`/`git` call. Creates the GitHub repo (and pushes) the first time,
/// records `repo` only once `gh repo create` actually succeeds; a later call
/// just pushes.
pub(crate) fn publish_skill_pack_with(
    home: &Path,
    name: &str,
    visibility: &str,
    git: &dyn GitRunner,
    gh: &dyn GhRepoCreate,
    confirm: &dyn PublishConfirm,
) -> Result<PackInfo, String> {
    validate_pack_name(name)?;
    if visibility != "private" && visibility != "public" {
        return Err(format!("Invalid visibility: {visibility:?}"));
    }
    let mut registry = skill_fork_registry::read_fork_registry(home)?;
    let record = registry
        .packs
        .get(name)
        .ok_or_else(|| format!("Pack {name:?} not found"))?
        .clone();
    require_pack_dir(home, name, &record.dir)?;

    let message = match &record.repo {
        Some(repo) => format!("Push ~/.agents/packs/{name} to {repo}?"),
        None => format!(
            "Create GitHub repository {name} ({visibility}) from ~/.agents/packs/{name} and push?"
        ),
    };
    if !confirm.confirm(&message) {
        return Err("Publish cancelled".to_string());
    }

    if record.repo.is_some() {
        git.run(&record.dir, &["push", "origin", "HEAD"])?;
        return Ok(PackInfo::from_record(home, name, &record));
    }

    let owner_repo = gh
        .create(&record.dir, name, visibility)
        .map_err(|e| e.message())?;

    let mut updated = record;
    updated.repo = Some(owner_repo);
    registry.packs.insert(name.to_string(), updated.clone());
    skill_fork_registry::write_fork_registry(home, &registry)?;

    Ok(PackInfo::from_record(home, name, &updated))
}

/// `delete_skill_pack`'s core - local only, never touches GitHub even when
/// `repo` is set.
pub(crate) fn delete_skill_pack_with(home: &Path, name: &str) -> Result<(), String> {
    validate_pack_name(name)?;
    let mut registry = skill_fork_registry::read_fork_registry(home)?;
    let record = registry
        .packs
        .get(name)
        .ok_or_else(|| format!("Pack {name:?} not found"))?
        .clone();
    require_pack_dir(home, name, &record.dir)?;

    if record.dir.exists() {
        assert_removable_inside_packs_root(home, &record.dir)?;
        fs::remove_dir_all(&record.dir)
            .map_err(|e| format!("Failed to remove {}: {e}", record.dir.display()))?;
    }
    registry.packs.remove(name);
    skill_fork_registry::write_fork_registry(home, &registry)
}

/// One `agents.toml` `[[skills]]` row, as read back from an imported repo -
/// a smaller shape than `dotagents_ledger`'s (this side never needs
/// `has_manifest_row`).
#[derive(Debug, Deserialize, Default)]
struct ImportManifest {
    #[serde(default)]
    skills: Vec<ImportRow>,
}

#[derive(Debug, Deserialize)]
struct ImportRow {
    name: String,
    source: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    r#ref: Option<String>,
}

/// Validates one imported `agents.toml` `[[skills]]` row before any install
/// command runs for it - the manifest comes from a remote repo, so every
/// field that could shape a filesystem path or a `gh`/`npx` argv gets the
/// same scrutiny as any other IPC-boundary input (F1).
fn validate_pack_manifest_row(row: &ImportRow) -> Result<(), String> {
    validate_skill_dir_name(&row.name)?;
    validate_pack_manifest_source(&row.source)?;
    if let Some(path) = &row.path {
        validate_pack_manifest_path(path)?;
    }
    if let Some(r#ref) = &row.r#ref {
        validate_pack_manifest_ref(r#ref)?;
    }
    Ok(())
}

/// `^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$`, no leading `-` on either segment, no
/// `..` segment.
fn validate_pack_manifest_source(source: &str) -> Result<(), String> {
    let invalid = || format!("Invalid source: {source:?}");
    let Some((owner, repo)) = source.split_once('/') else {
        return Err(invalid());
    };
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(invalid());
    }
    if owner.starts_with('-') || repo.starts_with('-') || owner == ".." || repo == ".." {
        return Err(invalid());
    }
    let valid_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-');
    if !owner.chars().all(valid_char) || !repo.chars().all(valid_char) {
        return Err(invalid());
    }
    Ok(())
}

/// Relative, no leading `/` or `-`, no `..` segment, no backslash, chars
/// `[A-Za-z0-9_./-]`.
fn validate_pack_manifest_path(path: &str) -> Result<(), String> {
    let invalid = || format!("Invalid path: {path:?}");
    if path.starts_with('/') || path.starts_with('-') || path.contains('\\') {
        return Err(invalid());
    }
    if Path::new(path)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(invalid());
    }
    let valid_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-');
    if !path.chars().all(valid_char) {
        return Err(invalid());
    }
    Ok(())
}

/// `^[A-Za-z0-9_./-]{1,128}$`, no leading `-`, no `..`.
fn validate_pack_manifest_ref(r#ref: &str) -> Result<(), String> {
    let invalid = || format!("Invalid ref: {ref:?}");
    if r#ref.is_empty() || r#ref.len() > 128 {
        return Err(invalid());
    }
    if r#ref.starts_with('-') || r#ref.contains("..") {
        return Err(invalid());
    }
    let valid_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-');
    if !r#ref.chars().all(valid_char) {
        return Err(invalid());
    }
    Ok(())
}

/// Above this, an import is refused outright rather than validated row by
/// row - a manifest this large is itself a sign something's wrong.
const MAX_MANIFEST_ROWS: usize = 200;

fn dir_entry_names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// `import_skill_pack`'s core: validates every `agents.toml` row before
/// running any command (F1), then `--all` for whatever the repo bundles
/// under `skills/`, then one `dotagents add <row.source> --name <row.name>
/// [--ref]` per remaining `agents.toml` row - skipping any row whose name
/// `--all` already bundled (F5), since that row is just provenance for a
/// name the wildcard install already covers. A repo without `agents.toml` is
/// just a multi-skill repo, so `--all` is the whole job.
pub(crate) fn import_skill_pack_with(
    home: &Path,
    source: &str,
    agents: &[AgentId],
    gh: &dyn GhContentsFetch,
    runner: &dyn CommandRunner,
) -> Result<ImportResult, String> {
    let shared_dir = shared_skills_dir(home);
    let claude_dir = home.join(".claude").join("skills");
    let mut result = ImportResult::default();

    let manifest_toml = gh.fetch_agents_toml(source)?;
    let manifest: ImportManifest = match &manifest_toml {
        Some(toml_text) => {
            toml::from_str(toml_text).map_err(|e| format!("Failed to parse agents.toml: {e}"))?
        }
        None => ImportManifest::default(),
    };

    if manifest.skills.len() > MAX_MANIFEST_ROWS {
        return Err("Pack manifest has too many skills".to_string());
    }
    for row in &manifest.skills {
        validate_pack_manifest_row(row).map_err(|e| format!("{}: {e}", row.name))?;
    }

    let all_args = vec![
        "-y".to_string(),
        "@sentry/dotagents".to_string(),
        "add".to_string(),
        source.to_string(),
        "--all".to_string(),
    ];
    let before = dir_entry_names(&shared_dir);
    runner.run_npx(&all_args, None)?;
    let after = dir_entry_names(&shared_dir);
    let mut bundled: Vec<String> = after.difference(&before).cloned().collect();
    bundled.sort();
    for name in &bundled {
        if let Err(e) = maybe_claude_code_symlink(&claude_dir, &shared_dir, name, agents) {
            result.errors.push(format!("{name}: {e}"));
        }
    }
    let bundled_names: BTreeSet<String> = bundled.iter().cloned().collect();
    result.bundled = bundled;

    for row in manifest.skills {
        if bundled_names.contains(&row.name) {
            continue;
        }
        let args = dotagents_add_args(&row.source, &row.name, row.r#ref.as_deref());
        match runner.run_npx(&args, None) {
            Ok(()) => {
                if let Err(e) =
                    maybe_claude_code_symlink(&claude_dir, &shared_dir, &row.name, agents)
                {
                    result.errors.push(format!("{}: {e}", row.name));
                }
                result.referenced.push(row.name);
            }
            Err(e) => result.errors.push(format!("{}: {e}", row.name)),
        }
    }

    Ok(result)
}

// ============================================================================
// Tauri commands
// ============================================================================

#[tauri::command]
pub fn create_skill_pack(
    name: String,
    members: Vec<PackMemberInput>,
    app: tauri::AppHandle,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<PackInfo, String> {
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not resolve app data dir: {e}"))?;
    create_skill_pack_with(&home, &app_data, &name, &members, &RealGitRunner)
}

#[tauri::command]
pub fn update_skill_pack(
    name: String,
    app: tauri::AppHandle,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<UpdatePackResult, String> {
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not resolve app data dir: {e}"))?;
    update_skill_pack_with(&home, &app_data, &name, &RealGitRunner)
}

#[tauri::command]
pub fn publish_skill_pack(
    name: String,
    visibility: String,
    app: tauri::AppHandle,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<PackInfo, String> {
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let gh_bin =
        skill_update_check::resolve_gh_binary().ok_or_else(|| "gh is not installed".to_string())?;
    publish_skill_pack_with(
        &home,
        &name,
        &visibility,
        &RealGitRunner,
        &RealGhRepoCreate { gh_bin },
        &RealPublishConfirm { app: &app },
    )
}

#[tauri::command]
pub fn delete_skill_pack(
    name: String,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    delete_skill_pack_with(&home, &name)
}

#[tauri::command]
pub fn import_skill_pack(
    source: String,
    agents: Vec<AgentId>,
    app: tauri::AppHandle,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<ImportResult, String> {
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let gh_bin =
        skill_update_check::resolve_gh_binary().ok_or_else(|| "gh is not installed".to_string())?;
    let result = import_skill_pack_with(
        &home,
        &source,
        &agents,
        &RealGhContentsFetch { gh_bin },
        &RealCommandRunner,
    )?;
    skill_refresh::request_snapshot_rebuild(&app);
    Ok(result)
}

/// Read-only: the Packs view's list, straight off the registry - not part of
/// `SkillSnapshot` since packs aren't installed skills.
#[tauri::command]
pub fn list_skill_packs() -> Result<Vec<PackInfo>, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let registry = skill_fork_registry::read_fork_registry_or_default(&home);
    Ok(registry
        .packs
        .iter()
        .map(|(name, record)| PackInfo::from_record(&home, name, record))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn write_shared_skill(home: &Path, name: &str) {
        let dir = shared_skills_dir(home).join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), format!("# {name}\n")).unwrap();
    }

    /// A member pointing at that name's own copy under the shared skills
    /// root - the common case in these tests, where provenance lookup
    /// (`classify_member`) applies.
    fn shared_member(home: &Path, name: &str) -> PackMemberInput {
        write_shared_skill_if_missing(home, name);
        PackMemberInput {
            name: name.to_string(),
            path: shared_skills_dir(home)
                .join(name)
                .to_string_lossy()
                .to_string(),
        }
    }

    fn write_shared_skill_if_missing(home: &Path, name: &str) {
        if !shared_skills_dir(home).join(name).join("SKILL.md").exists() {
            write_shared_skill(home, name);
        }
    }

    /// Records every call instead of touching real git state; each `run`
    /// call for `"status", "--porcelain"]` returns `porcelain_output`.
    struct FakeGit {
        calls: Mutex<Vec<Vec<String>>>,
        porcelain_output: String,
    }

    impl FakeGit {
        fn new(porcelain_output: &str) -> Self {
            FakeGit {
                calls: Mutex::new(Vec::new()),
                porcelain_output: porcelain_output.to_string(),
            }
        }
    }

    impl GitRunner for FakeGit {
        fn run(&self, _cwd: &Path, args: &[&str]) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|s| s.to_string()).collect());
            if args == ["status", "--porcelain"] {
                Ok(self.porcelain_output.clone())
            } else {
                Ok(String::new())
            }
        }
    }

    /// Also counts calls, so a test can assert `gh repo create` never ran
    /// (e.g. because `PublishConfirm` returned false).
    struct FakeGhRepoCreate {
        result: Result<String, GhError>,
        calls: Mutex<u32>,
    }

    impl FakeGhRepoCreate {
        fn new(result: Result<String, GhError>) -> Self {
            FakeGhRepoCreate {
                result,
                calls: Mutex::new(0),
            }
        }
    }

    impl GhRepoCreate for FakeGhRepoCreate {
        fn create(&self, _dir: &Path, _name: &str, _visibility: &str) -> Result<String, GhError> {
            *self.calls.lock().unwrap() += 1;
            self.result.clone()
        }
    }

    /// Stands in for the native `PublishConfirm` dialog.
    struct FakeConfirm {
        result: bool,
    }

    impl PublishConfirm for FakeConfirm {
        fn confirm(&self, _message: &str) -> bool {
            self.result
        }
    }

    struct FakeGhContents {
        toml: Option<String>,
    }

    impl GhContentsFetch for FakeGhContents {
        fn fetch_agents_toml(&self, _owner_repo: &str) -> Result<Option<String>, String> {
            Ok(self.toml.clone())
        }
    }

    struct FakeRunner {
        calls: Mutex<Vec<Vec<String>>>,
        /// Skill names to create under the shared dir on the `--all` call.
        home: PathBuf,
        all_creates: Vec<&'static str>,
        fail_sources: Vec<&'static str>,
    }

    impl CommandRunner for FakeRunner {
        fn run_npx(&self, args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
            self.calls.lock().unwrap().push(args.to_vec());
            if args.contains(&"--all".to_string()) {
                for name in &self.all_creates {
                    write_shared_skill(&self.home, name);
                }
                return Ok(());
            }
            if let Some(source) = args.get(3) {
                if self.fail_sources.contains(&source.as_str()) {
                    return Err(format!("failed: {source}"));
                }
            }
            Ok(())
        }
    }

    // ------------------------------------------------------------------
    // validate_pack_name
    // ------------------------------------------------------------------

    #[test]
    fn validate_pack_name_accepts_lowercase_digits_and_dashes() {
        assert!(validate_pack_name("my-skills-2").is_ok());
    }

    #[test]
    fn validate_pack_name_rejects_uppercase_and_spaces() {
        assert!(validate_pack_name("My Skills").is_err());
    }

    #[test]
    fn validate_pack_name_rejects_too_long() {
        let name = "a".repeat(65);
        assert!(validate_pack_name(&name).is_err());
    }

    // ------------------------------------------------------------------
    // create_skill_pack_with
    // ------------------------------------------------------------------

    #[test]
    fn create_builds_dotagents_pinned_row() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents/agents.lock"),
            r#"
[skills.find-bugs]
source = "getsentry/find-bugs"
resolved_path = "skills/find-bugs"
resolved_commit = "1111111111111111111111111111111111aaaa"
"#,
        )
        .unwrap();
        fs::write(
            home.join(".agents/agents.toml"),
            r#"
[[skills]]
name = "find-bugs"
source = "getsentry/find-bugs"
path = "skills/find-bugs"
ref = "1111111111111111111111111111111111aaaa"
"#,
        )
        .unwrap();

        let git = FakeGit::new("");
        let info = create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[shared_member(home, "find-bugs")],
            &git,
        )
        .unwrap();

        let toml_text = fs::read_to_string(Path::new(&info.dir).join("agents.toml")).unwrap();
        assert!(toml_text.contains("name = \"find-bugs\""));
        assert!(toml_text.contains("ref = \"1111111111111111111111111111111111aaaa\""));
        // F5: a dotagents-managed member is now also bundled, in addition to
        // the manifest row that carries its provenance.
        assert!(Path::new(&info.dir)
            .join("skills/find-bugs/SKILL.md")
            .exists());
    }

    #[test]
    fn create_builds_dotagents_unpinned_row() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents/agents.lock"),
            r#"
[skills.find-bugs]
source = "getsentry/find-bugs"
resolved_path = "skills/find-bugs"
"#,
        )
        .unwrap();
        fs::write(
            home.join(".agents/agents.toml"),
            r#"
[[skills]]
name = "find-bugs"
source = "getsentry/find-bugs"
path = "skills/find-bugs"
"#,
        )
        .unwrap();

        let git = FakeGit::new("");
        let info = create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[shared_member(home, "find-bugs")],
            &git,
        )
        .unwrap();

        let toml_text = fs::read_to_string(Path::new(&info.dir).join("agents.toml")).unwrap();
        assert!(!toml_text.contains("ref ="));
    }

    #[test]
    fn create_builds_dotagents_wildcard_row() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents/agents.lock"),
            r#"
[skills.some-wildcard-skill]
source = "getsentry/some-repo"
resolved_path = "skills/some-wildcard-skill"
resolved_commit = "3333333333333333333333333333333333cccc"
"#,
        )
        .unwrap();
        // No agents.toml - a wildcard (`--all`) entry.

        let git = FakeGit::new("");
        let info = create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[shared_member(home, "some-wildcard-skill")],
            &git,
        )
        .unwrap();

        let toml_text = fs::read_to_string(Path::new(&info.dir).join("agents.toml")).unwrap();
        assert!(toml_text.contains("name = \"some-wildcard-skill\""));
        assert!(toml_text.contains("source = \"getsentry/some-repo\""));
        assert!(toml_text.contains("ref = \"3333333333333333333333333333333333cccc\""));
    }

    #[test]
    fn create_builds_skills_sh_row_with_store_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let app_data = tmp.path().join("app-data");
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents/.skill-lock.json"),
            serde_json::json!({
                "version": 3,
                "skills": {
                    "cool-skill": {
                        "source": "someone/cool-skill",
                        "sourceType": "github",
                        "sourceUrl": "https://github.com/someone/cool-skill",
                        "skillPath": "cool-skill/SKILL.md",
                        "skillFolderHash": "abc",
                        "installedAt": "2026-01-01T00:00:00Z",
                        "updatedAt": "2026-01-01T00:00:00Z"
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        fs::create_dir_all(
            skill_update_check::update_check_path(&app_data)
                .parent()
                .unwrap(),
        )
        .unwrap();
        fs::write(
            skill_update_check::update_check_path(&app_data),
            serde_json::json!({
                "version": 2,
                "checked_at": "2026-01-01T00:00:00Z",
                "gh_status": {"kind": "ok"},
                "owners": {
                    "owner:v1/global/cool-skill": {
                        "repo": "someone/cool-skill",
                        "path": "cool-skill",
                        "installed_commit": "4444444444444444444444444444444444dddd",
                        "latest_commit": null,
                        "latest_commit_at": null,
                        "checked_at": "2026-01-01T00:00:00Z",
                        "error": null
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let git = FakeGit::new("");
        let info = create_skill_pack_with(
            home,
            &app_data,
            "my-skills",
            &[shared_member(home, "cool-skill")],
            &git,
        )
        .unwrap();

        let toml_text = fs::read_to_string(Path::new(&info.dir).join("agents.toml")).unwrap();
        assert!(toml_text.contains("ref = \"4444444444444444444444444444444444dddd\""));
    }

    #[test]
    fn create_builds_skills_sh_row_without_store_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents/.skill-lock.json"),
            serde_json::json!({
                "version": 3,
                "skills": {
                    "cool-skill": {
                        "source": "someone/cool-skill",
                        "sourceType": "github",
                        "sourceUrl": "https://github.com/someone/cool-skill",
                        "skillPath": "cool-skill/SKILL.md",
                        "skillFolderHash": "abc",
                        "installedAt": "2026-01-01T00:00:00Z",
                        "updatedAt": "2026-01-01T00:00:00Z"
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        // No update-check store at all this time.

        let git = FakeGit::new("");
        let info = create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[shared_member(home, "cool-skill")],
            &git,
        )
        .unwrap();

        let toml_text = fs::read_to_string(Path::new(&info.dir).join("agents.toml")).unwrap();
        assert!(toml_text.contains("name = \"cool-skill\""));
        assert!(!toml_text.contains("ref ="));
    }

    #[test]
    fn create_builds_fork_row_and_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_shared_skill(home, "my-fork");
        let mut registry = skill_fork_registry::ForkRegistry::default();
        registry.forks.insert(
            "my-fork".to_string(),
            skill_fork_registry::ForkRecord {
                deployment_id: String::new(),
                skill_dir: PathBuf::new(),
                forked_at: "2026-01-01T00:00:00Z".to_string(),
                origin_tool: skill_fork_registry::OriginTool::Dotagents,
                origin_source: "getsentry/my-fork".to_string(),
                repo: "getsentry/my-fork".to_string(),
                path: "skills/my-fork".to_string(),
                declared_ref: None,
                base_commit: "5".repeat(40),
            },
        );
        skill_fork_registry::write_fork_registry(home, &registry).unwrap();

        let git = FakeGit::new("");
        let info = create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[shared_member(home, "my-fork")],
            &git,
        )
        .unwrap();

        let toml_text = fs::read_to_string(Path::new(&info.dir).join("agents.toml")).unwrap();
        assert!(toml_text.contains("source = \"getsentry/my-fork\""));
        assert!(Path::new(&info.dir)
            .join("skills/my-fork/SKILL.md")
            .exists());
    }

    #[test]
    fn create_builds_manual_bundle_only() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_shared_skill(home, "manual-skill");

        let git = FakeGit::new("");
        let info = create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[shared_member(home, "manual-skill")],
            &git,
        )
        .unwrap();

        let toml_text = fs::read_to_string(Path::new(&info.dir).join("agents.toml")).unwrap();
        assert!(!toml_text.contains("[[skills]]"));
        assert!(Path::new(&info.dir)
            .join("skills/manual-skill/SKILL.md")
            .exists());
    }

    #[test]
    fn create_refuses_when_pack_dir_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_shared_skill(home, "some-skill");
        fs::create_dir_all(pack_dir(home, "my-skills")).unwrap();

        let git = FakeGit::new("");
        let err = create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[shared_member(home, "some-skill")],
            &git,
        )
        .unwrap_err();
        assert!(err.contains("already exists"));
    }

    // ------------------------------------------------------------------
    // update_skill_pack_with
    // ------------------------------------------------------------------

    #[test]
    fn update_reports_changed_true_when_tree_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_shared_skill(home, "some-skill");
        let app_data = tmp.path().join("app-data");
        create_skill_pack_with(
            home,
            &app_data,
            "my-skills",
            &[shared_member(home, "some-skill")],
            &FakeGit::new(""),
        )
        .unwrap();

        let git = FakeGit::new(" M agents.toml\n");
        let result = update_skill_pack_with(home, &app_data, "my-skills", &git).unwrap();
        assert!(result.changed);
    }

    #[test]
    fn update_reports_changed_false_when_nothing_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_shared_skill(home, "some-skill");
        let app_data = tmp.path().join("app-data");
        create_skill_pack_with(
            home,
            &app_data,
            "my-skills",
            &[shared_member(home, "some-skill")],
            &FakeGit::new(""),
        )
        .unwrap();

        let git = FakeGit::new("");
        let result = update_skill_pack_with(home, &app_data, "my-skills", &git).unwrap();
        assert!(!result.changed);
    }

    // ------------------------------------------------------------------
    // publish_skill_pack_with
    // ------------------------------------------------------------------

    #[test]
    fn publish_refused_when_gh_not_logged_in() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_shared_skill(home, "some-skill");
        create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[shared_member(home, "some-skill")],
            &FakeGit::new(""),
        )
        .unwrap();

        let gh = FakeGhRepoCreate::new(Err(GhError::NotLoggedIn));
        let confirm = FakeConfirm { result: true };
        let err = publish_skill_pack_with(
            home,
            "my-skills",
            "private",
            &FakeGit::new(""),
            &gh,
            &confirm,
        )
        .unwrap_err();
        assert!(err.contains("gh auth login"));

        let registry = skill_fork_registry::read_fork_registry(home).unwrap();
        assert!(registry.packs["my-skills"].repo.is_none());
    }

    #[test]
    fn publish_records_repo_only_after_success() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_shared_skill(home, "some-skill");
        create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[shared_member(home, "some-skill")],
            &FakeGit::new(""),
        )
        .unwrap();

        let gh = FakeGhRepoCreate::new(Ok("someone/my-skills".to_string()));
        let confirm = FakeConfirm { result: true };
        let info = publish_skill_pack_with(
            home,
            "my-skills",
            "private",
            &FakeGit::new(""),
            &gh,
            &confirm,
        )
        .unwrap();
        assert_eq!(info.repo, Some("someone/my-skills".to_string()));

        let registry = skill_fork_registry::read_fork_registry(home).unwrap();
        assert_eq!(
            registry.packs["my-skills"].repo,
            Some("someone/my-skills".to_string())
        );

        // A second publish, now that `repo` is set, only pushes.
        let git = FakeGit::new("");
        publish_skill_pack_with(home, "my-skills", "private", &git, &gh, &confirm).unwrap();
        assert!(git
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c == &vec!["push".to_string(), "origin".to_string(), "HEAD".to_string()]));
    }

    // ------------------------------------------------------------------
    // delete_skill_pack_with
    // ------------------------------------------------------------------

    #[test]
    fn delete_removes_dir_and_record_without_touching_github() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_shared_skill(home, "some-skill");
        let info = create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[shared_member(home, "some-skill")],
            &FakeGit::new(""),
        )
        .unwrap();

        delete_skill_pack_with(home, "my-skills").unwrap();

        assert!(!Path::new(&info.dir).exists());
        let registry = skill_fork_registry::read_fork_registry(home).unwrap();
        assert!(!registry.packs.contains_key("my-skills"));
    }

    // ------------------------------------------------------------------
    // import_skill_pack_with
    // ------------------------------------------------------------------

    #[test]
    fn import_without_agents_toml_runs_all_only() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(shared_skills_dir(home)).unwrap();

        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: home.to_path_buf(),
            all_creates: vec!["bundled-a", "bundled-b"],
            fail_sources: vec![],
        };
        let gh = FakeGhContents { toml: None };

        let result = import_skill_pack_with(home, "someone/repo", &[], &gh, &runner).unwrap();

        assert_eq!(result.bundled, vec!["bundled-a", "bundled-b"]);
        assert!(result.referenced.is_empty());
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains(&"--all".to_string()));
    }

    #[test]
    fn import_with_agents_toml_runs_per_row_dotagents_add() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(shared_skills_dir(home)).unwrap();

        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: home.to_path_buf(),
            all_creates: vec!["bundled-a"],
            fail_sources: vec![],
        };
        let gh = FakeGhContents {
            toml: Some(
                r#"
[[skills]]
name = "referenced-a"
source = "someone/referenced-a"
ref = "6666666666666666666666666666666666eeee"
"#
                .to_string(),
            ),
        };

        let result = import_skill_pack_with(home, "someone/repo", &[], &gh, &runner).unwrap();

        assert_eq!(result.bundled, vec!["bundled-a"]);
        assert_eq!(result.referenced, vec!["referenced-a"]);
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].contains(&"someone/referenced-a".to_string()));
        assert!(calls[1].contains(&"--name".to_string()));
        assert!(calls[1].contains(&"referenced-a".to_string()));
        assert!(calls[1].contains(&"--ref".to_string()));
        assert!(calls[1].contains(&"6666666666666666666666666666666666eeee".to_string()));
    }

    #[test]
    fn import_reports_per_row_failures_without_aborting() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(shared_skills_dir(home)).unwrap();

        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: home.to_path_buf(),
            all_creates: vec![],
            fail_sources: vec!["someone/broken"],
        };
        let gh = FakeGhContents {
            toml: Some(
                r#"
[[skills]]
name = "broken"
source = "someone/broken"
"#
                .to_string(),
            ),
        };

        let result = import_skill_pack_with(home, "someone/repo", &[], &gh, &runner).unwrap();
        assert!(result.referenced.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("broken"));
    }

    // ------------------------------------------------------------------
    // F1: every imported manifest row is validated before any install runs
    // ------------------------------------------------------------------

    #[test]
    fn import_refuses_traversal_name() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(shared_skills_dir(home)).unwrap();

        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: home.to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let gh = FakeGhContents {
            toml: Some(
                r#"
[[skills]]
name = "../../.ssh/new-link"
source = "someone/pkg"
"#
                .to_string(),
            ),
        };

        let err = import_skill_pack_with(home, "someone/repo", &[], &gh, &runner).unwrap_err();
        assert!(err.contains("../../.ssh/new-link"));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn import_refuses_leading_dash_source() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(shared_skills_dir(home)).unwrap();

        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: home.to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let gh = FakeGhContents {
            toml: Some(
                r#"
[[skills]]
name = "ok-name"
source = "--upload-pack=x"
"#
                .to_string(),
            ),
        };

        let err = import_skill_pack_with(home, "someone/repo", &[], &gh, &runner).unwrap_err();
        assert!(err.contains("ok-name"));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn import_refuses_leading_dash_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(shared_skills_dir(home)).unwrap();

        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: home.to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let gh = FakeGhContents {
            toml: Some(
                r#"
[[skills]]
name = "ok-name"
source = "someone/pkg"
ref = "-x"
"#
                .to_string(),
            ),
        };

        let err = import_skill_pack_with(home, "someone/repo", &[], &gh, &runner).unwrap_err();
        assert!(err.contains("ok-name"));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn import_refuses_dotdot_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(shared_skills_dir(home)).unwrap();

        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: home.to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let gh = FakeGhContents {
            toml: Some(
                r#"
[[skills]]
name = "ok-name"
source = "someone/pkg"
path = "../escape"
"#
                .to_string(),
            ),
        };

        let err = import_skill_pack_with(home, "someone/repo", &[], &gh, &runner).unwrap_err();
        assert!(err.contains("ok-name"));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn import_refuses_manifest_over_row_bound() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(shared_skills_dir(home)).unwrap();

        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: home.to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let mut toml_text = String::new();
        for i in 0..201 {
            toml_text.push_str(&format!(
                "\n[[skills]]\nname = \"skill-{i}\"\nsource = \"someone/skill-{i}\"\n"
            ));
        }
        let gh = FakeGhContents {
            toml: Some(toml_text),
        };

        let err = import_skill_pack_with(home, "someone/repo", &[], &gh, &runner).unwrap_err();
        assert_eq!(err, "Pack manifest has too many skills");
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    // ------------------------------------------------------------------
    // F2: pack directories are derived, never trusted
    // ------------------------------------------------------------------

    #[test]
    fn validate_pack_name_rejects_leading_dash_and_traversal() {
        assert!(validate_pack_name("-my-skills").is_err());
        assert!(validate_pack_name("my/skills").is_err());
        assert!(validate_pack_name("..").is_err());
    }

    /// Plants a `PackRecord` whose `dir` points outside `~/.agents/packs` -
    /// the only way that can happen is a hand-edited
    /// `~/.agents/skill-studio.json`.
    fn write_pack_record_outside_packs_root(home: &Path, outside_dir: &Path) {
        let mut registry = skill_fork_registry::ForkRegistry::default();
        registry.packs.insert(
            "my-skills".to_string(),
            PackRecord {
                created_at: "2026-01-01T00:00:00Z".to_string(),
                dir: outside_dir.to_path_buf(),
                repo: None,
                members: Vec::new(),
                skills: Vec::new(),
            },
        );
        skill_fork_registry::write_fork_registry(home, &registry).unwrap();
    }

    #[test]
    fn delete_refuses_when_record_dir_outside_packs_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("marker.txt"), "x").unwrap();
        write_pack_record_outside_packs_root(home, outside.path());

        let err = delete_skill_pack_with(home, "my-skills").unwrap_err();
        assert!(err.contains("points outside"));
        assert!(outside.path().join("marker.txt").exists());
    }

    #[test]
    fn update_refuses_when_record_dir_outside_packs_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("marker.txt"), "x").unwrap();
        write_pack_record_outside_packs_root(home, outside.path());

        let err = update_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &FakeGit::new(""),
        )
        .unwrap_err();
        assert!(err.contains("points outside"));
        assert!(outside.path().join("marker.txt").exists());
    }

    #[test]
    fn publish_refuses_when_record_dir_outside_packs_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("marker.txt"), "x").unwrap();
        write_pack_record_outside_packs_root(home, outside.path());

        let gh = FakeGhRepoCreate::new(Ok("someone/my-skills".to_string()));
        let confirm = FakeConfirm { result: true };
        let err = publish_skill_pack_with(
            home,
            "my-skills",
            "private",
            &FakeGit::new(""),
            &gh,
            &confirm,
        )
        .unwrap_err();
        assert!(err.contains("points outside"));
        assert!(outside.path().join("marker.txt").exists());
        assert_eq!(*gh.calls.lock().unwrap(), 0);
    }

    // ------------------------------------------------------------------
    // F3: publish confirmation at the backend boundary
    // ------------------------------------------------------------------

    #[test]
    fn publish_cancelled_when_confirm_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[shared_member(home, "some-skill")],
            &FakeGit::new(""),
        )
        .unwrap();

        let gh = FakeGhRepoCreate::new(Ok("someone/my-skills".to_string()));
        let confirm = FakeConfirm { result: false };
        let err = publish_skill_pack_with(
            home,
            "my-skills",
            "private",
            &FakeGit::new(""),
            &gh,
            &confirm,
        )
        .unwrap_err();
        assert_eq!(err, "Publish cancelled");
        assert_eq!(*gh.calls.lock().unwrap(), 0);

        let registry = skill_fork_registry::read_fork_registry(home).unwrap();
        assert!(registry.packs["my-skills"].repo.is_none());
    }

    // ------------------------------------------------------------------
    // F4: selection carries the deployment identity
    // ------------------------------------------------------------------

    #[test]
    fn create_bundles_project_only_skill_from_its_own_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let project = tempfile::tempdir().unwrap();
        let project_skill_dir = project.path().join(".claude/skills/project-skill");
        fs::create_dir_all(&project_skill_dir).unwrap();
        fs::write(project_skill_dir.join("SKILL.md"), "# project-skill\n").unwrap();

        let member = PackMemberInput {
            name: "project-skill".to_string(),
            path: project_skill_dir.to_string_lossy().to_string(),
        };
        let info = create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[member],
            &FakeGit::new(""),
        )
        .unwrap();

        assert!(Path::new(&info.dir)
            .join("skills/project-skill/SKILL.md")
            .exists());
        // Not one of dotagents'/skills.sh's own managed folders, so it's
        // bundle-only - no `agents.toml` row.
        let toml_text = fs::read_to_string(Path::new(&info.dir).join("agents.toml")).unwrap();
        assert!(!toml_text.contains("[[skills]]"));
    }

    #[test]
    fn create_refuses_member_whose_path_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        let member = PackMemberInput {
            name: "missing-skill".to_string(),
            path: tmp
                .path()
                .join("nowhere/missing-skill")
                .to_string_lossy()
                .to_string(),
        };
        let err = create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &[member],
            &FakeGit::new(""),
        )
        .unwrap_err();
        assert!(err.contains("missing-skill"));
    }

    #[test]
    fn create_refuses_duplicate_member_names() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        write_shared_skill(home, "dup-skill");
        let project = tempfile::tempdir().unwrap();
        let project_skill_dir = project.path().join("dup-skill");
        fs::create_dir_all(&project_skill_dir).unwrap();
        fs::write(project_skill_dir.join("SKILL.md"), "# dup-skill\n").unwrap();

        let members = vec![
            shared_member(home, "dup-skill"),
            PackMemberInput {
                name: "dup-skill".to_string(),
                path: project_skill_dir.to_string_lossy().to_string(),
            },
        ];
        let err = create_skill_pack_with(
            home,
            &tmp.path().join("app-data"),
            "my-skills",
            &members,
            &FakeGit::new(""),
        )
        .unwrap_err();
        assert!(err.contains("duplicate skill name"));
    }

    // ------------------------------------------------------------------
    // F5: every member is bundled
    // ------------------------------------------------------------------

    #[test]
    fn create_bundles_both_dotagents_and_skills_sh_members() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let app_data = tmp.path().join("app-data");
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents/agents.lock"),
            r#"
[skills.find-bugs]
source = "getsentry/find-bugs"
resolved_path = "skills/find-bugs"
resolved_commit = "1111111111111111111111111111111111aaaa"
"#,
        )
        .unwrap();
        fs::write(
            home.join(".agents/agents.toml"),
            r#"
[[skills]]
name = "find-bugs"
source = "getsentry/find-bugs"
path = "skills/find-bugs"
ref = "1111111111111111111111111111111111aaaa"
"#,
        )
        .unwrap();
        fs::write(
            home.join(".agents/.skill-lock.json"),
            serde_json::json!({
                "version": 3,
                "skills": {
                    "cool-skill": {
                        "source": "someone/cool-skill",
                        "sourceType": "github",
                        "sourceUrl": "https://github.com/someone/cool-skill",
                        "skillPath": "cool-skill/SKILL.md",
                        "skillFolderHash": "abc",
                        "installedAt": "2026-01-01T00:00:00Z",
                        "updatedAt": "2026-01-01T00:00:00Z"
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let members = vec![
            shared_member(home, "find-bugs"),
            shared_member(home, "cool-skill"),
        ];
        let info =
            create_skill_pack_with(home, &app_data, "my-skills", &members, &FakeGit::new(""))
                .unwrap();

        assert!(Path::new(&info.dir)
            .join("skills/find-bugs/SKILL.md")
            .exists());
        assert!(Path::new(&info.dir)
            .join("skills/cool-skill/SKILL.md")
            .exists());
        let toml_text = fs::read_to_string(Path::new(&info.dir).join("agents.toml")).unwrap();
        assert!(toml_text.contains("name = \"find-bugs\""));
        assert!(toml_text.contains("name = \"cool-skill\""));
    }

    #[test]
    fn import_skips_per_row_command_for_a_name_all_already_bundled() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(shared_skills_dir(home)).unwrap();

        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: home.to_path_buf(),
            all_creates: vec!["bundled-a"],
            fail_sources: vec![],
        };
        let gh = FakeGhContents {
            toml: Some(
                r#"
[[skills]]
name = "bundled-a"
source = "someone/bundled-a"
"#
                .to_string(),
            ),
        };

        let result = import_skill_pack_with(home, "someone/repo", &[], &gh, &runner).unwrap();

        assert_eq!(result.bundled, vec!["bundled-a"]);
        assert!(result.referenced.is_empty());
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains(&"--all".to_string()));
    }
}
