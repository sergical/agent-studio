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
// read side: given "owner/repo", it resolves one commit, reads that commit's
// `agents.toml`, and pins the pack install to the same commit. Local imports
// validate and install an app-owned snapshot.
//
// GitHub rule: this module never creates a repo or pushes except from
// `publish_skill_pack`, which itself confirms with the user through
// `PublishConfirm` (a native `tauri_plugin_dialog` message box) right before
// any `gh`/`git` call - the frontend no longer confirms this one itself (see
// `SkillDetailActions`'s Un-fork dialog for the pattern still used
// elsewhere). `delete_skill_pack` never touches GitHub at all.
// ============================================================================

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use ulid::Ulid;

use super::agents::AgentId;
use super::commands::dotagents_add_args;
use super::dotagents_ledger;
use super::gh_cli::{run_gh, GhError};
use super::lock_file;
use super::skill_add::{maybe_claude_code_symlink, CommandRunner, RealCommandRunner};
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_deployment::SkillDestination;
use super::skill_dto::InstallScope;
use super::skill_fork::ForkMutationLock;
use super::skill_fork_registry::{self, PackMember, PackRecord};
use super::skill_fs::{copy_dir_all, copy_dir_preserving_symlinks};
use super::skill_refresh;
use super::skill_trust_policy::{
    normalize_confirmation_identity, record_trusted_dotagents_sources,
    require_trusted_dotagents_identity,
};
use super::skill_update_check;

// ============================================================================
// DTOs
// ============================================================================

/// One skill pack, as sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdatePackResult {
    pub changed: bool,
    pub pack: PackInfo,
}

/// Result of `import_skill_pack`: which names came from the repo's own
/// `skills/` tree (`--all`) versus a `[[skills]]` row pointing elsewhere,
/// and any per-row failures (a partial import still reports what worked).
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ImportResult {
    pub bundled: Vec<String>,
    pub referenced: Vec<String>,
    pub errors: Vec<String>,
}

/// The complete pack import request. Trust confirmation must repeat this
/// value so a token cannot authorize a changed target or source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PackImportRequest {
    pub source: String,
    pub agents: Vec<AgentId>,
    pub method: String,
    pub destination: SkillDestination,
    pub scope: InstallScope,
    #[serde(default)]
    pub project_path: Option<String>,
}

/// Pack import either completes immediately or pauses for explicit trust.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum PackImportPreflightResult {
    Imported {
        result: ImportResult,
    },
    NeedsTrust {
        identities: Vec<String>,
        confirmation_token: String,
    },
}

struct PreparedPackImport {
    normalized_source: String,
    pinned_ref: Option<String>,
    manifest_text_hash: String,
    manifest: ImportManifest,
    identities: Vec<String>,
    local_snapshot: Option<LocalPackSnapshot>,
}

struct LocalPackSnapshot {
    staging_dir: PathBuf,
    source_dir: PathBuf,
    source_path: PathBuf,
    source_fingerprint: String,
    staging_fingerprint: String,
    ownership: Option<LocalPackSnapshotOwnership>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalPackSnapshotOwnership {
    version: u8,
    token_id: String,
    created_at_unix_seconds: u64,
    source_path: String,
    source_fingerprint: String,
    snapshot_fingerprint: String,
}

struct PendingPackTrust {
    request: PackImportRequest,
    prepared: PreparedPackImport,
    expires_at: Instant,
}

const PACK_TRUST_TOKEN_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_PENDING_PACK_TRUST_TOKENS: usize = 64;
const LOCAL_PACK_OWNERSHIP_FILE: &str = ".skill-studio-pack-import.json";

/// Process-local, bounded one-time confirmations for pack repository trust.
#[derive(Default)]
pub struct PackImportTrustState(Mutex<BTreeMap<String, PendingPackTrust>>);

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

/// Resolves a repository commit and reads `agents.toml` at that commit.
pub trait GhContentsFetch {
    /// Returns the immutable commit SHA currently selected by the repository.
    fn resolve_commit(&self, owner_repo: &str) -> Result<String, String>;

    /// Refuses when GitHub can no longer resolve the recorded commit SHA.
    fn verify_commit(&self, owner_repo: &str, commit: &str) -> Result<(), String>;

    /// `Ok(None)` when the repo has no `agents.toml` (a 404, not an error -
    /// most repos this imports are plain multi-skill repos with no
    /// manifest at all).
    fn fetch_agents_toml(&self, owner_repo: &str, commit: &str) -> Result<Option<String>, String>;
}

pub struct RealGhContentsFetch {
    pub gh_bin: PathBuf,
}

impl GhContentsFetch for RealGhContentsFetch {
    fn resolve_commit(&self, owner_repo: &str) -> Result<String, String> {
        let api_path = format!("repos/{owner_repo}/commits/HEAD");
        let bytes = run_gh(&self.gh_bin, &["api", &api_path, "--jq", ".sha"], None)
            .map_err(|error| error.message())?;
        validate_pack_commit_sha(String::from_utf8_lossy(&bytes).trim())
    }

    fn verify_commit(&self, owner_repo: &str, commit: &str) -> Result<(), String> {
        let commit = validate_pack_commit_sha(commit)?;
        let api_path = format!("repos/{owner_repo}/commits/{commit}");
        run_gh(&self.gh_bin, &["api", &api_path, "--silent"], None)
            .map(|_| ())
            .map_err(|error| error.message())
    }

    fn fetch_agents_toml(&self, owner_repo: &str, commit: &str) -> Result<Option<String>, String> {
        let commit = validate_pack_commit_sha(commit)?;
        let api_path = format!("repos/{owner_repo}/contents/agents.toml?ref={commit}");
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
#[derive(Debug, Clone, Deserialize, Default)]
struct ImportManifest {
    #[serde(default)]
    skills: Vec<ImportRow>,
}

#[derive(Debug, Clone, Deserialize)]
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

fn validate_pack_import_request(request: &PackImportRequest) -> Result<(), String> {
    if request.method != "pack"
        || request.destination != SkillDestination::Universal
        || request.scope != InstallScope::Global
        || request.project_path.is_some()
    {
        return Err(
            "Pack import request must use pack, Universal, global, and no project path".to_string(),
        );
    }
    Ok(())
}

fn parse_pack_manifest(text: Option<&str>) -> Result<ImportManifest, String> {
    let manifest = match text {
        Some(text) => {
            toml::from_str(text).map_err(|error| format!("Failed to parse agents.toml: {error}"))?
        }
        None => ImportManifest::default(),
    };
    if manifest.skills.len() > MAX_MANIFEST_ROWS {
        return Err("Pack manifest has too many skills".to_string());
    }
    for row in &manifest.skills {
        validate_pack_manifest_row(row).map_err(|error| format!("{}: {error}", row.name))?;
    }
    Ok(manifest)
}

fn manifest_text_hash(text: Option<&str>) -> String {
    let mut digest = Sha256::new();
    match text {
        Some(text) => {
            digest.update([1]);
            digest.update(text.as_bytes());
        }
        None => digest.update([0]),
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_pack_commit_sha(commit: &str) -> Result<String, String> {
    if commit.len() != 40
        || !commit
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(format!("Invalid GitHub commit SHA: {commit:?}"));
    }
    Ok(commit.to_ascii_lowercase())
}

fn local_pack_staging_root(home: &Path) -> PathBuf {
    home.join(".agents").join("pack-import-staging")
}

fn unix_timestamp_now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| format!("System clock is before Unix epoch: {error}"))
}

fn local_pack_snapshot_shape_is_owned(staging_dir: &Path) -> bool {
    dir_entry_names(staging_dir)
        == BTreeSet::from(["source".to_string(), LOCAL_PACK_OWNERSHIP_FILE.to_string()])
}

fn read_local_pack_snapshot_ownership(staging_dir: &Path) -> Option<LocalPackSnapshotOwnership> {
    let metadata_path = staging_dir.join(LOCAL_PACK_OWNERSHIP_FILE);
    if fs::symlink_metadata(&metadata_path)
        .ok()
        .is_none_or(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
    {
        return None;
    }
    serde_json::from_slice(&fs::read(metadata_path).ok()?).ok()
}

fn persist_local_pack_snapshot_ownership(
    snapshot: &mut LocalPackSnapshot,
    token_id: &str,
) -> Result<(), String> {
    let ownership = LocalPackSnapshotOwnership {
        version: 1,
        token_id: token_id.to_string(),
        created_at_unix_seconds: unix_timestamp_now()?,
        source_path: snapshot.source_path.to_string_lossy().into_owned(),
        source_fingerprint: snapshot.source_fingerprint.clone(),
        snapshot_fingerprint: super::event_store::fingerprint_path(&snapshot.source_dir),
    };
    let metadata_path = snapshot.staging_dir.join(LOCAL_PACK_OWNERSHIP_FILE);
    let bytes = serde_json::to_vec_pretty(&ownership)
        .map_err(|error| format!("Failed to serialize local pack snapshot ownership: {error}"))?;
    fs::write(&metadata_path, bytes)
        .map_err(|error| format!("Failed to write {}: {error}", metadata_path.display()))?;
    snapshot.staging_fingerprint = super::event_store::fingerprint_path(&snapshot.staging_dir);
    snapshot.ownership = Some(ownership);
    Ok(())
}

fn cleanup_local_pack_snapshot(home: &Path, snapshot: &LocalPackSnapshot) {
    if !snapshot.staging_dir.exists() {
        return;
    }
    let Some(name) = snapshot.staging_dir.file_name() else {
        return;
    };
    let ownership_matches = snapshot.ownership.as_ref().is_none_or(|expected| {
        local_pack_snapshot_shape_is_owned(&snapshot.staging_dir)
            && read_local_pack_snapshot_ownership(&snapshot.staging_dir).as_ref() == Some(expected)
            && super::event_store::fingerprint_path(&snapshot.source_dir)
                == expected.snapshot_fingerprint
    });
    if snapshot.staging_dir.parent() != Some(local_pack_staging_root(home).as_path())
        || name.to_string_lossy().len() != 26
        || !ownership_matches
        || super::event_store::fingerprint_path(&snapshot.staging_dir)
            != snapshot.staging_fingerprint
    {
        return;
    }
    let _ = fs::remove_dir_all(&snapshot.staging_dir);
}

fn cleanup_prepared_pack_import(home: &Path, prepared: &PreparedPackImport) {
    if let Some(snapshot) = &prepared.local_snapshot {
        cleanup_local_pack_snapshot(home, snapshot);
    }
}

fn snapshot_local_pack(home: &Path, source: &str) -> Result<LocalPackSnapshot, String> {
    if source.starts_with('-') {
        return Err(format!("Invalid pack source: {source:?}"));
    }
    let canonical = fs::canonicalize(source)
        .map_err(|error| format!("Failed to resolve local pack {source:?}: {error}"))?;
    if !canonical.is_dir() {
        return Err(format!(
            "Local pack is not a directory: {}",
            canonical.display()
        ));
    }

    let staging_root = local_pack_staging_root(home);
    fs::create_dir_all(&staging_root)
        .map_err(|error| format!("Failed to create {}: {error}", staging_root.display()))?;
    if fs::symlink_metadata(&staging_root)
        .map_err(|error| format!("Failed to stat {}: {error}", staging_root.display()))?
        .file_type()
        .is_symlink()
    {
        return Err(format!(
            "Local pack staging root must not be a symlink: {}",
            staging_root.display()
        ));
    }
    let staging_dir = staging_root.join(Ulid::new().to_string());
    fs::create_dir(&staging_dir)
        .map_err(|error| format!("Failed to create {}: {error}", staging_dir.display()))?;
    let source_dir = staging_dir.join("source");
    let source_fingerprint = super::event_store::fingerprint_path(&canonical);
    if let Err(error) = copy_dir_preserving_symlinks(&canonical, &source_dir) {
        let _ = fs::remove_dir_all(&staging_dir);
        return Err(error);
    }
    let copied_fingerprint = super::event_store::fingerprint_path(&source_dir);
    if source_fingerprint != super::event_store::fingerprint_path(&canonical)
        || source_fingerprint != copied_fingerprint
    {
        let _ = fs::remove_dir_all(&staging_dir);
        return Err("Local pack changed while its import snapshot was created".to_string());
    }
    let staging_fingerprint = super::event_store::fingerprint_path(&staging_dir);
    Ok(LocalPackSnapshot {
        staging_dir,
        source_dir,
        source_path: canonical,
        source_fingerprint,
        staging_fingerprint,
        ownership: None,
    })
}

fn prepare_pack_import(
    home: &Path,
    source: &str,
    gh: &dyn GhContentsFetch,
    pinned_commit: Option<&str>,
) -> Result<PreparedPackImport, String> {
    let (normalized_source, pinned_ref, manifest_text, pack_identity, local_snapshot) =
        if validate_pack_manifest_source(source).is_ok() {
            let normalized = normalize_confirmation_identity(source)?;
            let commit = match pinned_commit {
                Some(commit) => validate_pack_commit_sha(commit)?,
                None => validate_pack_commit_sha(&gh.resolve_commit(&normalized)?)?,
            };
            gh.verify_commit(&normalized, &commit)?;
            let text = gh.fetch_agents_toml(&normalized, &commit)?;
            (
                normalized.clone(),
                Some(commit),
                text,
                Some(normalized),
                None,
            )
        } else {
            let snapshot = snapshot_local_pack(home, source)?;
            let manifest_path = snapshot.source_dir.join("agents.toml");
            let text = if manifest_path.exists() {
                match fs::read_to_string(&manifest_path) {
                    Ok(text) => Some(text),
                    Err(error) => {
                        cleanup_local_pack_snapshot(home, &snapshot);
                        return Err(format!(
                            "Failed to read {}: {error}",
                            manifest_path.display()
                        ));
                    }
                }
            } else {
                None
            };
            (
                snapshot.source_dir.to_string_lossy().to_string(),
                None,
                text,
                None,
                Some(snapshot),
            )
        };

    let manifest = match parse_pack_manifest(manifest_text.as_deref()) {
        Ok(manifest) => manifest,
        Err(error) => {
            if let Some(snapshot) = &local_snapshot {
                cleanup_local_pack_snapshot(home, snapshot);
            }
            return Err(error);
        }
    };
    let mut identities = BTreeSet::new();
    if let Some(identity) = pack_identity {
        identities.insert(identity);
    }
    for row in &manifest.skills {
        identities.insert(normalize_confirmation_identity(&row.source)?);
    }
    Ok(PreparedPackImport {
        normalized_source,
        pinned_ref,
        manifest_text_hash: manifest_text_hash(manifest_text.as_deref()),
        manifest,
        identities: identities.into_iter().collect(),
        local_snapshot,
    })
}

fn all_pack_identities_trusted(home: &Path, identities: &[String]) -> Result<bool, String> {
    for identity in identities {
        match require_trusted_dotagents_identity(home, identity) {
            Ok(()) => {}
            Err(super::skill_trust_policy::DotagentsSourceTrustError::Untrusted { .. }) => {
                return Ok(false);
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(true)
}

fn execute_prepared_pack_import(
    home: &Path,
    prepared: &PreparedPackImport,
    agents: &[AgentId],
    runner: &dyn CommandRunner,
) -> Result<ImportResult, String> {
    let shared_dir = shared_skills_dir(home);
    let claude_dir = home.join(".claude").join("skills");
    let mut result = ImportResult::default();

    let all_args = vec![
        "-y".to_string(),
        "@sentry/dotagents".to_string(),
        "add".to_string(),
        prepared.normalized_source.clone(),
        "--all".to_string(),
    ];
    let all_args = if let Some(commit) = &prepared.pinned_ref {
        let mut pinned = all_args;
        pinned.push("--ref".to_string());
        pinned.push(commit.clone());
        pinned
    } else {
        all_args
    };
    let before = dir_entry_names(&shared_dir);
    runner.run_npx(&all_args, None)?;
    let after = dir_entry_names(&shared_dir);
    let mut bundled: Vec<String> = after.difference(&before).cloned().collect();
    bundled.sort();
    for name in &bundled {
        if let Err(error) = maybe_claude_code_symlink(&claude_dir, &shared_dir, name, agents) {
            result.errors.push(format!("{name}: {error}"));
        }
    }
    let bundled_names: BTreeSet<String> = bundled.iter().cloned().collect();
    result.bundled = bundled;

    for row in &prepared.manifest.skills {
        if bundled_names.contains(&row.name) {
            continue;
        }
        let args = dotagents_add_args(&row.source, &row.name, row.r#ref.as_deref());
        match runner.run_npx(&args, None) {
            Ok(()) => {
                if let Err(error) =
                    maybe_claude_code_symlink(&claude_dir, &shared_dir, &row.name, agents)
                {
                    result.errors.push(format!("{}: {error}", row.name));
                }
                result.referenced.push(row.name.clone());
            }
            Err(error) => result.errors.push(format!("{}: {error}", row.name)),
        }
    }
    Ok(result)
}

fn execute_and_cleanup_pack_import(
    home: &Path,
    prepared: PreparedPackImport,
    agents: &[AgentId],
    runner: &dyn CommandRunner,
) -> Result<ImportResult, String> {
    let result = execute_prepared_pack_import(home, &prepared, agents, runner);
    if let Some(snapshot) = &prepared.local_snapshot {
        cleanup_local_pack_snapshot(home, snapshot);
    }
    result
}

fn revalidate_prepared_pack_import(
    home: &Path,
    source: &str,
    prepared: &PreparedPackImport,
    gh: &dyn GhContentsFetch,
) -> Result<(), String> {
    if let Some(commit) = &prepared.pinned_ref {
        let current = prepare_pack_import(home, source, gh, Some(commit))?;
        if current.normalized_source != prepared.normalized_source
            || current.pinned_ref != prepared.pinned_ref
            || current.manifest_text_hash != prepared.manifest_text_hash
            || current.identities != prepared.identities
        {
            return Err(
                "Pack changed after trust confirmation was requested; review it again".to_string(),
            );
        }
        return Ok(());
    }

    let snapshot = prepared
        .local_snapshot
        .as_ref()
        .ok_or_else(|| "Local pack snapshot is missing".to_string())?;
    if super::event_store::fingerprint_path(&snapshot.source_dir) != snapshot.source_fingerprint
        || super::event_store::fingerprint_path(&snapshot.staging_dir)
            != snapshot.staging_fingerprint
    {
        return Err("Local pack import snapshot changed before installation".to_string());
    }
    let manifest_path = snapshot.source_dir.join("agents.toml");
    let manifest_text = if manifest_path.exists() {
        Some(
            fs::read_to_string(&manifest_path)
                .map_err(|error| format!("Failed to read {}: {error}", manifest_path.display()))?,
        )
    } else {
        None
    };
    let manifest = parse_pack_manifest(manifest_text.as_deref())?;
    let identities = manifest
        .skills
        .iter()
        .map(|row| normalize_confirmation_identity(&row.source))
        .collect::<Result<BTreeSet<_>, _>>()?
        .into_iter()
        .collect::<Vec<_>>();
    if manifest_text_hash(manifest_text.as_deref()) != prepared.manifest_text_hash
        || identities != prepared.identities
    {
        return Err("Local pack import snapshot changed before installation".to_string());
    }
    Ok(())
}

/// `import_skill_pack`'s core: snapshots local packs or pins GitHub packs to
/// one commit, validates every `agents.toml` row before running any command
/// (F1), then `--all` for whatever the repo bundles
/// under `skills/`, then one `dotagents add <row.source> --name <row.name>
/// [--ref]` per remaining `agents.toml` row - skipping any row whose name
/// `--all` already bundled (F5), since that row is just provenance for a
/// name the wildcard install already covers. A repo without `agents.toml` is
/// just a multi-skill repo, so `--all` is the whole job.
#[cfg(test)]
pub(crate) fn import_skill_pack_with(
    home: &Path,
    source: &str,
    agents: &[AgentId],
    gh: &dyn GhContentsFetch,
    runner: &dyn CommandRunner,
) -> Result<ImportResult, String> {
    let prepared = prepare_pack_import(home, source, gh, None)?;
    for identity in &prepared.identities {
        if let Err(error) = require_trusted_dotagents_identity(home, identity) {
            cleanup_prepared_pack_import(home, &prepared);
            return Err(error.to_string());
        }
    }
    if let Err(error) = revalidate_prepared_pack_import(home, source, &prepared, gh) {
        cleanup_prepared_pack_import(home, &prepared);
        return Err(error);
    }
    execute_and_cleanup_pack_import(home, prepared, agents, runner)
}

fn prune_pack_trust_tokens(
    home: &Path,
    tokens: &mut BTreeMap<String, PendingPackTrust>,
    now: Instant,
) {
    let expired = tokens
        .iter()
        .filter(|(_, pending)| pending.expires_at <= now)
        .map(|(token, _)| token.clone())
        .collect::<Vec<_>>();
    for token in expired {
        if let Some(pending) = tokens.remove(&token) {
            if let Some(snapshot) = &pending.prepared.local_snapshot {
                cleanup_local_pack_snapshot(home, snapshot);
            }
        }
    }
    while tokens.len() >= MAX_PENDING_PACK_TRUST_TOKENS {
        let Some(oldest) = tokens
            .iter()
            .min_by_key(|(_, pending)| pending.expires_at)
            .map(|(token, _)| token.clone())
        else {
            break;
        };
        if let Some(pending) = tokens.remove(&oldest) {
            if let Some(snapshot) = &pending.prepared.local_snapshot {
                cleanup_local_pack_snapshot(home, snapshot);
            }
        }
    }
}

fn preflight_pack_import_with(
    home: &Path,
    request: PackImportRequest,
    gh: &dyn GhContentsFetch,
    runner: &dyn CommandRunner,
    state: &PackImportTrustState,
    fork_lock: &ForkMutationLock,
) -> Result<PackImportPreflightResult, String> {
    validate_pack_import_request(&request)?;
    let mut prepared = prepare_pack_import(home, &request.source, gh, None)?;
    let all_trusted = match all_pack_identities_trusted(home, &prepared.identities) {
        Ok(all_trusted) => all_trusted,
        Err(error) => {
            cleanup_prepared_pack_import(home, &prepared);
            return Err(error);
        }
    };
    if all_trusted {
        let _guard = match fork_lock.try_acquire() {
            Ok(guard) => guard,
            Err(error) => {
                cleanup_prepared_pack_import(home, &prepared);
                return Err(error);
            }
        };
        if let Err(error) = revalidate_prepared_pack_import(home, &request.source, &prepared, gh) {
            cleanup_prepared_pack_import(home, &prepared);
            return Err(error);
        }
        let trust_still_valid = match all_pack_identities_trusted(home, &prepared.identities) {
            Ok(valid) => valid,
            Err(error) => {
                cleanup_prepared_pack_import(home, &prepared);
                return Err(error);
            }
        };
        if !trust_still_valid {
            cleanup_prepared_pack_import(home, &prepared);
            return Err("Pack repository trust changed before import".to_string());
        }
        return execute_and_cleanup_pack_import(home, prepared, &request.agents, runner)
            .map(|result| PackImportPreflightResult::Imported { result });
    }

    let identities = prepared.identities.clone();
    let confirmation_token = Ulid::new().to_string();
    if let Some(snapshot) = &mut prepared.local_snapshot {
        if let Err(error) = persist_local_pack_snapshot_ownership(snapshot, &confirmation_token) {
            let _ = fs::remove_dir_all(&snapshot.staging_dir);
            return Err(error);
        }
    }
    let mut tokens = match state.0.lock() {
        Ok(tokens) => tokens,
        Err(_) => {
            cleanup_prepared_pack_import(home, &prepared);
            return Err("Pack trust token state is unavailable".to_string());
        }
    };
    prune_pack_trust_tokens(home, &mut tokens, Instant::now());
    tokens.insert(
        confirmation_token.clone(),
        PendingPackTrust {
            request,
            prepared,
            expires_at: Instant::now() + PACK_TRUST_TOKEN_TTL,
        },
    );
    Ok(PackImportPreflightResult::NeedsTrust {
        identities,
        confirmation_token,
    })
}

fn abandon_pack_import_trust_with(
    home: &Path,
    confirmation_token: &str,
    state: &PackImportTrustState,
) -> Result<bool, String> {
    let pending = {
        let mut tokens = state
            .0
            .lock()
            .map_err(|_| "Pack trust token state is unavailable".to_string())?;
        prune_pack_trust_tokens(home, &mut tokens, Instant::now());
        tokens.remove(confirmation_token)
    };
    if let Some(pending) = pending {
        cleanup_prepared_pack_import(home, &pending.prepared);
        return Ok(true);
    }
    Ok(false)
}

fn reconcile_pack_import_staging_with(home: &Path, now_unix_seconds: u64) -> Result<usize, String> {
    let staging_root = local_pack_staging_root(home);
    let root_metadata = match fs::symlink_metadata(&staging_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(format!(
                "Failed to stat {}: {error}",
                staging_root.display()
            ))
        }
    };
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(format!(
            "Local pack staging root must be a directory, not a symlink: {}",
            staging_root.display()
        ));
    }

    let mut removed = 0;
    for entry in fs::read_dir(&staging_root)
        .map_err(|error| format!("Failed to read {}: {error}", staging_root.display()))?
        .flatten()
    {
        let staging_dir = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_dir() || entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
            continue;
        }
        let Some(ownership) = read_local_pack_snapshot_ownership(&staging_dir) else {
            continue;
        };
        let age = now_unix_seconds.checked_sub(ownership.created_at_unix_seconds);
        if ownership.version != 1
            || Ulid::from_string(&ownership.token_id).is_err()
            || age.is_none_or(|seconds| seconds < PACK_TRUST_TOKEN_TTL.as_secs())
            || !local_pack_snapshot_shape_is_owned(&staging_dir)
            || super::event_store::fingerprint_path(&staging_dir.join("source"))
                != ownership.snapshot_fingerprint
            || ownership.source_fingerprint != ownership.snapshot_fingerprint
        {
            continue;
        }
        fs::remove_dir_all(&staging_dir)
            .map_err(|error| format!("Failed to remove {}: {error}", staging_dir.display()))?;
        removed += 1;
    }
    Ok(removed)
}

/// Remove expired local pack snapshots only when their ownership record and
/// current contents still match what Skill Studio staged.
pub fn reconcile_pack_import_staging_at_startup(home: &Path) -> Result<usize, String> {
    reconcile_pack_import_staging_with(home, unix_timestamp_now()?)
}

fn confirm_pack_import_trust_with(
    home: &Path,
    confirmation_token: &str,
    request: PackImportRequest,
    gh: &dyn GhContentsFetch,
    runner: &dyn CommandRunner,
    state: &PackImportTrustState,
    fork_lock: &ForkMutationLock,
) -> Result<ImportResult, String> {
    validate_pack_import_request(&request)?;
    let pending = {
        let mut tokens = state
            .0
            .lock()
            .map_err(|_| "Pack trust token state is unavailable".to_string())?;
        let now = Instant::now();
        prune_pack_trust_tokens(home, &mut tokens, now);
        let pending = tokens.get(confirmation_token).ok_or_else(|| {
            "Pack trust confirmation is invalid, expired, or already used".to_string()
        })?;
        if pending.request != request {
            return Err("Pack trust confirmation does not match this import request".to_string());
        }
        tokens
            .remove(confirmation_token)
            .expect("matching pack trust token remains while token state is locked")
    };

    let _guard = match fork_lock.try_acquire() {
        Ok(guard) => guard,
        Err(error) => {
            cleanup_prepared_pack_import(home, &pending.prepared);
            return Err(error);
        }
    };
    if let Err(error) =
        revalidate_prepared_pack_import(home, &request.source, &pending.prepared, gh)
    {
        cleanup_prepared_pack_import(home, &pending.prepared);
        return Err(error);
    }
    if let Err(error) = record_trusted_dotagents_sources(home, &pending.prepared.identities) {
        cleanup_prepared_pack_import(home, &pending.prepared);
        return Err(error);
    }
    for identity in &pending.prepared.identities {
        if let Err(error) = require_trusted_dotagents_identity(home, identity) {
            cleanup_prepared_pack_import(home, &pending.prepared);
            return Err(error.to_string());
        }
    }
    execute_and_cleanup_pack_import(home, pending.prepared, &request.agents, runner)
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
    request: PackImportRequest,
    app: tauri::AppHandle,
    trust_state: tauri::State<PackImportTrustState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<PackImportPreflightResult, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let gh_bin = if validate_pack_manifest_source(&request.source).is_ok() {
        skill_update_check::resolve_gh_binary().ok_or_else(|| "gh is not installed".to_string())?
    } else {
        PathBuf::new()
    };
    let result = preflight_pack_import_with(
        &home,
        request,
        &RealGhContentsFetch { gh_bin },
        &RealCommandRunner::new(),
        &trust_state,
        &fork_lock,
    )?;
    if matches!(result, PackImportPreflightResult::Imported { .. }) {
        skill_refresh::request_snapshot_rebuild(&app);
    }
    Ok(result)
}

/// Consume one pack trust token, revalidate the request and manifest, record
/// every displayed identity, then import while the mutation lock is held.
#[tauri::command]
pub fn confirm_skill_pack_trust(
    confirmation_token: String,
    request: PackImportRequest,
    app: tauri::AppHandle,
    trust_state: tauri::State<PackImportTrustState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<ImportResult, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let gh_bin = if validate_pack_manifest_source(&request.source).is_ok() {
        skill_update_check::resolve_gh_binary().ok_or_else(|| "gh is not installed".to_string())?
    } else {
        PathBuf::new()
    };
    let result = confirm_pack_import_trust_with(
        &home,
        &confirmation_token,
        request,
        &RealGhContentsFetch { gh_bin },
        &RealCommandRunner::new(),
        &trust_state,
        &fork_lock,
    )?;
    skill_refresh::request_snapshot_rebuild(&app);
    Ok(result)
}

/// Consume one pending pack trust token without trusting or importing it.
#[tauri::command]
pub fn abandon_pack_import_trust(
    confirmation_token: String,
    trust_state: tauri::State<PackImportTrustState>,
) -> Result<bool, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    abandon_pack_import_trust_with(&home, &confirmation_token, &trust_state)
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

    fn trust_import_sources(home: &Path, sources: &[&str]) {
        for source in sources {
            super::super::skill_trust_policy::record_trusted_dotagents_source(home, source)
                .unwrap();
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
        fn resolve_commit(&self, _owner_repo: &str) -> Result<String, String> {
            Ok("1111111111111111111111111111111111111111".to_string())
        }

        fn verify_commit(&self, _owner_repo: &str, _commit: &str) -> Result<(), String> {
            Ok(())
        }

        fn fetch_agents_toml(
            &self,
            _owner_repo: &str,
            _commit: &str,
        ) -> Result<Option<String>, String> {
            Ok(self.toml.clone())
        }
    }

    struct ChangingGhContents {
        toml: Mutex<Vec<Option<String>>>,
    }

    impl GhContentsFetch for ChangingGhContents {
        fn resolve_commit(&self, _owner_repo: &str) -> Result<String, String> {
            Ok("1111111111111111111111111111111111111111".to_string())
        }

        fn verify_commit(&self, _owner_repo: &str, _commit: &str) -> Result<(), String> {
            Ok(())
        }

        fn fetch_agents_toml(
            &self,
            _owner_repo: &str,
            _commit: &str,
        ) -> Result<Option<String>, String> {
            let mut values = self.toml.lock().unwrap();
            if values.len() > 1 {
                Ok(values.remove(0))
            } else {
                Ok(values[0].clone())
            }
        }
    }

    struct RecordedCommitGhContents {
        heads: Mutex<Vec<String>>,
        verify_results: Mutex<Vec<Result<(), String>>>,
        fetch_results: Mutex<Vec<Result<Option<String>, String>>>,
        fetched_commits: Mutex<Vec<String>>,
    }

    impl GhContentsFetch for RecordedCommitGhContents {
        fn resolve_commit(&self, _owner_repo: &str) -> Result<String, String> {
            let mut heads = self.heads.lock().unwrap();
            if heads.len() > 1 {
                Ok(heads.remove(0))
            } else {
                Ok(heads[0].clone())
            }
        }

        fn verify_commit(&self, _owner_repo: &str, _commit: &str) -> Result<(), String> {
            let mut results = self.verify_results.lock().unwrap();
            if results.len() > 1 {
                results.remove(0)
            } else {
                results[0].clone()
            }
        }

        fn fetch_agents_toml(
            &self,
            _owner_repo: &str,
            commit: &str,
        ) -> Result<Option<String>, String> {
            self.fetched_commits
                .lock()
                .unwrap()
                .push(commit.to_string());
            let mut results = self.fetch_results.lock().unwrap();
            if results.len() > 1 {
                results.remove(0)
            } else {
                results[0].clone()
            }
        }
    }

    struct LocalSnapshotRunner {
        installed_skill: Mutex<Option<String>>,
    }

    impl CommandRunner for LocalSnapshotRunner {
        fn run(&self, _program: &str, args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
            if args.contains(&"--all".to_string()) {
                let source = Path::new(&args[3]);
                *self.installed_skill.lock().unwrap() =
                    Some(fs::read_to_string(source.join("skills/local/SKILL.md")).unwrap());
            }
            Ok(())
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
        fn run(&self, _program: &str, args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
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

    fn pack_import_request(source: &str) -> PackImportRequest {
        PackImportRequest {
            source: source.to_string(),
            agents: Vec::new(),
            method: "pack".to_string(),
            destination: SkillDestination::Universal,
            scope: InstallScope::Global,
            project_path: None,
        }
    }

    fn trust_token(result: PackImportPreflightResult) -> (Vec<String>, String) {
        match result {
            PackImportPreflightResult::NeedsTrust {
                identities,
                confirmation_token,
            } => (identities, confirmation_token),
            PackImportPreflightResult::Imported { .. } => panic!("expected pack trust preflight"),
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
    fn clean_profile_pack_preflight_lists_every_identity_without_installing() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let gh = FakeGhContents {
            toml: Some(
                r#"
[[skills]]
name = "z"
source = "Other/Zed.git"
[[skills]]
name = "a"
source = "someone/repo"
"#
                .to_string(),
            ),
        };
        let state = PackImportTrustState::default();

        let (identities, _) = trust_token(
            preflight_pack_import_with(
                tmp.path(),
                pack_import_request("Someone/Repo.git"),
                &gh,
                &runner,
                &state,
                &ForkMutationLock::default(),
            )
            .unwrap(),
        );

        assert_eq!(identities, vec!["other/zed", "someone/repo"]);
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn explicit_pack_confirmation_trusts_all_identities_and_imports_once() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let gh = FakeGhContents {
            toml: Some("[[skills]]\nname = \"child\"\nsource = \"someone/child\"\n".to_string()),
        };
        let state = PackImportTrustState::default();
        let lock = ForkMutationLock::default();
        let request = pack_import_request("someone/repo");
        let (_, token) = trust_token(
            preflight_pack_import_with(tmp.path(), request.clone(), &gh, &runner, &state, &lock)
                .unwrap(),
        );

        confirm_pack_import_trust_with(tmp.path(), &token, request, &gh, &runner, &state, &lock)
            .unwrap();

        assert_eq!(runner.calls.lock().unwrap().len(), 2);
        assert!(require_trusted_dotagents_identity(tmp.path(), "someone/repo").is_ok());
        assert!(require_trusted_dotagents_identity(tmp.path(), "someone/child").is_ok());
    }

    #[test]
    fn remote_pack_confirmation_uses_recorded_commit_after_head_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let original_commit = "1".repeat(40);
        let changed_head = "2".repeat(40);
        let manifest =
            Some("[[skills]]\nname = \"child\"\nsource = \"someone/child\"\n".to_string());
        let gh = RecordedCommitGhContents {
            heads: Mutex::new(vec![original_commit.clone(), changed_head]),
            verify_results: Mutex::new(vec![Ok(()), Ok(())]),
            fetch_results: Mutex::new(vec![Ok(manifest.clone()), Ok(manifest)]),
            fetched_commits: Mutex::new(Vec::new()),
        };
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let state = PackImportTrustState::default();
        let lock = ForkMutationLock::default();
        let request = pack_import_request("someone/repo");
        let (_, token) = trust_token(
            preflight_pack_import_with(tmp.path(), request.clone(), &gh, &runner, &state, &lock)
                .unwrap(),
        );

        confirm_pack_import_trust_with(tmp.path(), &token, request, &gh, &runner, &state, &lock)
            .unwrap();

        assert_eq!(*gh.heads.lock().unwrap(), vec!["2".repeat(40)]);
        assert_eq!(
            *gh.fetched_commits.lock().unwrap(),
            vec![original_commit.clone(), original_commit.clone()]
        );
        let calls = runner.calls.lock().unwrap();
        assert!(calls[0].contains(&"--ref".to_string()));
        assert!(calls[0].contains(&original_commit));
    }

    #[test]
    fn remote_pack_confirmation_refuses_when_recorded_commit_is_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let commit = "1".repeat(40);
        let gh = RecordedCommitGhContents {
            heads: Mutex::new(vec![commit]),
            verify_results: Mutex::new(vec![Ok(()), Err("recorded SHA unavailable".to_string())]),
            fetch_results: Mutex::new(vec![Ok(None)]),
            fetched_commits: Mutex::new(Vec::new()),
        };
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let state = PackImportTrustState::default();
        let lock = ForkMutationLock::default();
        let request = pack_import_request("someone/repo");
        let (_, token) = trust_token(
            preflight_pack_import_with(tmp.path(), request.clone(), &gh, &runner, &state, &lock)
                .unwrap(),
        );

        let error = confirm_pack_import_trust_with(
            tmp.path(),
            &token,
            request,
            &gh,
            &runner,
            &state,
            &lock,
        )
        .unwrap_err();

        assert!(error.contains("recorded SHA unavailable"));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn local_pack_confirmation_installs_preflight_snapshot_after_source_edit() {
        let tmp = tempfile::tempdir().unwrap();
        let local_pack = tmp.path().join("local-pack");
        fs::create_dir_all(local_pack.join("skills/local")).unwrap();
        fs::write(local_pack.join("skills/local/SKILL.md"), "reviewed bytes").unwrap();
        fs::write(
            local_pack.join("agents.toml"),
            "[[skills]]\nname = \"child\"\nsource = \"someone/child\"\n",
        )
        .unwrap();
        let runner = LocalSnapshotRunner {
            installed_skill: Mutex::new(None),
        };
        let state = PackImportTrustState::default();
        let lock = ForkMutationLock::default();
        let request = pack_import_request(&local_pack.to_string_lossy());
        let (_, token) = trust_token(
            preflight_pack_import_with(
                tmp.path(),
                request.clone(),
                &FakeGhContents { toml: None },
                &runner,
                &state,
                &lock,
            )
            .unwrap(),
        );
        fs::write(local_pack.join("skills/local/SKILL.md"), "edited bytes").unwrap();

        confirm_pack_import_trust_with(
            tmp.path(),
            &token,
            request,
            &FakeGhContents { toml: None },
            &runner,
            &state,
            &lock,
        )
        .unwrap();

        assert_eq!(
            runner.installed_skill.lock().unwrap().as_deref(),
            Some("reviewed bytes")
        );
        assert!(dir_entry_names(&local_pack_staging_root(tmp.path())).is_empty());
    }

    #[test]
    fn local_pack_snapshot_cleanup_is_safe_and_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let local_pack = tmp.path().join("local-pack");
        fs::create_dir_all(&local_pack).unwrap();
        fs::write(local_pack.join("file.txt"), "reviewed").unwrap();
        let snapshot = snapshot_local_pack(tmp.path(), &local_pack.to_string_lossy()).unwrap();

        cleanup_local_pack_snapshot(tmp.path(), &snapshot);
        cleanup_local_pack_snapshot(tmp.path(), &snapshot);

        assert!(!snapshot.staging_dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn local_pack_snapshot_preserves_symlink_entries_and_fingerprint() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let local_pack = tmp.path().join("local-pack");
        fs::create_dir_all(local_pack.join("directory")).unwrap();
        fs::write(local_pack.join("file.txt"), "inside").unwrap();
        fs::write(local_pack.join("directory/nested.txt"), "nested").unwrap();
        let outside = tmp.path().join("outside.txt");
        fs::write(&outside, "outside").unwrap();
        symlink("file.txt", local_pack.join("file-link")).unwrap();
        symlink("directory", local_pack.join("directory-link")).unwrap();
        symlink(&outside, local_pack.join("outside-link")).unwrap();
        symlink("missing", local_pack.join("dangling-link")).unwrap();

        let snapshot = snapshot_local_pack(tmp.path(), &local_pack.to_string_lossy()).unwrap();

        assert_eq!(
            snapshot.source_fingerprint,
            super::super::event_store::fingerprint_path(&snapshot.source_dir)
        );
        for (name, target) in [
            ("file-link", Path::new("file.txt")),
            ("directory-link", Path::new("directory")),
            ("outside-link", outside.as_path()),
            ("dangling-link", Path::new("missing")),
        ] {
            assert_eq!(
                fs::read_link(snapshot.source_dir.join(name)).unwrap(),
                target
            );
        }
        cleanup_local_pack_snapshot(tmp.path(), &snapshot);
        assert!(!snapshot.staging_dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn local_pack_snapshot_refuses_special_file_and_cleans_staging() {
        use std::os::unix::net::UnixListener;

        let tmp = tempfile::tempdir().unwrap();
        let local_pack = tmp.path().join("local-pack");
        fs::create_dir_all(&local_pack).unwrap();
        let _listener = UnixListener::bind(local_pack.join("special.socket")).unwrap();

        let error = match snapshot_local_pack(tmp.path(), &local_pack.to_string_lossy()) {
            Ok(_) => panic!("special file snapshot unexpectedly succeeded"),
            Err(error) => error,
        };

        assert!(error.contains("Refused to copy unsupported special file"));
        assert!(error.contains("special.socket"));
        assert!(dir_entry_names(&local_pack_staging_root(tmp.path())).is_empty());
    }

    #[test]
    fn expired_local_pack_confirmation_cleans_its_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let local_pack = tmp.path().join("local-pack");
        fs::create_dir_all(&local_pack).unwrap();
        fs::write(
            local_pack.join("agents.toml"),
            "[[skills]]\nname = \"child\"\nsource = \"someone/child\"\n",
        )
        .unwrap();
        let state = PackImportTrustState::default();
        let lock = ForkMutationLock::default();
        let request = pack_import_request(&local_pack.to_string_lossy());
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let (_, token) = trust_token(
            preflight_pack_import_with(
                tmp.path(),
                request.clone(),
                &FakeGhContents { toml: None },
                &runner,
                &state,
                &lock,
            )
            .unwrap(),
        );
        let staging_dir = {
            let mut tokens = state.0.lock().unwrap();
            let pending = tokens.get_mut(&token).unwrap();
            pending.expires_at = Instant::now();
            pending
                .prepared
                .local_snapshot
                .as_ref()
                .unwrap()
                .staging_dir
                .clone()
        };

        assert!(confirm_pack_import_trust_with(
            tmp.path(),
            &token,
            request,
            &FakeGhContents { toml: None },
            &runner,
            &state,
            &lock,
        )
        .is_err());

        assert!(!staging_dir.exists());
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn local_pack_snapshot_cleanup_preserves_unknown_staging_content() {
        let tmp = tempfile::tempdir().unwrap();
        let local_pack = tmp.path().join("local-pack");
        fs::create_dir_all(&local_pack).unwrap();
        fs::write(local_pack.join("file.txt"), "reviewed").unwrap();
        let snapshot = snapshot_local_pack(tmp.path(), &local_pack.to_string_lossy()).unwrap();
        fs::write(snapshot.staging_dir.join("unknown.txt"), "do not delete").unwrap();

        cleanup_local_pack_snapshot(tmp.path(), &snapshot);

        assert_eq!(
            fs::read_to_string(snapshot.staging_dir.join("unknown.txt")).unwrap(),
            "do not delete"
        );
    }

    #[test]
    fn abandoning_pack_trust_cleans_only_the_exact_owned_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let local_pack = tmp.path().join("local-pack");
        fs::create_dir_all(&local_pack).unwrap();
        fs::write(
            local_pack.join("agents.toml"),
            "[[skills]]\nname = \"child\"\nsource = \"someone/child\"\n",
        )
        .unwrap();
        let state = PackImportTrustState::default();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let (_, token) = trust_token(
            preflight_pack_import_with(
                tmp.path(),
                pack_import_request(&local_pack.to_string_lossy()),
                &FakeGhContents { toml: None },
                &runner,
                &state,
                &ForkMutationLock::default(),
            )
            .unwrap(),
        );
        let staging_dir = state
            .0
            .lock()
            .unwrap()
            .get(&token)
            .unwrap()
            .prepared
            .local_snapshot
            .as_ref()
            .unwrap()
            .staging_dir
            .clone();
        let unknown = local_pack_staging_root(tmp.path()).join("unknown-content");
        fs::create_dir_all(&unknown).unwrap();
        fs::write(unknown.join("keep.txt"), "keep").unwrap();

        assert!(abandon_pack_import_trust_with(tmp.path(), &token, &state).unwrap());
        assert!(!staging_dir.exists());
        assert!(!abandon_pack_import_trust_with(tmp.path(), &token, &state).unwrap());
        assert!(!abandon_pack_import_trust_with(tmp.path(), "unknown-token", &state).unwrap());
        assert_eq!(
            fs::read_to_string(unknown.join("keep.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn confirm_and_abandon_pack_trust_have_one_winner() {
        let tmp = tempfile::tempdir().unwrap();
        let local_pack = tmp.path().join("local-pack");
        fs::create_dir_all(&local_pack).unwrap();
        fs::write(
            local_pack.join("agents.toml"),
            "[[skills]]\nname = \"child\"\nsource = \"someone/child\"\n",
        )
        .unwrap();
        let state = PackImportTrustState::default();
        let lock = ForkMutationLock::default();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let request = pack_import_request(&local_pack.to_string_lossy());
        let (_, token) = trust_token(
            preflight_pack_import_with(
                tmp.path(),
                request.clone(),
                &FakeGhContents { toml: None },
                &runner,
                &state,
                &lock,
            )
            .unwrap(),
        );
        let barrier = std::sync::Barrier::new(2);

        let (confirmed, abandoned) = std::thread::scope(|scope| {
            let confirm = scope.spawn(|| {
                barrier.wait();
                confirm_pack_import_trust_with(
                    tmp.path(),
                    &token,
                    request,
                    &FakeGhContents { toml: None },
                    &runner,
                    &state,
                    &lock,
                )
                .is_ok()
            });
            let abandon = scope.spawn(|| {
                barrier.wait();
                abandon_pack_import_trust_with(tmp.path(), &token, &state).unwrap()
            });
            (confirm.join().unwrap(), abandon.join().unwrap())
        });

        assert_ne!(confirmed, abandoned);
        assert!(dir_entry_names(&local_pack_staging_root(tmp.path())).is_empty());
    }

    #[test]
    fn startup_reconcile_removes_only_stale_unchanged_owned_snapshots() {
        let tmp = tempfile::tempdir().unwrap();
        let local_pack = tmp.path().join("local-pack");
        fs::create_dir_all(&local_pack).unwrap();
        fs::write(
            local_pack.join("agents.toml"),
            "[[skills]]\nname = \"child\"\nsource = \"someone/child\"\n",
        )
        .unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let state = PackImportTrustState::default();
        let (_, token) = trust_token(
            preflight_pack_import_with(
                tmp.path(),
                pack_import_request(&local_pack.to_string_lossy()),
                &FakeGhContents { toml: None },
                &runner,
                &state,
                &ForkMutationLock::default(),
            )
            .unwrap(),
        );
        let staging_dir = state
            .0
            .lock()
            .unwrap()
            .get(&token)
            .unwrap()
            .prepared
            .local_snapshot
            .as_ref()
            .unwrap()
            .staging_dir
            .clone();
        let created_at = read_local_pack_snapshot_ownership(&staging_dir)
            .unwrap()
            .created_at_unix_seconds;

        assert_eq!(
            reconcile_pack_import_staging_with(
                tmp.path(),
                created_at + PACK_TRUST_TOKEN_TTL.as_secs() - 1
            )
            .unwrap(),
            0
        );
        assert!(staging_dir.exists());
        assert_eq!(
            reconcile_pack_import_staging_with(
                tmp.path(),
                created_at + PACK_TRUST_TOKEN_TTL.as_secs()
            )
            .unwrap(),
            1
        );
        assert!(!staging_dir.exists());
        assert_eq!(
            reconcile_pack_import_staging_with(
                tmp.path(),
                created_at + PACK_TRUST_TOKEN_TTL.as_secs()
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn startup_reconcile_preserves_edited_unknown_and_unowned_snapshots() {
        let tmp = tempfile::tempdir().unwrap();
        let local_pack = tmp.path().join("local-pack");
        fs::create_dir_all(&local_pack).unwrap();
        fs::write(
            local_pack.join("agents.toml"),
            "[[skills]]\nname = \"child\"\nsource = \"someone/child\"\n",
        )
        .unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let state = PackImportTrustState::default();
        let (_, token) = trust_token(
            preflight_pack_import_with(
                tmp.path(),
                pack_import_request(&local_pack.to_string_lossy()),
                &FakeGhContents { toml: None },
                &runner,
                &state,
                &ForkMutationLock::default(),
            )
            .unwrap(),
        );
        let edited = state
            .0
            .lock()
            .unwrap()
            .get(&token)
            .unwrap()
            .prepared
            .local_snapshot
            .as_ref()
            .unwrap()
            .staging_dir
            .clone();
        let created_at = read_local_pack_snapshot_ownership(&edited)
            .unwrap()
            .created_at_unix_seconds;
        fs::write(edited.join("source/edited.txt"), "changed").unwrap();
        let unknown = local_pack_staging_root(tmp.path()).join("unknown");
        fs::create_dir_all(&unknown).unwrap();
        fs::write(unknown.join("keep.txt"), "keep").unwrap();
        let unowned = snapshot_local_pack(tmp.path(), &local_pack.to_string_lossy())
            .unwrap()
            .staging_dir;

        assert_eq!(
            reconcile_pack_import_staging_with(
                tmp.path(),
                created_at + PACK_TRUST_TOKEN_TTL.as_secs()
            )
            .unwrap(),
            0
        );
        assert!(edited.exists());
        assert!(unknown.exists());
        assert!(unowned.exists());
    }

    #[test]
    fn pack_confirmation_refuses_changed_manifest_request_and_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let original = "[[skills]]\nname = \"child\"\nsource = \"someone/child\"\n";
        let changed = "[[skills]]\nname = \"child\"\nsource = \"someone/changed\"\n";
        let gh = ChangingGhContents {
            toml: Mutex::new(vec![Some(original.to_string()), Some(changed.to_string())]),
        };
        let state = PackImportTrustState::default();
        let lock = ForkMutationLock::default();
        let request = pack_import_request("someone/repo");
        let (_, token) = trust_token(
            preflight_pack_import_with(tmp.path(), request.clone(), &gh, &runner, &state, &lock)
                .unwrap(),
        );
        let mut mismatched = request.clone();
        mismatched.agents.push(AgentId::Codex);
        assert!(confirm_pack_import_trust_with(
            tmp.path(),
            &token,
            mismatched,
            &gh,
            &runner,
            &state,
            &lock,
        )
        .unwrap_err()
        .contains("does not match"));
        assert!(confirm_pack_import_trust_with(
            tmp.path(),
            &token,
            request.clone(),
            &gh,
            &runner,
            &state,
            &lock,
        )
        .unwrap_err()
        .contains("Pack changed"));
        assert!(confirm_pack_import_trust_with(
            tmp.path(),
            &token,
            request,
            &gh,
            &runner,
            &state,
            &lock,
        )
        .unwrap_err()
        .contains("already used"));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn pack_confirmation_preserves_registry_changes_after_preflight() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let gh = FakeGhContents { toml: None };
        let state = PackImportTrustState::default();
        let lock = ForkMutationLock::default();
        let request = pack_import_request("someone/repo");
        let (_, token) = trust_token(
            preflight_pack_import_with(tmp.path(), request.clone(), &gh, &runner, &state, &lock)
                .unwrap(),
        );
        super::super::skill_trust_policy::record_trusted_dotagents_source(
            tmp.path(),
            "concurrent/repo",
        )
        .unwrap();

        confirm_pack_import_trust_with(tmp.path(), &token, request, &gh, &runner, &state, &lock)
            .unwrap();

        assert!(require_trusted_dotagents_identity(tmp.path(), "concurrent/repo").is_ok());
        assert!(require_trusted_dotagents_identity(tmp.path(), "someone/repo").is_ok());
    }

    #[test]
    fn expired_and_unknown_pack_confirmation_tokens_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let gh = FakeGhContents { toml: None };
        let state = PackImportTrustState::default();
        let lock = ForkMutationLock::default();
        let request = pack_import_request("someone/repo");
        let (_, token) = trust_token(
            preflight_pack_import_with(tmp.path(), request.clone(), &gh, &runner, &state, &lock)
                .unwrap(),
        );
        state.0.lock().unwrap().get_mut(&token).unwrap().expires_at = Instant::now();

        for invalid_token in [&token, "unknown-token"] {
            assert!(confirm_pack_import_trust_with(
                tmp.path(),
                invalid_token,
                request.clone(),
                &gh,
                &runner,
                &state,
                &lock,
            )
            .unwrap_err()
            .contains("invalid, expired, or already used"));
        }
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn trusted_and_local_pack_preflights_import_without_confirmation() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("local-pack/skills")).unwrap();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: tmp.path().to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };
        let gh = FakeGhContents { toml: None };
        trust_import_sources(tmp.path(), &["someone/repo"]);

        for source in [
            "someone/repo".to_string(),
            tmp.path().join("local-pack").to_string_lossy().to_string(),
        ] {
            assert!(matches!(
                preflight_pack_import_with(
                    tmp.path(),
                    pack_import_request(&source),
                    &gh,
                    &runner,
                    &PackImportTrustState::default(),
                    &ForkMutationLock::default(),
                )
                .unwrap(),
                PackImportPreflightResult::Imported { .. }
            ));
        }
        assert_eq!(runner.calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn import_refuses_an_untrusted_pack_before_running_dotagents() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let runner = FakeRunner {
            calls: Mutex::new(Vec::new()),
            home: home.to_path_buf(),
            all_creates: vec![],
            fail_sources: vec![],
        };

        let error = import_skill_pack_with(
            home,
            "kentcdodds/kcd-skills",
            &[],
            &FakeGhContents { toml: None },
            &runner,
        )
        .unwrap_err();

        assert_eq!(error, "Untrusted dotagents source: kentcdodds/kcd-skills");
        assert!(runner.calls.lock().unwrap().is_empty());
    }

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
        trust_import_sources(home, &["someone/repo"]);

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
        trust_import_sources(home, &["someone/repo", "someone/referenced-a"]);

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
        trust_import_sources(home, &["someone/repo", "someone/broken"]);

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
        trust_import_sources(home, &["someone/repo"]);

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
        trust_import_sources(home, &["someone/repo"]);

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
        trust_import_sources(home, &["someone/repo"]);

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
        trust_import_sources(home, &["someone/repo"]);

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
        trust_import_sources(home, &["someone/repo"]);

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
        trust_import_sources(home, &["someone/repo", "someone/bundled-a"]);

        let result = import_skill_pack_with(home, "someone/repo", &[], &gh, &runner).unwrap();

        assert_eq!(result.bundled, vec!["bundled-a"]);
        assert!(result.referenced.is_empty());
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains(&"--all".to_string()));
    }
}
