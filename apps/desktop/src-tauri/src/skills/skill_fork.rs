// ============================================================================
// Skills Module - skill_fork
// Fork / Pull upstream / Un-fork for a dotagents- or skills.sh-managed skill:
// "Fork" detaches it from its owning ledger (so `sync`/`update` can't
// overwrite local edits) while keeping a snapshot of the last-synced copy;
// "Pull upstream" compares that snapshot against the skill's current on-disk
// copy and a freshly fetched upstream copy, never merging automatically: a
// file that differs on all three sides gets conflict markers written into it
// and is opened in the user's editor; "Un-fork" discards local edits and
// reinstalls from the recorded origin. The CLI-shelling and GitHub-fetching
// bits are behind small traits so the conflict-marker/refusal logic is
// testable with fakes.
// ============================================================================

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tauri::Manager;

use super::agents::AgentId;
use super::commands::{dotagents_add_args, dotagents_remove_args};
use super::skill_deployment::SkillDestination;
use super::skill_dto::InstallScope;
use super::skill_fork_registry::{
    fork_snapshot_dir, read_fork_registry, write_fork_registry_locked, ForkRecord, ForkRegistry,
    OriginTool,
};
use super::skill_fs::copy_dir_all;
use super::skill_lifecycle::skills_sh_remove_args_for_scope;
use super::skill_process::{
    run_controlled_command_to_file, AddOperationControl, ControlledProcessError,
    MAX_PROCESS_OUTPUT_BYTES,
};
use super::skill_refresh::{self, SkillRefreshState};
use super::skill_update_check::{self, CommitLookup, GhCommitLookup};
use skill_studio_core::dotagents_ledger;
use skill_studio_core::lock_file;

// ============================================================================
// Traits - real implementations shell out / hit the network; tests use fakes.
// ============================================================================

/// Removes a skill from its owning ledger, or reinstalls it from its
/// recorded origin. The real implementation shells out to the same argv
/// `remove_skill` / `add_skill` build (see the command and install-plan arg
/// builders) - `ops::update`'s own dotagents argv now lives in
/// `ops_update::update_cli_args_and_cwd` instead.
pub trait LedgerTool {
    fn remove(&self, tool: OriginTool, name: &str) -> Result<(), String>;
    fn reinstall(&self, rec: &ForkRecord, name: &str) -> Result<(), String>;
}

/// Fetches a skill's directory out of its upstream repo at a specific
/// commit, read-only. The real implementation runs `gh api
/// repos/{repo}/tarball/{commit}`, extracts it to a temp dir, and locates
/// `<top>/<path>` inside it.
pub trait UpstreamFetch {
    fn fetch_skill_dir(
        &self,
        repo: &str,
        path: &str,
        commit: &str,
        into: &Path,
    ) -> Result<(), String>;

    /// Downloads `repo` at `commit` once so several folders can be copied
    /// out of it without refetching - see `add_skills`, which installs a
    /// whole picker's worth of skills from one tarball. `Ok(None)` means
    /// this implementation has no bulk mode and the caller falls back to
    /// one `fetch_skill_dir` per folder.
    fn open_repo(
        &self,
        _repo: &str,
        _commit: &str,
    ) -> Result<Option<Box<dyn RepoSnapshot>>, String> {
        Ok(None)
    }

    /// Fetch under one Add operation's cancellation flag and deadline.
    fn fetch_skill_dir_controlled(
        &self,
        repo: &str,
        path: &str,
        commit: &str,
        into: &Path,
        control: &AddOperationControl,
    ) -> Result<(), String> {
        control.check_message()?;
        let result = self.fetch_skill_dir(repo, path, commit, into);
        control.check_message()?;
        result
    }

    /// Open a reusable repo snapshot under one Add operation deadline.
    fn open_repo_controlled(
        &self,
        repo: &str,
        commit: &str,
        control: &AddOperationControl,
    ) -> Result<Option<Box<dyn RepoSnapshot>>, String> {
        control.check_message()?;
        let result = self.open_repo(repo, commit);
        control.check_message()?;
        result
    }
}

/// A repo already downloaded and extracted at one commit; `copy_dir` pulls
/// one folder out of it.
pub trait RepoSnapshot {
    fn copy_dir(&self, path: &str, into: &Path) -> Result<(), String>;

    /// Copy one folder while honoring an Add operation interruption.
    fn copy_dir_controlled(
        &self,
        path: &str,
        into: &Path,
        control: &AddOperationControl,
    ) -> Result<(), String> {
        control.check_message()?;
        let result = self.copy_dir(path, into);
        control.check_message()?;
        result
    }
}

/// Real `LedgerTool`, shelling out to `npx`.
pub struct RealLedgerTool;

// ============================================================================
// skills.sh Universal argv - moved from `skill_install_plan.rs` (unit 3.5c):
// `skill_add.rs`'s own install path is gone, and `ops_install_cli.rs` builds
// this argv for every new install, so this stays only for `skills_sh_unfork_add_args`
// below, an unrelated reinstall-from-origin call `ops::install` doesn't cover.
// ============================================================================

/// One install request used by `skills_sh_unfork_add_args`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillInstallSpec {
    pub scope: InstallScope,
    pub destination: SkillDestination,
    pub project_path: Option<String>,
    /// Harnesses that receive a Claude Code link (Universal). Empty
    /// Universal still writes `.agents/skills`.
    pub harnesses: Vec<AgentId>,
}

/// Universal skills.sh argv, and the process cwd to run it in. Never
/// includes Codex as a proxy for Universal. `skills@1.7.0` has no `--cwd`
/// flag (PR #101 / `fix/project-install-runs-in-project-dir`), so a project
/// scope returns the project path as the process cwd instead of an argv
/// token - the same fix as `ops_install_cli.rs`'s `cli_args_and_cwd`.
pub fn skills_sh_universal_add_args(
    repo_source: &str,
    skill_name: Option<&str>,
    spec: &SkillInstallSpec,
) -> Result<(Vec<String>, Option<PathBuf>), String> {
    if spec.destination != SkillDestination::Universal {
        return Err("skills.sh Universal argv is only for the Universal destination".to_string());
    }
    let mut args = vec![
        "skills".to_string(),
        "add".to_string(),
        repo_source.to_string(),
        "--yes".to_string(),
    ];
    let cwd = match spec.scope {
        InstallScope::Global => {
            args.push("--global".to_string());
            None
        }
        InstallScope::Project => {
            let path = spec
                .project_path
                .as_deref()
                .ok_or("Project scope needs a project path")?;
            Some(PathBuf::from(path))
        }
    };
    if let Some(name) = skill_name {
        args.push("--skill".to_string());
        args.push(name.to_string());
    }
    args.push("--agent".to_string());
    args.push("universal".to_string());
    if spec.harnesses.contains(&AgentId::ClaudeCode) {
        args.push("--agent".to_string());
        args.push("claude-code".to_string());
    }
    Ok((args, cwd))
}

fn skills_sh_unfork_add_args(
    rec: &ForkRecord,
    name: &str,
) -> Result<(Vec<String>, Option<PathBuf>), String> {
    // Fork only ever applies to a global-scope skill (see
    // `skill_refresh::build_snapshot`), so this reinstall is always global
    // and the cwd `skills_sh_universal_add_args` returns is always `None` -
    // still threaded through `run_npx` rather than discarded, so a future
    // caller that reinstalls a project-scope fork gets the right cwd for
    // free instead of a silently dropped one.
    let spec = SkillInstallSpec {
        scope: InstallScope::Global,
        destination: SkillDestination::Universal,
        project_path: None,
        harnesses: vec![],
    };
    skills_sh_universal_add_args(&rec.origin_source, Some(name), &spec)
}

fn run_npx(args: &[String], cwd: Option<&Path>) -> Result<(), String> {
    let mut command = Command::new("npx");
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
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

impl LedgerTool for RealLedgerTool {
    fn remove(&self, tool: OriginTool, name: &str) -> Result<(), String> {
        let args = match tool {
            OriginTool::Dotagents => dotagents_remove_args(name, InstallScope::Global),
            // Fork only ever applies to a global-scope skill (see
            // `skill_refresh::build_snapshot`), so remove/reinstall always
            // target the global scope.
            OriginTool::SkillsSh => skills_sh_remove_args_for_scope(name, InstallScope::Global),
        };
        run_npx(&args, None)
    }

    fn reinstall(&self, rec: &ForkRecord, name: &str) -> Result<(), String> {
        let (args, cwd) = match rec.origin_tool {
            OriginTool::Dotagents => (
                dotagents_add_args(&rec.origin_source, name, rec.declared_ref.as_deref()),
                None,
            ),
            OriginTool::SkillsSh => skills_sh_unfork_add_args(rec, name)?,
        };
        run_npx(&args, cwd.as_deref())
    }
}

/// A `CommitLookup` that always fails with `message` - used when `gh` isn't
/// resolvable, so a lookup attempt surfaces exactly "Run Check now first"
/// instead of a confusing "failed to run gh" further down the call chain.
struct UnavailableLookup(String);

impl CommitLookup for UnavailableLookup {
    fn latest_commit(
        &self,
        _repo: &str,
        _path: &str,
        _until: Option<&str>,
    ) -> Result<Option<(String, String)>, String> {
        Err(self.0.clone())
    }
}

/// Real `CommitLookup`: `gh` if resolvable, otherwise a lookup that always
/// fails with "Run Check now first" - so a fork/pull that doesn't actually
/// need a fresh lookup (a cached baseline in the update-check store) never
/// requires `gh` at all.
fn resolve_lookup() -> Box<dyn CommitLookup> {
    match skill_update_check::resolve_gh_binary() {
        Some(gh_bin) => Box::new(GhCommitLookup { gh_bin }),
        None => Box::new(UnavailableLookup("Run Check now first".to_string())),
    }
}

/// Runs the same fork transaction as `fork_skill` after another command has
/// already resolved and locked the exact Global Universal deployment.
pub(crate) fn fork_resolved_deployment_with_real_services(
    guard: &super::write_lease::WriteLeaseGuard,
    home: &Path,
    app_data: &Path,
    name: &str,
    path: &Path,
) -> Result<ForkRecord, String> {
    let lookup = resolve_lookup();
    let gh_bin =
        skill_update_check::resolve_gh_binary().ok_or_else(|| "Run Check now first".to_string())?;
    let fetch = RealUpstreamFetch {
        gh_bin,
        cache_dir: app_data.join("skill-studio").join("cache"),
    };
    fork_skill_with(
        guard,
        home,
        app_data,
        name,
        path,
        &RealLedgerTool,
        &fetch,
        lookup.as_ref(),
    )
}

/// Real `UpstreamFetch`, via `gh api .../tarball/<sha>` + `tar -xzf`.
pub struct RealUpstreamFetch {
    pub gh_bin: PathBuf,
    /// Scratch directory for the tarball and its extraction - the app cache
    /// dir, cleaned up (best-effort) after every fetch.
    pub cache_dir: PathBuf,
}

impl UpstreamFetch for RealUpstreamFetch {
    fn fetch_skill_dir(
        &self,
        repo: &str,
        path: &str,
        commit: &str,
        into: &Path,
    ) -> Result<(), String> {
        self.download(repo, commit)?.copy_dir(path, into)
    }

    fn open_repo(&self, repo: &str, commit: &str) -> Result<Option<Box<dyn RepoSnapshot>>, String> {
        Ok(Some(Box::new(self.download(repo, commit)?)))
    }

    fn fetch_skill_dir_controlled(
        &self,
        repo: &str,
        path: &str,
        commit: &str,
        into: &Path,
        control: &AddOperationControl,
    ) -> Result<(), String> {
        self.download_controlled(repo, commit, control)?
            .copy_dir_controlled(path, into, control)
    }

    fn open_repo_controlled(
        &self,
        repo: &str,
        commit: &str,
        control: &AddOperationControl,
    ) -> Result<Option<Box<dyn RepoSnapshot>>, String> {
        Ok(Some(Box::new(
            self.download_controlled(repo, commit, control)?,
        )))
    }
}

impl RealUpstreamFetch {
    /// One `gh api .../tarball/<sha>` download, extracted to a scratch
    /// directory that is removed when the returned snapshot drops.
    fn download(&self, repo: &str, commit: &str) -> Result<ExtractedRepo, String> {
        self.download_controlled(repo, commit, &AddOperationControl::bounded_default())
    }

    fn download_controlled(
        &self,
        repo: &str,
        commit: &str,
        control: &AddOperationControl,
    ) -> Result<ExtractedRepo, String> {
        control.check_message()?;
        fs::create_dir_all(&self.cache_dir)
            .map_err(|e| format!("Failed to create {}: {e}", self.cache_dir.display()))?;

        let unique = format!("{}-{}", std::process::id(), commit);
        let tarball_path = self.cache_dir.join(format!("fork-pull-{unique}.tar.gz"));
        let extract_dir = self.cache_dir.join(format!("fork-pull-extract-{unique}"));
        let cleanup = TempCleanup {
            paths: vec![tarball_path.clone(), extract_dir.clone()],
        };

        run_controlled_command_to_file(
            &self.gh_bin,
            &["api".to_string(), format!("repos/{repo}/tarball/{commit}")],
            None,
            control,
            &tarball_path,
            MAX_PROCESS_OUTPUT_BYTES,
        )
        .map_err(ControlledProcessError::into_message)?;

        control.check_message()?;
        fs::create_dir_all(&extract_dir)
            .map_err(|e| format!("Failed to create {}: {e}", extract_dir.display()))?;
        run_controlled_command_to_file(
            Path::new("tar"),
            &[
                "-xzf".to_string(),
                tarball_path.to_string_lossy().into_owned(),
                "-C".to_string(),
                extract_dir.to_string_lossy().into_owned(),
            ],
            None,
            control,
            Path::new("/dev/null"),
            MAX_PROCESS_OUTPUT_BYTES,
        )
        .map_err(ControlledProcessError::into_message)?;
        control.check_message()?;

        Ok(ExtractedRepo {
            extract_dir,
            _cleanup: cleanup,
        })
    }
}

/// A tarball already extracted under `extract_dir`, kept alive for as long
/// as folders are still being copied out of it.
struct ExtractedRepo {
    extract_dir: PathBuf,
    _cleanup: TempCleanup,
}

impl RepoSnapshot for ExtractedRepo {
    fn copy_dir(&self, path: &str, into: &Path) -> Result<(), String> {
        let source_dir = locate_extracted_skill_dir(&self.extract_dir, path)?;
        copy_dir_all(&source_dir, into)
    }

    fn copy_dir_controlled(
        &self,
        path: &str,
        into: &Path,
        control: &AddOperationControl,
    ) -> Result<(), String> {
        control.check_message()?;
        let source_dir = locate_extracted_skill_dir(&self.extract_dir, path)?;
        super::skill_fs::copy_dir_all_controlled(&source_dir, into, control)
    }
}

/// Best-effort recursive cleanup of scratch paths, run whether
/// `fetch_skill_dir` succeeds or fails.
struct TempCleanup {
    paths: Vec<PathBuf>,
}

impl Drop for TempCleanup {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
            let _ = fs::remove_dir_all(path);
        }
    }
}

/// Finds `<top>/<path>` inside an already-extracted GitHub tarball
/// (`gh api repos/{repo}/tarball/{sha}` always has exactly one top-level
/// `<owner>-<repo>-<sha7>/` directory), and refuses a `path` that would
/// resolve outside the extraction directory. Pulled out of
/// `RealUpstreamFetch::fetch_skill_dir` so the tarball-locating logic is
/// testable without a network call.
fn locate_extracted_skill_dir(extract_dir: &Path, path: &str) -> Result<PathBuf, String> {
    let top = fs::read_dir(extract_dir)
        .map_err(|e| format!("Failed to read {}: {e}", extract_dir.display()))?
        .filter_map(std::result::Result::ok)
        .find(|e| e.path().is_dir())
        .ok_or_else(|| "Tarball had no top-level directory".to_string())?
        .path();

    let candidate = top.join(path);
    let canonical_extract = fs::canonicalize(extract_dir)
        .map_err(|e| format!("Failed to resolve {}: {e}", extract_dir.display()))?;
    let canonical_candidate = fs::canonicalize(&candidate)
        .map_err(|_| format!("{path} was not found in the fetched tarball"))?;
    if !canonical_candidate.starts_with(&canonical_extract) {
        return Err("Refusing to extract a path outside the tarball".to_string());
    }
    Ok(canonical_candidate)
}

/// Every relative file path (`/`-separated) under `dir`, skipping `.git` and
/// symlinks. Empty when `dir` doesn't exist.
fn collect_relative_files(dir: &Path, out: &mut BTreeSet<String>) {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            if entry.file_name() == ".git" {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                walk(root, &entry.path(), out);
            } else if let Ok(rel) = entry.path().strip_prefix(root) {
                out.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    walk(dir, dir, out);
}

// ============================================================================
// Fork
// ============================================================================

/// Where a skill's ledger provenance came from, resolved by
/// `resolve_fork_origin`.
struct ForkOrigin {
    tool: OriginTool,
    origin_source: String,
    repo: String,
    path: String,
    declared_ref: Option<String>,
    base_commit: String,
}

/// Resolves `name`'s ledger provenance and its fork `base_commit`, or the
/// refusal message `fork_skill` should return instead. `agents_dir` is
/// `home/.agents`.
fn resolve_fork_origin(
    agents_dir: &Path,
    app_data: &Path,
    name: &str,
    lookup: &dyn CommitLookup,
) -> Result<ForkOrigin, String> {
    let fs = skill_studio_host::RealFs::new();
    let dotagents_skills =
        dotagents_ledger::read_dotagents_ledger(&fs, agents_dir).map_err(|e| e.to_string())?;
    if let Some(entry) = dotagents_skills.into_iter().find(|s| s.name == name) {
        if !entry.has_manifest_row {
            return Err(format!(
                "`{name}` comes from the wildcard source `{}`; dotagents install would overwrite a fork. Add it by name first.",
                entry.source
            ));
        }
        let repo = entry.github_repo.clone().ok_or_else(|| {
            format!(
                "`{name}` is not hosted on GitHub; forking is only supported for GitHub sources"
            )
        })?;
        let base_commit = entry
            .installed_commit
            .clone()
            .ok_or_else(|| format!("Could not determine {name}'s installed commit"))?;
        return Ok(ForkOrigin {
            tool: OriginTool::Dotagents,
            origin_source: entry.source,
            repo,
            path: entry.path,
            declared_ref: entry.declared_ref,
            base_commit,
        });
    }

    let lock = lock_file::read_lock_file(&fs, &lock_file::lock_file_path_in(agents_dir))
        .map_err(|e| e.to_string())?;
    if let Some(entry) = lock.skills.get(name) {
        if entry.source_type != "github" {
            return Err(format!(
                "`{name}` is not hosted on GitHub; forking is only supported for GitHub sources"
            ));
        }
        let repo = dotagents_ledger::github_repo_from_source(&entry.source)
            .ok_or_else(|| format!("Could not determine {name}'s GitHub repo from its source"))?;
        let skill_path = entry.skill_path.clone().unwrap_or_default();
        let path = skill_path
            .strip_suffix("/SKILL.md")
            .unwrap_or(&skill_path)
            .to_string();

        let store = skill_update_check::read_update_check_store(app_data);
        let owner_id = format!("owner:v1/global/{name}");
        let base_commit = if let Some(commit) = store
            .owners
            .get(&owner_id)
            .and_then(|s| s.installed_commit.clone())
        {
            commit
        } else {
            let until = if entry.updated_at.is_empty() {
                None
            } else {
                Some(entry.updated_at.as_str())
            };
            match lookup.latest_commit(&repo, &path, until)? {
                Some((sha, _)) => sha,
                None => return Err(format!("Could not determine {name}'s installed commit")),
            }
        };

        return Ok(ForkOrigin {
            tool: OriginTool::SkillsSh,
            origin_source: entry.source.clone(),
            repo,
            path,
            declared_ref: None,
            base_commit,
        });
    }

    Err(format!(
        "`{name}` is not managed by dotagents or skills.sh; only skills installed through one of those can be forked"
    ))
}

/// A scratch copy of the live tree taken right before the ledger's `remove`
/// runs, so a folder wiped by that removal can be restored even though the
/// snapshot dir now holds the upstream base, not the live tree (see
/// `fork_skill_with`).
fn fork_live_recovery_dir(app_data: &Path, name: &str) -> PathBuf {
    app_data
        .join("skill-studio")
        .join("forks")
        .join(name)
        .join("live-recovery")
}

/// Owned sibling used to protect an earlier live recovery while a new fork
/// transaction prepares its replacement.
fn fork_live_recovery_quarantine_dir(app_data: &Path, name: &str) -> PathBuf {
    app_data
        .join("skill-studio")
        .join("forks")
        .join(name)
        .join("live-recovery-quarantine")
}

trait ForkTransactionStorage {
    fn rename_dir(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn remove_dir_all(&self, path: &Path) -> std::io::Result<()>;
    fn snapshot_live_skill(&self, skill_dir: &Path, recovery_dir: &Path) -> Result<(), String>;
    fn read_registry(&self, home: &Path) -> Result<ForkRegistry, String>;
    fn write_registry(
        &self,
        guard: &super::write_lease::WriteLeaseGuard,
        home: &Path,
        registry: &ForkRegistry,
    ) -> Result<(), String>;
}

struct FileForkTransactionStorage;

impl ForkTransactionStorage for FileForkTransactionStorage {
    fn rename_dir(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        fs::rename(from, to)
    }

    fn remove_dir_all(&self, path: &Path) -> std::io::Result<()> {
        fs::remove_dir_all(path)
    }

    fn snapshot_live_skill(&self, skill_dir: &Path, recovery_dir: &Path) -> Result<(), String> {
        copy_dir_all(skill_dir, recovery_dir)
    }

    fn read_registry(&self, home: &Path) -> Result<ForkRegistry, String> {
        read_fork_registry(home)
    }

    fn write_registry(
        &self,
        guard: &super::write_lease::WriteLeaseGuard,
        home: &Path,
        registry: &ForkRegistry,
    ) -> Result<(), String> {
        write_fork_registry_locked(guard, home, registry)
    }
}

fn fork_transaction_path_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "Failed to inspect fork transaction path {}: {error}",
            path.display()
        )),
    }
}

fn clear_fork_transaction_dir(
    storage: &dyn ForkTransactionStorage,
    path: &Path,
) -> Result<(), String> {
    if !fork_transaction_path_exists(path)? {
        return Ok(());
    }
    storage
        .remove_dir_all(path)
        .map_err(|error| format!("Failed to clear {}: {error}", path.display()))
}

fn quarantine_existing_live_recovery(
    storage: &dyn ForkTransactionStorage,
    recovery_dir: &Path,
    quarantine_dir: &Path,
) -> Result<Option<PathBuf>, String> {
    if fork_transaction_path_exists(quarantine_dir)? {
        return Err(format!(
            "Fork recovery requires attention: the quarantine path {} already exists. It was not replaced, and {} was not changed.",
            quarantine_dir.display(),
            recovery_dir.display()
        ));
    }
    if !fork_transaction_path_exists(recovery_dir)? {
        return Ok(None);
    }

    storage
        .rename_dir(recovery_dir, quarantine_dir)
        .map_err(|error| {
            format!(
                "Failed to quarantine the existing recovery copy from {} to {}: {error}. No fork changes were made; the recovery copy remains at {}.",
                recovery_dir.display(),
                quarantine_dir.display(),
                recovery_dir.display()
            )
        })?;
    Ok(Some(quarantine_dir.to_path_buf()))
}

fn restore_quarantined_live_recovery(
    storage: &dyn ForkTransactionStorage,
    recovery_dir: &Path,
    quarantine_dir: Option<&Path>,
) -> Result<(), String> {
    if let Err(error) = clear_fork_transaction_dir(storage, recovery_dir) {
        return Err(match quarantine_dir {
            Some(quarantine_dir) => format!(
                "Could not restore the previous recovery copy because the incomplete replacement at {} could not be cleared: {error}. The previous recovery remains at {}.",
                recovery_dir.display(),
                quarantine_dir.display()
            ),
            None => format!(
                "Could not clear the incomplete recovery copy at {}: {error}",
                recovery_dir.display()
            ),
        });
    }

    let Some(quarantine_dir) = quarantine_dir else {
        return Ok(());
    };
    storage
        .rename_dir(quarantine_dir, recovery_dir)
        .map_err(|error| {
            format!(
                "Could not restore the previous recovery copy from {} to {}: {error}. The previous recovery remains at {}.",
                quarantine_dir.display(),
                recovery_dir.display(),
                quarantine_dir.display()
            )
        })
}

struct ForkPreDetachPaths<'a> {
    home: &'a Path,
    base_dir: &'a Path,
    recovery_dir: &'a Path,
    quarantine_dir: Option<&'a Path>,
}

#[derive(Clone, Copy)]
enum ForkRecoveryRollback {
    RestorePrevious,
    KeepComplete,
}

fn rollback_fork_before_detach(
    storage: &dyn ForkTransactionStorage,
    guard: &super::write_lease::WriteLeaseGuard,
    primary_error: String,
    paths: &ForkPreDetachPaths<'_>,
    registry_before: Option<&ForkRegistry>,
    recovery: ForkRecoveryRollback,
) -> String {
    let mut rollback_errors = Vec::new();
    if let Some(registry_before) = registry_before {
        if let Err(error) = storage.write_registry(guard, paths.home, registry_before) {
            rollback_errors.push(format!("Failed to restore the fork registry: {error}"));
        }
    }
    if let Err(error) = clear_fork_transaction_dir(storage, paths.base_dir) {
        rollback_errors.push(error);
    }

    if matches!(recovery, ForkRecoveryRollback::KeepComplete) {
        rollback_errors.push(format!(
            "A complete live recovery copy remains at {}.",
            paths.recovery_dir.display()
        ));
        if let Some(quarantine_dir) = paths.quarantine_dir {
            rollback_errors.push(format!(
                "The previous recovery copy remains at {}.",
                quarantine_dir.display()
            ));
        }
    } else if let Err(error) =
        restore_quarantined_live_recovery(storage, paths.recovery_dir, paths.quarantine_dir)
    {
        rollback_errors.push(error);
    }

    if rollback_errors.is_empty() {
        primary_error
    } else {
        format!(
            "{primary_error} Recovery rollback needs attention: {}",
            rollback_errors.join(" ")
        )
    }
}

/// Requires `path` to canonicalize to `~/.agents/skills/<name>` (following
/// the whole-dir symlink Claude Code needs at `~/.claude/skills`), so
/// forking a same-named project or plugin deployment can't detach an
/// unrelated global skill.
fn validate_fork_path(home: &Path, name: &str, path: &Path) -> Result<(), String> {
    let canonical_given =
        fs::canonicalize(path).map_err(|e| format!("Failed to resolve {}: {e}", path.display()))?;
    let expected = home.join(".agents").join("skills").join(name);
    let canonical_expected = fs::canonicalize(&expected)
        .map_err(|e| format!("Failed to resolve {}: {e}", expected.display()))?;
    if canonical_given != canonical_expected {
        return Err(
            "Only the Universal-folder copy (~/.agents/skills/<name>) can be forked".to_string(),
        );
    }
    Ok(())
}

/// `fork_skill`'s logic, taking `home`/`app_data` directly and the traits as
/// fakeable dependencies, so it's testable without a Tauri `AppHandle` or a
/// network call.
///
/// Order matters: the snapshot fetched here is the upstream tree *at
/// `base_commit`*, not the current on-disk copy - a local edit made before
/// forking (e.g. one `dotagents sync` preserved) must still show up as a
/// diff against `base_commit` on the next Pull, not get silently treated as
/// "already synced". An earlier live recovery is quarantined until its
/// replacement is complete. The record and replacement recovery are written
/// before the ledger is touched, so a pre-detach failure keeps the skill
/// attached and restores the earlier recovery. The replacement recovery stays
/// available while ledger removal and live-tree restoration run.
#[allow(clippy::too_many_arguments)]
pub fn fork_skill_with(
    guard: &super::write_lease::WriteLeaseGuard,
    home: &Path,
    app_data: &Path,
    name: &str,
    path: &Path,
    ledger: &dyn LedgerTool,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
) -> Result<ForkRecord, String> {
    fork_skill_with_storage(
        guard,
        home,
        app_data,
        name,
        path,
        ledger,
        fetch,
        lookup,
        &FileForkTransactionStorage,
    )
}

#[allow(clippy::too_many_arguments)]
fn fork_skill_with_storage(
    guard: &super::write_lease::WriteLeaseGuard,
    home: &Path,
    app_data: &Path,
    name: &str,
    path: &Path,
    ledger: &dyn LedgerTool,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
    storage: &dyn ForkTransactionStorage,
) -> Result<ForkRecord, String> {
    validate_fork_path(home, name, path)?;

    let agents_dir = home.join(".agents");
    let skill_dir = agents_dir.join("skills").join(name);
    let origin = resolve_fork_origin(&agents_dir, app_data, name, lookup)?;

    let recovery_dir = fork_live_recovery_dir(app_data, name);
    let quarantine_dir = fork_live_recovery_quarantine_dir(app_data, name);
    let quarantined_recovery =
        quarantine_existing_live_recovery(storage, &recovery_dir, &quarantine_dir)?;

    // 1. Fetch the upstream tree at `base_commit` as the merge base - not a
    //    copy of the (possibly locally edited) live tree.
    let base_dir = fork_snapshot_dir(app_data, name);
    let rollback_paths = ForkPreDetachPaths {
        home,
        base_dir: &base_dir,
        recovery_dir: &recovery_dir,
        quarantine_dir: quarantined_recovery.as_deref(),
    };
    if let Err(error) = clear_fork_transaction_dir(storage, &base_dir) {
        return Err(rollback_fork_before_detach(
            storage,
            guard,
            format!("Failed to clear the stale snapshot for {name}: {error}"),
            &rollback_paths,
            None,
            ForkRecoveryRollback::RestorePrevious,
        ));
    }
    if let Err(error) =
        fetch.fetch_skill_dir(&origin.repo, &origin.path, &origin.base_commit, &base_dir)
    {
        return Err(rollback_fork_before_detach(
            storage,
            guard,
            format!(
                "Could not fetch {name}'s upstream copy at {}: {error}. Nothing was changed.",
                origin.base_commit
            ),
            &rollback_paths,
            None,
            ForkRecoveryRollback::RestorePrevious,
        ));
    }

    // 2. Write the record before touching the ledger - a failure here means
    //    the skill is still fully attached, never detached with no record.
    let record = ForkRecord {
        deployment_id: super::skill_deployment::deployment_id(
            name,
            "global",
            super::skill_deployment::SkillDestination::Universal,
            "universal",
            None,
            path,
        ),
        skill_dir: skill_dir.clone(),
        forked_at: chrono::Utc::now().to_rfc3339(),
        origin_tool: origin.tool,
        origin_source: origin.origin_source,
        repo: origin.repo,
        path: origin.path,
        declared_ref: origin.declared_ref,
        base_commit: origin.base_commit,
    };
    let registry_before = match storage.read_registry(home) {
        Ok(registry) => registry,
        Err(error) => {
            return Err(rollback_fork_before_detach(
                storage,
                guard,
                error,
                &rollback_paths,
                None,
                ForkRecoveryRollback::RestorePrevious,
            ));
        }
    };
    let mut registry = registry_before.clone();
    registry.forks.insert(name.to_string(), record.clone());
    if let Err(error) = storage.write_registry(guard, home, &registry) {
        return Err(rollback_fork_before_detach(
            storage,
            guard,
            error,
            &rollback_paths,
            None,
            ForkRecoveryRollback::RestorePrevious,
        ));
    }

    // 3. Snapshot the live tree as a recovery copy before removing it from
    //    the ledger, in case that removal wipes the directory.
    if let Err(error) = storage.snapshot_live_skill(&skill_dir, &recovery_dir) {
        return Err(rollback_fork_before_detach(
            storage,
            guard,
            format!("Failed to snapshot {name} before forking: {error}"),
            &rollback_paths,
            Some(&registry_before),
            ForkRecoveryRollback::RestorePrevious,
        ));
    }

    if let Some(quarantine_dir) = quarantined_recovery.as_deref() {
        if let Err(error) = storage.remove_dir_all(quarantine_dir) {
            return Err(rollback_fork_before_detach(
                storage,
                guard,
                format!(
                    "Failed to clear the previous recovery quarantine at {}: {error}",
                    quarantine_dir.display()
                ),
                &rollback_paths,
                Some(&registry_before),
                ForkRecoveryRollback::KeepComplete,
            ));
        }
    }

    // 4. Remove it from the owning ledger.
    let detached_rollback_paths = ForkPreDetachPaths {
        quarantine_dir: None,
        ..rollback_paths
    };
    if let Err(error) = ledger.remove(origin.tool, name) {
        return Err(rollback_fork_before_detach(
            storage,
            guard,
            error,
            &detached_rollback_paths,
            Some(&registry_before),
            ForkRecoveryRollback::KeepComplete,
        ));
    }

    // 5. If the ledger's removal wiped the folder, restore it from the
    //    recovery copy - the record is already durable, so on a restore
    //    failure keep it (it holds provenance) and name the recovery path.
    if !skill_dir.exists() {
        if let Err(e) = copy_dir_all(&recovery_dir, &skill_dir) {
            return Err(format!(
                "Removed {name} from its ledger, but could not restore it from the recovery copy at {}: {e}. Restore it manually from that path.",
                recovery_dir.display()
            ));
        }
    }
    let _ = fs::remove_dir_all(&recovery_dir);

    Ok(record)
}

#[tauri::command]
pub async fn fork_skill(
    target: super::skill_dto::LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<ForkRecord, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "fork_skill", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let write_lease = super::write_lease::WriteLease::default();
        let guard = write_lease.try_acquire(&home)?;
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("Could not resolve app data dir: {e}"))?;
        let lookup = resolve_lookup();
        let gh_bin = skill_update_check::resolve_gh_binary()
            .ok_or_else(|| "Run Check now first".to_string())?;
        let fetch = RealUpstreamFetch {
            gh_bin,
            cache_dir: app_data.join("skill-studio").join("cache"),
        };

        let resolved = super::skill_lifecycle::resolve_fresh_lifecycle_target(
            &app,
            &refresh_state,
            &target,
            "Fork",
        )?;
        let snapshot = resolved.snapshot;
        let id = target
            .deployment_id
            .as_deref()
            .ok_or("Fork needs one Global Universal deployment_id")?;
        if target.owner_id.is_some() {
            return Err(
                "Fork targets one Global Universal deployment, not an owner group".to_string(),
            );
        }
        let (skill, deployment) = super::skill_lifecycle::find_deployment(&snapshot, id)?;
        super::skill_lifecycle::revalidate_deployment(deployment, id)?;
        super::skill_lifecycle::require_direct_deployment_mutable(deployment, "Fork")?;
        super::skill_lifecycle::require_global_universal_park_target(deployment)
            .map_err(|_| "Fork is only available for the Global Universal folder.".to_string())?;

        let result = fork_skill_with(
            &guard,
            &home,
            &app_data,
            &skill.name,
            Path::new(&deployment.path),
            &RealLedgerTool,
            &fetch,
            lookup.as_ref(),
        );
        skill_refresh::request_snapshot_rebuild(&app);
        result
    })
    .await
}

// ============================================================================
// Pull upstream
// ============================================================================

/// What one `pull_fork_upstream` call did.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct PullResult {
    pub from_commit: String,
    pub to_commit: String,
    pub merged: Vec<String>,
    pub conflicts: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: usize,
    /// Set to "Already up to date" when `to_commit == from_commit`; `None`
    /// otherwise.
    pub message: Option<String>,
}

/// True when `bytes` contains a NUL byte - a binary file never gets
/// git-style text markers written into it.
fn is_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

/// Opens one or more paths in the user's editor. Real callers hand the user
/// something to look at; tests hand a recorder so a conflict's editor-open
/// can be asserted without actually launching an application.
pub trait EditorOpener {
    fn open_paths(&self, paths: &[PathBuf]) -> Result<(), String>;
}

/// The real opener: `pull_fork_upstream`'s only caller in production,
/// delegating to `skill_editor`'s "Open in editor" choice.
pub struct RealEditorOpener;

impl EditorOpener for RealEditorOpener {
    fn open_paths(&self, paths: &[PathBuf]) -> Result<(), String> {
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        super::skill_editor::open_paths_in_editor(&home, paths)
    }
}

/// Writes `mine` and `theirs` side by side in one file with git-style
/// conflict markers, the way `git merge-file` would report a conflicted
/// hunk - but built in-process rather than shelled out to `git`, since a
/// pull never merges automatically: any three-way divergence this deep
/// (base, mine, and theirs all differ) always needs the user, so there is
/// no "clean" case left to detect once we get here.
fn write_conflict_markers(mine: &[u8], theirs: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(mine.len() + theirs.len() + 32);
    out.extend_from_slice(b"<<<<<<< mine\n");
    out.extend_from_slice(mine);
    if !mine.is_empty() && !mine.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(b"=======\n");
    out.extend_from_slice(theirs);
    if !theirs.is_empty() && !theirs.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(b">>>>>>> theirs\n");
    out
}

/// Writes `bytes` at `root/rel`, creating parent directories as needed - the
/// staging-tree equivalent of what used to be an in-place write to the live
/// tree.
fn write_staged(root: &Path, rel: &str, bytes: &[u8]) -> Result<(), String> {
    let dest = root.join(rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    fs::write(&dest, bytes).map_err(|e| format!("Failed to write {}: {e}", dest.display()))
}

/// Renames `src` to `dst`, falling back to copy-then-remove when the rename
/// fails (e.g. across filesystems).
fn rename_or_copy(src: &Path, dst: &Path) -> Result<(), String> {
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_dir_all(src, dst)?;
    fs::remove_dir_all(src).map_err(|e| format!("Failed to remove {}: {e}", src.display()))
}

/// Atomically swaps the staged merge result into place: `mine_dir` becomes
/// `staging_live`, `base_dir` becomes `staging_base`, and the registry's
/// `base_commit` advances to `to_commit` - in that order, backing up the two
/// live directories first so any failure before the registry write can be
/// rolled back and reported without touching the on-disk live tree or base
/// beyond what's undone here.
#[allow(clippy::too_many_arguments)]
fn swap_in_pull_result(
    guard: &super::write_lease::WriteLeaseGuard,
    home: &Path,
    app_data: &Path,
    name: &str,
    mine_dir: &Path,
    base_dir: &Path,
    staging_live: &Path,
    staging_base: &Path,
    to_commit: &str,
    registry: &mut ForkRegistry,
) -> Result<(), String> {
    let scratch = app_data.join("skill-studio").join("forks").join(name);
    let live_backup = scratch.join("live-backup");
    let old_base_backup = scratch.join("old-base-backup");
    for backup in [&live_backup, &old_base_backup] {
        if backup.exists() {
            fs::remove_dir_all(backup)
                .map_err(|e| format!("Failed to clear {}: {e}", backup.display()))?;
        }
    }

    fs::rename(mine_dir, &live_backup)
        .map_err(|e| format!("Failed to back up the live tree of {name}: {e}"))?;

    if let Err(e) = rename_or_copy(staging_live, mine_dir) {
        let _ = rename_or_copy(&live_backup, mine_dir);
        let _ = fs::remove_dir_all(&live_backup);
        return Err(format!(
            "Failed to install the merged tree for {name}, rolled back the live tree: {e}"
        ));
    }

    if let Err(e) = fs::rename(base_dir, &old_base_backup) {
        let _ = fs::remove_dir_all(mine_dir);
        let _ = rename_or_copy(&live_backup, mine_dir);
        let _ = fs::remove_dir_all(&live_backup);
        return Err(format!(
            "Failed to back up {name}'s old base snapshot, rolled back the live tree: {e}"
        ));
    }

    if let Err(e) = rename_or_copy(staging_base, base_dir) {
        let _ = rename_or_copy(&old_base_backup, base_dir);
        let _ = fs::remove_dir_all(mine_dir);
        let _ = rename_or_copy(&live_backup, mine_dir);
        let _ = fs::remove_dir_all(&live_backup);
        return Err(format!(
            "Failed to install {name}'s new base snapshot, rolled back the live tree and base: {e}"
        ));
    }

    if let Some(rec) = registry.forks.get_mut(name) {
        rec.base_commit = to_commit.to_string();
    }
    if let Err(e) = write_fork_registry_locked(guard, home, registry) {
        let _ = fs::remove_dir_all(base_dir);
        let _ = rename_or_copy(&old_base_backup, base_dir);
        let _ = fs::remove_dir_all(mine_dir);
        let _ = rename_or_copy(&live_backup, mine_dir);
        let _ = fs::remove_dir_all(&live_backup);
        return Err(format!(
            "Failed to record {name}'s pull, rolled back the live tree and base: {e}"
        ));
    }

    let _ = fs::remove_dir_all(&live_backup);
    let _ = fs::remove_dir_all(&old_base_backup);
    Ok(())
}

/// `pull_fork_upstream`'s logic, taking `home`/`app_data` directly and the
/// two traits as fakeable dependencies.
///
/// The merged tree and the new base snapshot are built in scratch staging
/// directories, never touching `mine_dir`/`base_dir` directly, so any
/// failure while fetching, diffing, or merging leaves the live tree and the
/// old base exactly as they were - `swap_in_pull_result` is the only place
/// that mutates them, and it does so as close to atomically as the
/// filesystem allows.
pub fn pull_fork_upstream_with(
    guard: &super::write_lease::WriteLeaseGuard,
    home: &Path,
    app_data: &Path,
    name: &str,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
    editor: &dyn EditorOpener,
) -> Result<PullResult, String> {
    let mut registry = read_fork_registry(home)?;
    let record = registry
        .forks
        .get(name)
        .cloned()
        .ok_or_else(|| format!("`{name}` is not forked"))?;

    let store = skill_update_check::read_update_check_store(app_data);
    let owner_id = format!("owner:v1/global/{name}");
    let to_commit = match store
        .owners
        .get(&owner_id)
        .and_then(|state| state.latest_commit.clone())
    {
        Some(commit) => commit,
        None => match lookup.latest_commit(&record.repo, &record.path, None)? {
            Some((sha, _)) => sha,
            None => {
                return Err(format!(
                    "Could not determine {name}'s latest upstream commit"
                ))
            }
        },
    };

    if to_commit == record.base_commit {
        return Ok(PullResult {
            from_commit: record.base_commit,
            to_commit,
            message: Some("Already up to date".to_string()),
            ..Default::default()
        });
    }

    let mine_dir = if record.skill_dir.as_os_str().is_empty() {
        home.join(".agents").join("skills").join(name)
    } else {
        record.skill_dir.clone()
    };
    let base_dir = fork_snapshot_dir(app_data, name);
    let scratch = app_data.join("skill-studio").join("forks").join(name);
    let staging_live = scratch.join("staging-live");
    let staging_base = scratch.join("staging-base");
    for staging in [&staging_live, &staging_base] {
        if staging.exists() {
            fs::remove_dir_all(staging).map_err(|e| format!("Failed to clear scratch dir: {e}"))?;
        }
    }
    // The freshly fetched upstream tree doubles as both the "theirs" side of
    // the merge and (verbatim) the new base snapshot once the pull commits.
    fetch.fetch_skill_dir(&record.repo, &record.path, &to_commit, &staging_base)?;
    fs::create_dir_all(&staging_live)
        .map_err(|e| format!("Failed to create {}: {e}", staging_live.display()))?;
    let cleanup_staging = TempCleanup {
        paths: vec![staging_live.clone(), staging_base.clone()],
    };

    let mut all_paths: BTreeSet<String> = BTreeSet::new();
    collect_relative_files(&base_dir, &mut all_paths);
    collect_relative_files(&mine_dir, &mut all_paths);
    collect_relative_files(&staging_base, &mut all_paths);

    let mut result = PullResult {
        from_commit: record.base_commit.clone(),
        to_commit: to_commit.clone(),
        ..Default::default()
    };

    for rel in &all_paths {
        let base_bytes = fs::read(base_dir.join(rel)).ok();
        let mine_bytes = fs::read(mine_dir.join(rel)).ok();
        let theirs_bytes = fs::read(staging_base.join(rel)).ok();

        match (base_bytes, mine_bytes, theirs_bytes) {
            (None, None, Some(theirs)) => {
                write_staged(&staging_live, rel, &theirs)?;
                result.added.push(rel.clone());
            }
            (None, Some(mine), None) => {
                // Mine-only - added locally with no base or upstream copy -
                // carried forward untouched, and not counted as "unchanged"
                // since it was never compared to anything.
                write_staged(&staging_live, rel, &mine)?;
            }
            (Some(base), None, Some(theirs)) => {
                if base == theirs {
                    // Upstream never actually changed it - the local
                    // deletion wins, nothing to restore.
                } else {
                    // Upstream changed a file we deleted locally: restore it
                    // so the change isn't silently lost, but flag it.
                    write_staged(&staging_live, rel, &theirs)?;
                    result.conflicts.push(rel.clone());
                }
            }
            (Some(base), Some(mine), None) => {
                if base == mine {
                    result.removed.push(rel.clone());
                } else {
                    // Deleted upstream, but changed locally: keep mine and
                    // flag it.
                    write_staged(&staging_live, rel, &mine)?;
                    result.conflicts.push(rel.clone());
                }
            }
            (base, Some(mine), Some(theirs)) => {
                let base_eq_theirs = base.as_ref().is_some_and(|b| *b == theirs);
                let base_eq_mine = base.as_ref().is_some_and(|b| *b == mine);
                if mine == theirs {
                    write_staged(&staging_live, rel, &mine)?;
                    result.unchanged += 1;
                } else if base_eq_theirs {
                    // Mine changed, theirs didn't: keep mine as-is.
                    write_staged(&staging_live, rel, &mine)?;
                } else if base_eq_mine {
                    write_staged(&staging_live, rel, &theirs)?;
                    result.merged.push(rel.clone());
                } else {
                    let base_bytes = base.as_deref().unwrap_or(&[]);
                    if is_binary(&mine) || is_binary(base_bytes) || is_binary(&theirs) {
                        // Binary and all three differ: keep mine, flag it,
                        // never try to write text markers into it.
                        write_staged(&staging_live, rel, &mine)?;
                        result.conflicts.push(rel.clone());
                    } else {
                        // Text, and base, mine, and theirs all differ from
                        // each other: never merges - write git-style
                        // conflict markers and let the caller open the
                        // editor on it once it's swapped into place.
                        write_staged(&staging_live, rel, &write_conflict_markers(&mine, &theirs))?;
                        result.conflicts.push(rel.clone());
                    }
                }
            }
            // Deleted on both sides, or nothing anywhere: nothing to carry
            // into the merged tree.
            (Some(_) | None, None, None) => {}
        }
    }

    swap_in_pull_result(
        guard,
        home,
        app_data,
        name,
        &mine_dir,
        &base_dir,
        &staging_live,
        &staging_base,
        &to_commit,
        &mut registry,
    )?;
    drop(cleanup_staging);

    if !result.conflicts.is_empty() {
        // The markers are already on disk under `mine_dir`, and
        // `swap_in_pull_result` above already committed the registry and
        // swapped the marker file in - the pull itself is done. A failed
        // editor launch must not turn a completed pull into an `Err` (that
        // would discard `result.conflicts`, the only place the caller
        // learns markers are in SKILL.md); it's reported as a message on
        // the still-`Ok` result instead.
        let conflict_paths: Vec<PathBuf> = result
            .conflicts
            .iter()
            .map(|rel| mine_dir.join(rel))
            .collect();
        if let Err(e) = editor.open_paths(&conflict_paths) {
            let rel = &result.conflicts[0];
            result.message = Some(format!(
                "Conflict markers written to {rel}; could not open editor: {e}"
            ));
        }
    }

    Ok(result)
}

#[tauri::command]
pub async fn pull_fork_upstream(
    target: super::skill_dto::LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<PullResult, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "pull_fork_upstream", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let write_lease = super::write_lease::WriteLease::default();
        let guard = write_lease.try_acquire(&home)?;
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("Could not resolve app data dir: {e}"))?;
        let lookup = resolve_lookup();
        let fetch = RealUpstreamFetch {
            gh_bin: skill_update_check::resolve_gh_binary()
                .ok_or_else(|| "Run Check now first".to_string())?,
            cache_dir: app_data.join("skill-studio").join("cache"),
        };

        let resolved = super::skill_lifecycle::resolve_fresh_lifecycle_target(
            &app,
            &refresh_state,
            &target,
            "Pull upstream",
        )?;
        let (name, _) = resolve_recorded_fork_target(&resolved.snapshot, &target, &home)?;
        let result = pull_fork_upstream_with(
            &guard,
            &home,
            &app_data,
            &name,
            &fetch,
            lookup.as_ref(),
            &RealEditorOpener,
        );
        skill_refresh::request_snapshot_rebuild(&app);
        result
    })
    .await
}

// ============================================================================
// Un-fork
// ============================================================================

/// `unfork_skill`'s logic, taking `home`/`app_data` and the trait directly.
pub fn unfork_skill_with(
    guard: &super::write_lease::WriteLeaseGuard,
    home: &Path,
    app_data: &Path,
    name: &str,
    ledger: &dyn LedgerTool,
) -> Result<(), String> {
    let mut registry = read_fork_registry(home)?;
    let record = registry
        .forks
        .get(name)
        .cloned()
        .ok_or_else(|| format!("`{name}` is not forked"))?;

    ledger.reinstall(&record, name)?;

    registry.forks.remove(name);
    write_fork_registry_locked(guard, home, &registry)?;
    let _ = fs::remove_dir_all(fork_snapshot_dir(app_data, name));
    Ok(())
}

#[tauri::command]
pub async fn unfork_skill(
    target: super::skill_dto::LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "unfork_skill", move || {
        let refresh_state = app.state::<SkillRefreshState>();
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let write_lease = super::write_lease::WriteLease::default();
        let guard = write_lease.try_acquire(&home)?;
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("Could not resolve app data dir: {e}"))?;

        let resolved = super::skill_lifecycle::resolve_fresh_lifecycle_target(
            &app,
            &refresh_state,
            &target,
            "Unfork",
        )?;
        let (name, _) = resolve_recorded_fork_target(&resolved.snapshot, &target, &home)?;
        let result = unfork_skill_with(&guard, &home, &app_data, &name, &RealLedgerTool);
        skill_refresh::request_snapshot_rebuild(&app);
        result
    })
    .await
}

fn resolve_recorded_fork_target(
    snapshot: &super::skill_refresh::SkillSnapshot,
    target: &super::skill_dto::LifecycleTarget,
    home: &Path,
) -> Result<(String, ForkRecord), String> {
    let id = target
        .deployment_id
        .as_deref()
        .ok_or("Fork lifecycle needs one Global Universal deployment_id")?;
    if target.owner_id.is_some() {
        return Err(
            "Fork lifecycle targets one Global Universal deployment, not an owner group"
                .to_string(),
        );
    }
    let (skill, deployment) = super::skill_lifecycle::find_deployment(snapshot, id)?;
    super::skill_lifecycle::revalidate_deployment(deployment, id)?;
    super::skill_lifecycle::require_global_universal_park_target(deployment).map_err(|_| {
        "Fork lifecycle is only available for the Global Universal folder.".to_string()
    })?;
    let registry = read_fork_registry(home)?;
    let record = registry
        .forks
        .get(&skill.name)
        .cloned()
        .ok_or_else(|| format!("`{}` is not forked", skill.name))?;
    let expected_path = if record.skill_dir.as_os_str().is_empty() {
        home.join(".agents/skills").join(&skill.name)
    } else {
        record.skill_dir.clone()
    };
    if (!record.deployment_id.is_empty() && record.deployment_id != id)
        || Path::new(&deployment.path) != expected_path
    {
        return Err(
            "The fork record does not belong to the selected Global Universal deployment"
                .to_string(),
        );
    }
    Ok((skill.name.clone(), record))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::super::skill_fork_registry::write_fork_registry;
    use super::*;
    use std::sync::Mutex;

    fn test_guard(home: &Path) -> super::super::write_lease::WriteLeaseGuard {
        super::super::write_lease::WriteLease::default()
            .try_acquire(home)
            .unwrap()
    }

    // Moved from `skill_install_plan.rs` (unit 3.5c) alongside
    // `skills_sh_universal_add_args` itself.
    fn universal_global() -> SkillInstallSpec {
        SkillInstallSpec {
            scope: InstallScope::Global,
            destination: SkillDestination::Universal,
            project_path: None,
            harnesses: vec![],
        }
    }

    #[test]
    fn universal_skills_sh_uses_agent_universal_and_global_or_names_the_wrong_argv() {
        let (argv, cwd) =
            skills_sh_universal_add_args("o/r", Some("find-bugs"), &universal_global()).unwrap();
        assert_eq!(
            argv,
            vec![
                "skills",
                "add",
                "o/r",
                "--yes",
                "--global",
                "--skill",
                "find-bugs",
                "--agent",
                "universal",
            ]
        );
        assert!(!argv.iter().any(|a| a == "codex"));
        assert_eq!(cwd, None);
    }

    #[test]
    fn universal_skills_sh_may_add_claude_code_not_codex_or_names_the_missing_agent() {
        let mut spec = universal_global();
        spec.harnesses = vec![AgentId::ClaudeCode];
        let (argv, _cwd) = skills_sh_universal_add_args("o/r", None, &spec).unwrap();
        assert!(argv.windows(2).any(|w| w == ["--agent", "universal"]));
        assert!(argv.windows(2).any(|w| w == ["--agent", "claude-code"]));
        assert!(!argv.iter().any(|a| a == "codex"));
    }

    #[test]
    fn universal_skills_sh_ignores_direct_readers_or_names_the_leaked_agent() {
        let mut spec = universal_global();
        spec.harnesses = vec![AgentId::Codex];
        let (argv, _cwd) = skills_sh_universal_add_args("o/r", None, &spec).unwrap();
        assert!(!argv.iter().any(|arg| arg == "codex"));
    }

    /// `skills@1.7.0` has no `--cwd` flag: a project scope must carry the
    /// project path as the process cwd, not as an argv token, or the CLI
    /// writes into whatever directory the process happened to start in.
    #[test]
    fn project_universal_runs_in_project_dir_not_via_cwd_flag_or_names_the_wrong_scope() {
        let spec = SkillInstallSpec {
            scope: InstallScope::Project,
            destination: SkillDestination::Universal,
            project_path: Some("/work/app".to_string()),
            harnesses: vec![],
        };
        let (argv, cwd) = skills_sh_universal_add_args("o/r", None, &spec).unwrap();
        assert!(!argv.contains(&"--cwd".to_string()));
        assert!(!argv.contains(&"/work/app".to_string()));
        assert!(!argv.contains(&"--global".to_string()));
        assert_eq!(cwd, Some(PathBuf::from("/work/app")));
    }

    /// Records every `remove`/`reinstall` call so tests can assert "called
    /// once with the right `OriginTool`" without shelling out to `npx`.
    #[derive(Default)]
    struct FakeLedger {
        remove_calls: Mutex<Vec<(OriginTool, String)>>,
        reinstall_calls: Mutex<Vec<(ForkRecord, String)>>,
        remove_result: Mutex<Option<Result<(), String>>>,
    }

    impl FakeLedger {
        fn failing_remove(message: &str) -> Self {
            Self {
                remove_result: Mutex::new(Some(Err(message.to_string()))),
                ..Default::default()
            }
        }
    }

    impl LedgerTool for FakeLedger {
        fn remove(&self, tool: OriginTool, name: &str) -> Result<(), String> {
            self.remove_calls
                .lock()
                .unwrap()
                .push((tool, name.to_string()));
            self.remove_result.lock().unwrap().take().unwrap_or(Ok(()))
        }
        fn reinstall(&self, rec: &ForkRecord, name: &str) -> Result<(), String> {
            self.reinstall_calls
                .lock()
                .unwrap()
                .push((rec.clone(), name.to_string()));
            Ok(())
        }
    }

    /// A `CommitLookup` that never expects to be called - fork/pull tests
    /// that already have a cached baseline in the update-check store must
    /// not need it.
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

    /// An `EditorOpener` for tests that don't exercise a conflict: asserts
    /// it is never asked to open anything, the same guarantee
    /// `NeverCalledLookup` gives the commit lookup port.
    struct NoopEditorOpener;
    impl EditorOpener for NoopEditorOpener {
        fn open_paths(&self, paths: &[PathBuf]) -> Result<(), String> {
            assert!(paths.is_empty(), "unexpected editor open: {paths:?}");
            Ok(())
        }
    }

    /// Records every call so a conflict test can assert the editor opened
    /// on exactly the merged file's live path.
    #[derive(Default)]
    struct RecordingEditorOpener {
        opened: std::sync::Mutex<Vec<Vec<PathBuf>>>,
    }
    impl EditorOpener for RecordingEditorOpener {
        fn open_paths(&self, paths: &[PathBuf]) -> Result<(), String> {
            self.opened.lock().unwrap().push(paths.to_vec());
            Ok(())
        }
    }

    /// An `EditorOpener` that always fails, for the F2 regression: a
    /// failed editor launch must not turn a completed pull into an `Err`.
    struct FailingEditorOpener;
    impl EditorOpener for FailingEditorOpener {
        fn open_paths(&self, _paths: &[PathBuf]) -> Result<(), String> {
            Err("no editor configured".to_string())
        }
    }

    /// An `UpstreamFetch` that never expects to be called - refusal tests
    /// (a wildcard/manual source) must fail before ever reaching a fetch.
    struct NeverCalledFetch;
    impl UpstreamFetch for NeverCalledFetch {
        fn fetch_skill_dir(&self, _: &str, _: &str, _: &str, _: &Path) -> Result<(), String> {
            panic!("fetch should not have been called");
        }
    }

    /// A fake `UpstreamFetch` that writes canned file contents, regardless
    /// of the requested commit - good enough for tests that only care about
    /// one commit's tree at a time.
    struct FakeFetch {
        files: Vec<(&'static str, &'static str)>,
    }
    impl UpstreamFetch for FakeFetch {
        fn fetch_skill_dir(
            &self,
            _repo: &str,
            _path: &str,
            _commit: &str,
            into: &Path,
        ) -> Result<(), String> {
            for (name, content) in &self.files {
                write_file(&into.join(name), content);
            }
            Ok(())
        }
    }

    struct FailingFetch;

    impl UpstreamFetch for FailingFetch {
        fn fetch_skill_dir(
            &self,
            _repo: &str,
            _path: &str,
            _commit: &str,
            into: &Path,
        ) -> Result<(), String> {
            write_file(&into.join("partial.txt"), "incomplete fetch");
            Err("injected fetch failure".to_string())
        }
    }

    #[derive(Default)]
    struct InjectedForkTransactionStorage {
        fail_rename_to: Option<PathBuf>,
        fail_rename_from: Option<PathBuf>,
        fail_remove: Option<PathBuf>,
        fail_snapshot: bool,
        fail_registry_read: bool,
        fail_registry_write_call: Option<usize>,
        registry_write_calls: Mutex<usize>,
    }

    impl ForkTransactionStorage for InjectedForkTransactionStorage {
        fn rename_dir(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            if self.fail_rename_to.as_deref() == Some(to)
                || self.fail_rename_from.as_deref() == Some(from)
            {
                return Err(std::io::Error::other("injected quarantine rename failure"));
            }
            fs::rename(from, to)
        }

        fn remove_dir_all(&self, path: &Path) -> std::io::Result<()> {
            if self.fail_remove.as_deref() == Some(path) {
                return Err(std::io::Error::other("injected quarantine cleanup failure"));
            }
            fs::remove_dir_all(path)
        }

        fn snapshot_live_skill(&self, skill_dir: &Path, recovery_dir: &Path) -> Result<(), String> {
            if self.fail_snapshot {
                write_file(&recovery_dir.join("partial.txt"), "incomplete snapshot");
                return Err("injected live snapshot failure".to_string());
            }
            copy_dir_all(skill_dir, recovery_dir)
        }

        fn read_registry(&self, home: &Path) -> Result<ForkRegistry, String> {
            if self.fail_registry_read {
                return Err("injected registry read failure".to_string());
            }
            read_fork_registry(home)
        }

        fn write_registry(
            &self,
            guard: &super::super::write_lease::WriteLeaseGuard,
            home: &Path,
            registry: &ForkRegistry,
        ) -> Result<(), String> {
            let mut calls = self.registry_write_calls.lock().unwrap();
            *calls += 1;
            if self.fail_registry_write_call == Some(*calls) {
                return Err("injected registry write failure".to_string());
            }
            write_fork_registry_locked(guard, home, registry)
        }
    }

    /// Compares two serialized `ForkRegistry` files ignoring `write_version`,
    /// which `write_fork_registry` bumps on every write - including a
    /// rollback that restores otherwise-identical content, per
    /// `skill_studio_core::registry::write_registry_document`.
    fn assert_registry_content_unchanged(after: &[u8], before: &[u8]) {
        let strip_write_version = |bytes: &[u8]| {
            let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            value.as_object_mut().unwrap().remove("write_version");
            value
        };
        assert_eq!(strip_write_version(after), strip_write_version(before));
    }

    fn write_file(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn seed_dotagents_ledger(home: &Path, name: &str, source: &str, path: &str, commit: &str) {
        let agents = home.join(".agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("agents.lock"),
            format!(
                "[skills.{name}]\nsource = \"{source}\"\nresolved_path = \"{path}\"\nresolved_commit = \"{commit}\"\n"
            ),
        )
        .unwrap();
        fs::write(
            agents.join("agents.toml"),
            format!("[[skills]]\nname = \"{name}\"\nsource = \"{source}\"\npath = \"{path}\"\n"),
        )
        .unwrap();
    }

    fn seed_wildcard_dotagents_ledger(home: &Path, name: &str, source: &str) {
        let agents = home.join(".agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("agents.lock"),
            format!(
                "[skills.{name}]\nsource = \"{source}\"\nresolved_path = \"skills/{name}\"\nresolved_commit = \"{}\"\n",
                "a".repeat(40)
            ),
        )
        .unwrap();
        // No agents.toml row for this name - the wildcard case.
    }

    #[test]
    fn fork_happy_path_snapshots_removes_restores_and_records() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        write_file(
            &home.join(".agents/skills/find-bugs/SKILL.md"),
            "---\nname: find-bugs\n---\nbody",
        );

        let ledger = FakeLedger::default();
        let fetch = FakeFetch {
            files: vec![("SKILL.md", "---\nname: find-bugs\n---\nupstream body")],
        };
        let record = fork_skill_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &fetch,
            &NeverCalledLookup,
        )
        .unwrap();

        assert_eq!(record.origin_tool, OriginTool::Dotagents);
        assert_eq!(record.base_commit, "a".repeat(40));
        assert_eq!(ledger.remove_calls.lock().unwrap().len(), 1);
        assert_eq!(
            ledger.remove_calls.lock().unwrap()[0].0,
            OriginTool::Dotagents
        );

        // The skill directory still exists (the fake "removed" it from the
        // ledger without touching the folder, same as a real dotagents
        // remove that only deletes the manifest row for a plain folder
        // adoption scenario - fork_skill's restore step is a no-op here).
        assert!(home.join(".agents/skills/find-bugs/SKILL.md").exists());

        let registry = read_fork_registry(&home).unwrap();
        assert!(registry.forks.contains_key("find-bugs"));
    }

    /// Finding 1: the base snapshot must be the upstream tree fetched at
    /// `base_commit`, not a copy of the (possibly locally edited) live tree,
    /// otherwise a local edit made before forking would be treated as
    /// "already synced" and silently overwritten on the next Pull.
    #[test]
    fn fork_snapshots_upstream_base_not_the_live_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        let base_commit = "a".repeat(40);
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &base_commit,
        );
        // A local edit made before forking (e.g. `dotagents sync` preserved
        // it), diverging from what's actually at `base_commit` upstream.
        write_file(
            &home.join(".agents/skills/find-bugs/SKILL.md"),
            "line one\nmine edit\n",
        );

        let ledger = FakeLedger::default();
        let fetch_at_fork = FakeFetch {
            files: vec![("SKILL.md", "line one\nbase line\n")],
        };
        fork_skill_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &fetch_at_fork,
            &NeverCalledLookup,
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(fork_snapshot_dir(&app_data, "find-bugs").join("SKILL.md")).unwrap(),
            "line one\nbase line\n"
        );
        assert_eq!(
            fs::read_to_string(home.join(".agents/skills/find-bugs/SKILL.md")).unwrap(),
            "line one\nmine edit\n"
        );

        // Upstream moved on and touched the same line the local edit did:
        // Pull must report a conflict, not silently take theirs.
        seed_update_check_latest(&app_data, "find-bugs", &"b".repeat(40));
        let fetch_at_pull = FakeFetch {
            files: vec![("SKILL.md", "line one\ntheirs edit\n")],
        };
        let editor = RecordingEditorOpener::default();
        let result = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch_at_pull,
            &NeverCalledLookup,
            &editor,
        )
        .unwrap();
        assert_eq!(result.conflicts, vec!["SKILL.md".to_string()]);
        assert!(
            !editor.opened.lock().unwrap().is_empty(),
            "expected the conflict to open the editor"
        );
    }

    /// F2 (unit 3.7b review round 1): `swap_in_pull_result` already
    /// committed the registry and swapped the marker file in by the time
    /// the editor is asked to open - a failing opener must keep that
    /// result and name it in `message`, not discard it as an `Err`.
    #[test]
    fn fork_pull_conflict_with_a_failing_editor_keeps_the_markers_and_names_them_or_names_the_lost_result(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        let base_commit = "a".repeat(40);
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &base_commit,
        );
        write_file(
            &home.join(".agents/skills/find-bugs/SKILL.md"),
            "line one\nmine edit\n",
        );

        let ledger = FakeLedger::default();
        let fetch_at_fork = FakeFetch {
            files: vec![("SKILL.md", "line one\nbase line\n")],
        };
        fork_skill_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &fetch_at_fork,
            &NeverCalledLookup,
        )
        .unwrap();

        seed_update_check_latest(&app_data, "find-bugs", &"b".repeat(40));
        let fetch_at_pull = FakeFetch {
            files: vec![("SKILL.md", "line one\ntheirs edit\n")],
        };
        let result = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch_at_pull,
            &NeverCalledLookup,
            &FailingEditorOpener,
        )
        .expect("a failed editor open must not turn a completed pull into an Err");

        assert_eq!(result.conflicts, vec!["SKILL.md".to_string()]);
        let message = result.message.expect("failed editor open must be named");
        assert!(message.contains("SKILL.md"), "{message}");
        assert!(message.contains("no editor configured"), "{message}");
        // The markers are on disk regardless of whether the editor opened.
        let on_disk = fs::read_to_string(home.join(".agents/skills/find-bugs/SKILL.md")).unwrap();
        assert!(on_disk.contains("<<<<<<<"), "{on_disk}");
    }

    /// Finding 7: forking a same-named copy that isn't the shared folder
    /// must be refused, not silently detach the unrelated global skill.
    #[test]
    fn fork_is_refused_for_a_path_outside_the_shared_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        write_file(&home.join(".agents/skills/find-bugs/SKILL.md"), "body");
        // A same-named project-scoped deployment - not the shared folder.
        write_file(
            &home.join("project/.claude/skills/find-bugs/SKILL.md"),
            "body",
        );

        let ledger = FakeLedger::default();
        let err = fork_skill_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join("project/.claude/skills/find-bugs"),
            &ledger,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();
        assert!(err.contains("Universal-folder"));
        assert_eq!(ledger.remove_calls.lock().unwrap().len(), 0);
    }

    /// Finding 7: `~/.claude/skills` is a whole-dir symlink to
    /// `~/.agents/skills` - forking through it must canonicalize to the same
    /// target and be accepted.
    #[test]
    fn fork_accepts_the_claude_code_symlink_to_the_shared_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        write_file(&home.join(".agents/skills/find-bugs/SKILL.md"), "body");
        fs::create_dir_all(home.join(".claude")).unwrap();
        std::os::unix::fs::symlink(home.join(".agents/skills"), home.join(".claude/skills"))
            .unwrap();

        let ledger = FakeLedger::default();
        let fetch = FakeFetch {
            files: vec![("SKILL.md", "body")],
        };
        let record = fork_skill_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".claude/skills/find-bugs"),
            &ledger,
            &fetch,
            &NeverCalledLookup,
        )
        .unwrap();
        assert_eq!(record.base_commit, "a".repeat(40));
    }

    #[test]
    fn fork_restores_the_folder_when_removal_deleted_it() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        let skill_md = home.join(".agents/skills/find-bugs/SKILL.md");
        write_file(&skill_md, "original body");

        // A ledger tool whose `remove` actually deletes the directory, like
        // a real `dotagents remove` / `npx skills remove` would.
        struct DeletingLedger {
            skill_dir: PathBuf,
        }
        impl LedgerTool for DeletingLedger {
            fn remove(&self, _tool: OriginTool, _name: &str) -> Result<(), String> {
                fs::remove_dir_all(&self.skill_dir).unwrap();
                Ok(())
            }
            fn reinstall(&self, _rec: &ForkRecord, _name: &str) -> Result<(), String> {
                Ok(())
            }
        }
        let ledger = DeletingLedger {
            skill_dir: home.join(".agents/skills/find-bugs"),
        };

        let fetch = FakeFetch {
            files: vec![("SKILL.md", "upstream body")],
        };
        fork_skill_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &fetch,
            &NeverCalledLookup,
        )
        .unwrap();
        // Restored from the live-tree recovery copy, not the upstream base.
        assert_eq!(fs::read_to_string(&skill_md).unwrap(), "original body");
    }

    #[test]
    fn fork_is_refused_for_a_wildcard_dotagents_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_wildcard_dotagents_ledger(&home, "find-bugs", "getsentry/some-repo");
        write_file(&home.join(".agents/skills/find-bugs/SKILL.md"), "body");

        let ledger = FakeLedger::default();
        let err = fork_skill_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();
        assert!(err.contains("wildcard source"));
        assert_eq!(ledger.remove_calls.lock().unwrap().len(), 0);
    }

    #[test]
    fn fork_is_refused_for_a_manual_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        write_file(&home.join(".agents/skills/my-notes/SKILL.md"), "body");

        let ledger = FakeLedger::default();
        let err = fork_skill_with(
            &test_guard(&home),
            &home,
            &app_data,
            "my-notes",
            &home.join(".agents/skills/my-notes"),
            &ledger,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();
        assert!(err.contains("not managed by dotagents or skills.sh"));
    }

    /// A CLI-remove failure leaves no record or base snapshot, but keeps the
    /// live recovery because a failed CLI can still have removed the folder.
    #[test]
    fn fork_remove_failure_keeps_recovery_but_no_base_or_record() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        write_file(&home.join(".agents/skills/find-bugs/SKILL.md"), "body");

        let ledger = FakeLedger::failing_remove("npx failed");
        let fetch = FakeFetch {
            files: vec![("SKILL.md", "upstream body")],
        };
        let err = fork_skill_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &fetch,
            &NeverCalledLookup,
        )
        .unwrap_err();
        assert!(err.contains("npx failed"), "{err}");
        assert!(err.contains("live-recovery"), "{err}");

        assert!(!fork_snapshot_dir(&app_data, "find-bugs").exists());
        assert_eq!(
            fs::read_to_string(fork_live_recovery_dir(&app_data, "find-bugs").join("SKILL.md"))
                .unwrap(),
            "body"
        );
        assert!(!read_fork_registry(&home)
            .unwrap()
            .forks
            .contains_key("find-bugs"));
        assert_eq!(
            fs::read_to_string(home.join(".agents/skills/find-bugs/SKILL.md")).unwrap(),
            "body"
        );
    }

    #[test]
    fn stale_recovery_quarantine_failure_happens_before_registry_or_ledger_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        let skill_md = home.join(".agents/skills/find-bugs/SKILL.md");
        write_file(&skill_md, "live body");
        let recovery_dir = fork_live_recovery_dir(&app_data, "find-bugs");
        write_file(&recovery_dir.join("stale.txt"), "stale recovery");
        let quarantine_dir = fork_live_recovery_quarantine_dir(&app_data, "find-bugs");

        let mut registry = ForkRegistry {
            server_url: Some("https://registry.example.test".to_string()),
            ..ForkRegistry::default()
        };
        registry
            .trusted_dotagents_sources
            .insert("owner/repo".to_string());
        write_fork_registry(&home, &registry).unwrap();
        let registry_before =
            fs::read(super::super::skill_fork_registry::fork_registry_path(&home)).unwrap();
        let agents_toml_before = fs::read(home.join(".agents/agents.toml")).unwrap();
        let agents_lock_before = fs::read(home.join(".agents/agents.lock")).unwrap();
        let ledger = FakeLedger::default();
        let storage = InjectedForkTransactionStorage {
            fail_rename_to: Some(quarantine_dir.clone()),
            ..Default::default()
        };
        let error = fork_skill_with_storage(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &NeverCalledFetch,
            &NeverCalledLookup,
            &storage,
        )
        .unwrap_err();

        assert!(
            error.contains("injected quarantine rename failure"),
            "{error}"
        );
        assert!(
            error.contains(&recovery_dir.display().to_string()),
            "{error}"
        );
        assert!(skill_md.is_file());
        assert_eq!(fs::read_to_string(skill_md).unwrap(), "live body");
        assert_eq!(ledger.remove_calls.lock().unwrap().len(), 0);
        assert_eq!(
            fs::read(super::super::skill_fork_registry::fork_registry_path(&home,)).unwrap(),
            registry_before
        );
        assert_eq!(
            fs::read(home.join(".agents/agents.toml")).unwrap(),
            agents_toml_before
        );
        assert_eq!(
            fs::read(home.join(".agents/agents.lock")).unwrap(),
            agents_lock_before
        );
        assert!(!fork_snapshot_dir(&app_data, "find-bugs").exists());
        assert_eq!(
            fs::read_to_string(recovery_dir.join("stale.txt")).unwrap(),
            "stale recovery"
        );
        assert!(!quarantine_dir.exists());
    }

    #[test]
    fn fetch_failure_restores_the_quarantined_recovery_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        let skill_md = home.join(".agents/skills/find-bugs/SKILL.md");
        write_file(&skill_md, "live body");
        let recovery_dir = fork_live_recovery_dir(&app_data, "find-bugs");
        write_file(&recovery_dir.join("stale.txt"), "stale recovery");

        let ledger = FakeLedger::default();
        let error = fork_skill_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &FailingFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();

        assert!(error.contains("injected fetch failure"), "{error}");
        assert_eq!(fs::read_to_string(skill_md).unwrap(), "live body");
        assert_eq!(ledger.remove_calls.lock().unwrap().len(), 0);
        assert_eq!(
            fs::read_to_string(recovery_dir.join("stale.txt")).unwrap(),
            "stale recovery"
        );
        assert!(!fork_live_recovery_quarantine_dir(&app_data, "find-bugs").exists());
        assert!(!fork_snapshot_dir(&app_data, "find-bugs").exists());
        assert!(!read_fork_registry(&home)
            .unwrap()
            .forks
            .contains_key("find-bugs"));
    }

    #[test]
    fn registry_write_failure_restores_the_quarantined_recovery_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        let skill_md = home.join(".agents/skills/find-bugs/SKILL.md");
        write_file(&skill_md, "live body");
        let recovery_dir = fork_live_recovery_dir(&app_data, "find-bugs");
        write_file(&recovery_dir.join("stale.txt"), "stale recovery");
        let registry = ForkRegistry {
            server_url: Some("https://registry.example.test".to_string()),
            ..ForkRegistry::default()
        };
        write_fork_registry(&home, &registry).unwrap();
        let registry_path = super::super::skill_fork_registry::fork_registry_path(&home);
        let registry_before = fs::read(&registry_path).unwrap();
        let storage = InjectedForkTransactionStorage {
            fail_registry_write_call: Some(1),
            ..Default::default()
        };
        let fetch = FakeFetch {
            files: vec![("SKILL.md", "upstream body")],
        };
        let ledger = FakeLedger::default();

        let error = fork_skill_with_storage(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &fetch,
            &NeverCalledLookup,
            &storage,
        )
        .unwrap_err();

        assert!(error.contains("injected registry write failure"), "{error}");
        assert_eq!(fs::read(&registry_path).unwrap(), registry_before);
        assert_eq!(fs::read_to_string(skill_md).unwrap(), "live body");
        assert_eq!(ledger.remove_calls.lock().unwrap().len(), 0);
        assert_eq!(
            fs::read_to_string(recovery_dir.join("stale.txt")).unwrap(),
            "stale recovery"
        );
        assert!(!fork_live_recovery_quarantine_dir(&app_data, "find-bugs").exists());
        assert!(!fork_snapshot_dir(&app_data, "find-bugs").exists());
    }

    #[test]
    fn registry_read_failure_restores_the_quarantined_recovery_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        let skill_md = home.join(".agents/skills/find-bugs/SKILL.md");
        write_file(&skill_md, "live body");
        let recovery_dir = fork_live_recovery_dir(&app_data, "find-bugs");
        write_file(&recovery_dir.join("stale.txt"), "stale recovery");
        let storage = InjectedForkTransactionStorage {
            fail_registry_read: true,
            ..Default::default()
        };
        let fetch = FakeFetch {
            files: vec![("SKILL.md", "upstream body")],
        };
        let ledger = FakeLedger::default();

        let error = fork_skill_with_storage(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &fetch,
            &NeverCalledLookup,
            &storage,
        )
        .unwrap_err();

        assert!(error.contains("injected registry read failure"), "{error}");
        assert_eq!(fs::read_to_string(skill_md).unwrap(), "live body");
        assert_eq!(ledger.remove_calls.lock().unwrap().len(), 0);
        assert_eq!(
            fs::read_to_string(recovery_dir.join("stale.txt")).unwrap(),
            "stale recovery"
        );
        assert!(!fork_live_recovery_quarantine_dir(&app_data, "find-bugs").exists());
        assert!(!fork_snapshot_dir(&app_data, "find-bugs").exists());
    }

    #[test]
    fn live_snapshot_failure_restores_registry_and_quarantined_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        let skill_md = home.join(".agents/skills/find-bugs/SKILL.md");
        write_file(&skill_md, "live body");
        let recovery_dir = fork_live_recovery_dir(&app_data, "find-bugs");
        write_file(&recovery_dir.join("stale.txt"), "stale recovery");
        let registry = ForkRegistry {
            server_url: Some("https://registry.example.test".to_string()),
            ..ForkRegistry::default()
        };
        write_fork_registry(&home, &registry).unwrap();
        let registry_path = super::super::skill_fork_registry::fork_registry_path(&home);
        let registry_before = fs::read(&registry_path).unwrap();
        let storage = InjectedForkTransactionStorage {
            fail_snapshot: true,
            ..Default::default()
        };
        let fetch = FakeFetch {
            files: vec![("SKILL.md", "upstream body")],
        };
        let ledger = FakeLedger::default();

        let error = fork_skill_with_storage(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &fetch,
            &NeverCalledLookup,
            &storage,
        )
        .unwrap_err();

        assert!(error.contains("injected live snapshot failure"), "{error}");
        assert_registry_content_unchanged(&fs::read(&registry_path).unwrap(), &registry_before);
        assert_eq!(fs::read_to_string(skill_md).unwrap(), "live body");
        assert_eq!(ledger.remove_calls.lock().unwrap().len(), 0);
        assert_eq!(
            fs::read_to_string(recovery_dir.join("stale.txt")).unwrap(),
            "stale recovery"
        );
        assert!(!fork_live_recovery_quarantine_dir(&app_data, "find-bugs").exists());
        assert!(!fork_snapshot_dir(&app_data, "find-bugs").exists());
    }

    #[test]
    fn quarantine_cleanup_failure_keeps_complete_and_previous_recovery_copies() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        let skill_md = home.join(".agents/skills/find-bugs/SKILL.md");
        write_file(&skill_md, "live body");
        let recovery_dir = fork_live_recovery_dir(&app_data, "find-bugs");
        let quarantine_dir = fork_live_recovery_quarantine_dir(&app_data, "find-bugs");
        write_file(&recovery_dir.join("stale.txt"), "stale recovery");
        let registry = ForkRegistry {
            server_url: Some("https://registry.example.test".to_string()),
            ..ForkRegistry::default()
        };
        write_fork_registry(&home, &registry).unwrap();
        let registry_path = super::super::skill_fork_registry::fork_registry_path(&home);
        let registry_before = fs::read(&registry_path).unwrap();
        let storage = InjectedForkTransactionStorage {
            fail_remove: Some(quarantine_dir.clone()),
            ..Default::default()
        };
        let fetch = FakeFetch {
            files: vec![("SKILL.md", "upstream body")],
        };
        let ledger = FakeLedger::default();

        let error = fork_skill_with_storage(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &fetch,
            &NeverCalledLookup,
            &storage,
        )
        .unwrap_err();

        assert!(
            error.contains("injected quarantine cleanup failure"),
            "{error}"
        );
        assert!(
            error.contains(&recovery_dir.display().to_string()),
            "{error}"
        );
        assert!(
            error.contains(&quarantine_dir.display().to_string()),
            "{error}"
        );
        assert_registry_content_unchanged(&fs::read(&registry_path).unwrap(), &registry_before);
        assert_eq!(fs::read_to_string(skill_md).unwrap(), "live body");
        assert_eq!(ledger.remove_calls.lock().unwrap().len(), 0);
        assert_eq!(
            fs::read_to_string(recovery_dir.join("SKILL.md")).unwrap(),
            "live body"
        );
        assert_eq!(
            fs::read_to_string(quarantine_dir.join("stale.txt")).unwrap(),
            "stale recovery"
        );
        assert!(!fork_snapshot_dir(&app_data, "find-bugs").exists());
    }

    #[test]
    fn rollback_failure_reports_the_quarantine_that_preserves_old_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        write_file(&home.join(".agents/skills/find-bugs/SKILL.md"), "live body");
        let recovery_dir = fork_live_recovery_dir(&app_data, "find-bugs");
        let quarantine_dir = fork_live_recovery_quarantine_dir(&app_data, "find-bugs");
        write_file(&recovery_dir.join("stale.txt"), "stale recovery");
        let storage = InjectedForkTransactionStorage {
            fail_rename_from: Some(quarantine_dir.clone()),
            ..Default::default()
        };
        let ledger = FakeLedger::default();

        let error = fork_skill_with_storage(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &FailingFetch,
            &NeverCalledLookup,
            &storage,
        )
        .unwrap_err();

        assert!(
            error.contains("Recovery rollback needs attention"),
            "{error}"
        );
        assert!(
            error.contains(&quarantine_dir.display().to_string()),
            "{error}"
        );
        assert!(!recovery_dir.exists());
        assert_eq!(
            fs::read_to_string(quarantine_dir.join("stale.txt")).unwrap(),
            "stale recovery"
        );
        assert_eq!(ledger.remove_calls.lock().unwrap().len(), 0);
    }

    #[test]
    fn preexisting_recovery_quarantine_is_not_clobbered() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_dotagents_ledger(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &"a".repeat(40),
        );
        write_file(&home.join(".agents/skills/find-bugs/SKILL.md"), "live body");
        let recovery_dir = fork_live_recovery_dir(&app_data, "find-bugs");
        let quarantine_dir = fork_live_recovery_quarantine_dir(&app_data, "find-bugs");
        write_file(&recovery_dir.join("current.txt"), "current recovery");
        write_file(&quarantine_dir.join("previous.txt"), "previous recovery");
        let ledger = FakeLedger::default();

        let error = fork_skill_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();

        assert!(
            error.contains("Fork recovery requires attention"),
            "{error}"
        );
        assert!(
            error.contains(&quarantine_dir.display().to_string()),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(recovery_dir.join("current.txt")).unwrap(),
            "current recovery"
        );
        assert_eq!(
            fs::read_to_string(quarantine_dir.join("previous.txt")).unwrap(),
            "previous recovery"
        );
        assert_eq!(ledger.remove_calls.lock().unwrap().len(), 0);
    }

    fn seed_registry(
        home: &Path,
        app_data: &Path,
        name: &str,
        base_commit: &str,
        mine_content: &str,
    ) {
        let mut registry = read_fork_registry(home).unwrap();
        registry.forks.insert(
            name.to_string(),
            ForkRecord {
                deployment_id: String::new(),
                skill_dir: PathBuf::new(),
                forked_at: "2026-01-01T00:00:00Z".to_string(),
                origin_tool: OriginTool::Dotagents,
                origin_source: "getsentry/find-bugs".to_string(),
                repo: "getsentry/find-bugs".to_string(),
                path: "skills/find-bugs".to_string(),
                declared_ref: None,
                base_commit: base_commit.to_string(),
            },
        );
        write_fork_registry(home, &registry).unwrap();
        write_file(
            &fork_snapshot_dir(app_data, name).join("SKILL.md"),
            mine_content,
        );
        write_file(
            &home.join(".agents/skills").join(name).join("SKILL.md"),
            mine_content,
        );
    }

    fn seed_update_check_latest(app_data: &Path, name: &str, latest_commit: &str) {
        use super::super::skill_update_check::{GhStatus, SkillUpdateState, UpdateCheckStore};
        use std::collections::BTreeMap;
        fs::create_dir_all(app_data.join("skill-studio")).unwrap();
        let store = UpdateCheckStore {
            version: 2,
            checked_at: Some("2026-01-01T00:00:00Z".to_string()),
            gh_status: GhStatus::Ok,
            owners: BTreeMap::from([(
                format!("owner:v1/global/{name}"),
                SkillUpdateState {
                    repo: "getsentry/find-bugs".to_string(),
                    path: "skills/find-bugs".to_string(),
                    installed_commit: None,
                    latest_commit: Some(latest_commit.to_string()),
                    latest_commit_at: None,
                    checked_at: "2026-01-01T00:00:00Z".to_string(),
                    error: None,
                },
            )]),
            legacy_skills: BTreeMap::new(),
        };
        fs::write(
            app_data.join("skill-studio/update-check.json"),
            serde_json::to_string(&store).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn pull_already_up_to_date_when_commits_match() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        let commit = "a".repeat(40);
        seed_registry(&home, &app_data, "find-bugs", &commit, "same body");
        seed_update_check_latest(&app_data, "find-bugs", &commit);

        let fetch = FakeFetch { files: vec![] };
        let result = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch,
            &NeverCalledLookup,
            &NoopEditorOpener,
        )
        .unwrap();
        assert_eq!(result.message.as_deref(), Some("Already up to date"));
        assert!(result.merged.is_empty() && result.conflicts.is_empty());
    }

    #[test]
    fn pull_clean_merge_takes_theirs_when_base_equals_mine() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_registry(
            &home,
            &app_data,
            "find-bugs",
            &"a".repeat(40),
            "shared body",
        );
        seed_update_check_latest(&app_data, "find-bugs", &"b".repeat(40));

        let fetch = FakeFetch {
            files: vec![("SKILL.md", "updated upstream body")],
        };
        let result = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch,
            &NeverCalledLookup,
            &NoopEditorOpener,
        )
        .unwrap();

        assert_eq!(result.merged, vec!["SKILL.md".to_string()]);
        assert!(result.conflicts.is_empty());
        let mine = fs::read_to_string(home.join(".agents/skills/find-bugs/SKILL.md")).unwrap();
        assert_eq!(mine, "updated upstream body");
        // The snapshot advances to the new base commit.
        assert_eq!(
            read_fork_registry(&home).unwrap().forks["find-bugs"].base_commit,
            "b".repeat(40)
        );
    }

    /// A conflicting pull writes git-style markers directly into the file -
    /// no subprocess, never an automatic merge - and hands the caller's
    /// editor opener the exact live path the markers landed at; a failure
    /// here names the file that would have been merged silently under the
    /// deleted `git merge-file` path instead.
    #[test]
    fn fork_pull_conflict_writes_markers_and_opens_the_editor_or_names_the_merged_file() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_registry(
            &home,
            &app_data,
            "find-bugs",
            &"a".repeat(40),
            "line one\nbase line\n",
        );
        write_file(
            &home.join(".agents/skills/find-bugs/SKILL.md"),
            "line one\nmine line\n",
        );
        seed_update_check_latest(&app_data, "find-bugs", &"b".repeat(40));

        let fetch = FakeFetch {
            files: vec![("SKILL.md", "line one\ntheirs line\n")],
        };
        let editor = RecordingEditorOpener::default();
        let result = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch,
            &NeverCalledLookup,
            &editor,
        )
        .unwrap();

        assert_eq!(result.conflicts, vec!["SKILL.md".to_string()]);
        let merged_path = home.join(".agents/skills/find-bugs/SKILL.md");
        let mine = fs::read_to_string(&merged_path).unwrap();
        assert!(
            mine.contains("<<<<<<< mine")
                && mine.contains("=======")
                && mine.contains(">>>>>>> theirs"),
            "expected conflict markers in {}: {mine}",
            merged_path.display()
        );
        let opened = editor.opened.lock().unwrap();
        assert_eq!(
            opened.as_slice(),
            [vec![merged_path.clone()]],
            "expected the editor to be opened once on {}: opened {opened:?}",
            merged_path.display()
        );
    }

    #[test]
    fn pull_conflict_produces_markers_and_is_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_registry(
            &home,
            &app_data,
            "find-bugs",
            &"a".repeat(40),
            "line one\nbase line\n",
        );
        // Mine diverges from base.
        write_file(
            &home.join(".agents/skills/find-bugs/SKILL.md"),
            "line one\nmine line\n",
        );
        seed_update_check_latest(&app_data, "find-bugs", &"b".repeat(40));

        let fetch = FakeFetch {
            files: vec![("SKILL.md", "line one\ntheirs line\n")],
        };
        let editor = RecordingEditorOpener::default();
        let result = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch,
            &NeverCalledLookup,
            &editor,
        )
        .unwrap();

        assert_eq!(result.conflicts, vec!["SKILL.md".to_string()]);
        let mine = fs::read_to_string(home.join(".agents/skills/find-bugs/SKILL.md")).unwrap();
        assert!(mine.contains("<<<<<<<"));
        assert!(
            !editor.opened.lock().unwrap().is_empty(),
            "expected the conflict to open the editor"
        );
    }

    /// Restores a directory's permissions on drop, so a fault-injection test
    /// that locks a directory down doesn't leave the tempdir un-removable
    /// even if an assertion panics first.
    struct RestorePerms(PathBuf, std::fs::Permissions);
    impl Drop for RestorePerms {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, self.1.clone());
        }
    }

    #[test]
    fn pull_swap_failure_leaves_live_tree_base_and_registry_unchanged() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_registry(&home, &app_data, "find-bugs", &"a".repeat(40), "body");
        seed_update_check_latest(&app_data, "find-bugs", &"b".repeat(40));

        // Lock down the parent of `mine_dir` so `swap_in_pull_result`'s first
        // rename (mine -> live-backup) fails with a permission error.
        let skills_root = home.join(".agents").join("skills");
        let original_perms = std::fs::metadata(&skills_root).unwrap().permissions();
        let restore = RestorePerms(skills_root.clone(), original_perms.clone());
        let mut locked = original_perms;
        locked.set_mode(0o555);
        std::fs::set_permissions(&skills_root, locked).unwrap();

        let fetch = FakeFetch {
            files: vec![("SKILL.md", "upstream changed it")],
        };
        let err = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch,
            &NeverCalledLookup,
            &NoopEditorOpener,
        )
        .unwrap_err();
        assert!(err.contains("Failed to back up the live tree"));

        drop(restore); // restore write access before reading back through it

        assert_eq!(
            fs::read_to_string(skills_root.join("find-bugs/SKILL.md")).unwrap(),
            "body"
        );
        assert_eq!(
            fs::read_to_string(fork_snapshot_dir(&app_data, "find-bugs").join("SKILL.md")).unwrap(),
            "body"
        );
        assert_eq!(
            read_fork_registry(&home).unwrap().forks["find-bugs"].base_commit,
            "a".repeat(40)
        );
    }

    #[test]
    fn pull_added_upstream_only_file_is_added() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_registry(&home, &app_data, "find-bugs", &"a".repeat(40), "body");
        seed_update_check_latest(&app_data, "find-bugs", &"b".repeat(40));

        let fetch = FakeFetch {
            files: vec![("SKILL.md", "body"), ("NEW.md", "new upstream file")],
        };
        let result = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch,
            &NeverCalledLookup,
            &NoopEditorOpener,
        )
        .unwrap();

        assert_eq!(result.added, vec!["NEW.md".to_string()]);
        assert!(home.join(".agents/skills/find-bugs/NEW.md").exists());
    }

    #[test]
    fn pull_removed_upstream_file_unchanged_locally_is_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_registry(&home, &app_data, "find-bugs", &"a".repeat(40), "body");
        write_file(
            &fork_snapshot_dir(&app_data, "find-bugs").join("OLD.md"),
            "old file",
        );
        write_file(&home.join(".agents/skills/find-bugs/OLD.md"), "old file");
        seed_update_check_latest(&app_data, "find-bugs", &"b".repeat(40));

        let fetch = FakeFetch {
            files: vec![("SKILL.md", "body")], // OLD.md gone upstream
        };
        let result = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch,
            &NeverCalledLookup,
            &NoopEditorOpener,
        )
        .unwrap();

        assert_eq!(result.removed, vec!["OLD.md".to_string()]);
        assert!(!home.join(".agents/skills/find-bugs/OLD.md").exists());
    }

    #[test]
    fn pull_theirs_modified_mine_deleted_restores_theirs_and_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_registry(&home, &app_data, "find-bugs", &"a".repeat(40), "body");
        write_file(
            &fork_snapshot_dir(&app_data, "find-bugs").join("SHARED.md"),
            "base",
        );
        // Deleted locally - no file at all under `mine_dir`.
        seed_update_check_latest(&app_data, "find-bugs", &"b".repeat(40));

        let fetch = FakeFetch {
            files: vec![("SKILL.md", "body"), ("SHARED.md", "upstream changed it")],
        };
        let editor = RecordingEditorOpener::default();
        let result = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch,
            &NeverCalledLookup,
            &editor,
        )
        .unwrap();

        assert_eq!(result.conflicts, vec!["SHARED.md".to_string()]);
        assert_eq!(
            fs::read_to_string(home.join(".agents/skills/find-bugs/SHARED.md")).unwrap(),
            "upstream changed it"
        );
        assert!(
            !editor.opened.lock().unwrap().is_empty(),
            "expected the conflict to open the editor"
        );
    }

    #[test]
    fn pull_mine_modified_theirs_deleted_keeps_mine_and_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_registry(&home, &app_data, "find-bugs", &"a".repeat(40), "body");
        write_file(
            &fork_snapshot_dir(&app_data, "find-bugs").join("SHARED.md"),
            "base",
        );
        write_file(
            &home.join(".agents/skills/find-bugs/SHARED.md"),
            "my local edit",
        );
        seed_update_check_latest(&app_data, "find-bugs", &"b".repeat(40));

        let fetch = FakeFetch {
            files: vec![("SKILL.md", "body")], // SHARED.md removed upstream
        };
        let editor = RecordingEditorOpener::default();
        let result = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch,
            &NeverCalledLookup,
            &editor,
        )
        .unwrap();

        assert_eq!(result.conflicts, vec!["SHARED.md".to_string()]);
        assert_eq!(
            fs::read_to_string(home.join(".agents/skills/find-bugs/SHARED.md")).unwrap(),
            "my local edit"
        );
        assert!(
            !editor.opened.lock().unwrap().is_empty(),
            "expected the conflict to open the editor"
        );
    }

    #[test]
    fn pull_leaves_a_local_only_file_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        seed_registry(&home, &app_data, "find-bugs", &"a".repeat(40), "body");
        write_file(
            &home.join(".agents/skills/find-bugs/NOTES.md"),
            "my private notes",
        );
        seed_update_check_latest(&app_data, "find-bugs", &"b".repeat(40));

        let fetch = FakeFetch {
            files: vec![("SKILL.md", "body")],
        };
        let result = pull_fork_upstream_with(
            &test_guard(&home),
            &home,
            &app_data,
            "find-bugs",
            &fetch,
            &NeverCalledLookup,
            &NoopEditorOpener,
        )
        .unwrap();

        assert!(!result.added.contains(&"NOTES.md".to_string()));
        assert!(!result.removed.contains(&"NOTES.md".to_string()));
        assert!(!result.conflicts.contains(&"NOTES.md".to_string()));
        assert_eq!(
            fs::read_to_string(home.join(".agents/skills/find-bugs/NOTES.md")).unwrap(),
            "my private notes"
        );
    }

    #[test]
    fn skills_sh_unfork_argv_keeps_source_ref_skill_and_global_scope() {
        let record = ForkRecord {
            deployment_id: String::new(),
            skill_dir: PathBuf::new(),
            forked_at: "2026-01-01T00:00:00Z".to_string(),
            origin_tool: OriginTool::SkillsSh,
            origin_source: "obra/find-bugs@v1.2.3".to_string(),
            repo: "obra/find-bugs".to_string(),
            path: "skills/find-bugs".to_string(),
            declared_ref: None,
            base_commit: "a".repeat(40),
        };

        let (args, cwd) = skills_sh_unfork_add_args(&record, "find-bugs").unwrap();
        assert_eq!(
            args,
            vec![
                "skills",
                "add",
                "obra/find-bugs@v1.2.3",
                "--yes",
                "--global",
                "--skill",
                "find-bugs",
                "--agent",
                "universal",
            ]
        );
        assert_eq!(
            cwd, None,
            "a fork is always global-scope, so unfork never sets a process cwd"
        );
    }

    /// `run_npx(args, Some(cwd))` must run the child process itself in
    /// `cwd`, not just log it - a fake `npx` script records its own working
    /// directory (via `pwd`) so this asserts the real
    /// `Command::current_dir` call, not the argv this function builds.
    #[test]
    fn run_npx_with_a_cwd_runs_the_process_there_or_names_the_ignored_cwd() {
        let bin_dir = tempfile::tempdir().expect("fake bin dir");
        let recording = bin_dir.path().join("pwd.log");
        let fake_npx = bin_dir.path().join("npx");
        std::fs::write(
            &fake_npx,
            format!("#!/bin/sh\npwd > '{}'\nexit 0\n", recording.display()),
        )
        .expect("write fake npx");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_npx, std::fs::Permissions::from_mode(0o700))
                .expect("chmod fake npx");
        }

        let target_dir = tempfile::tempdir().expect("target cwd");
        let _path_guard = super::super::test_support::PathGuard::new(bin_dir.path());
        let result = run_npx(&["--version".to_string()], Some(target_dir.path()));

        assert!(result.is_ok(), "{result:?}");
        let recorded_cwd = std::fs::read_to_string(&recording)
            .expect("read recording")
            .trim()
            .to_string();
        assert_eq!(
            std::fs::canonicalize(&recorded_cwd).expect("canonicalize recorded cwd"),
            std::fs::canonicalize(target_dir.path()).expect("canonicalize target cwd"),
            "run_npx must launch the process in the given cwd, not wherever the test process runs"
        );
    }

    #[test]
    fn unfork_removes_record_and_snapshot_and_reinstalls_with_declared_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        let mut registry = read_fork_registry(&home).unwrap();
        registry.forks.insert(
            "find-bugs".to_string(),
            ForkRecord {
                deployment_id: String::new(),
                skill_dir: PathBuf::new(),
                forked_at: "2026-01-01T00:00:00Z".to_string(),
                origin_tool: OriginTool::Dotagents,
                origin_source: "getsentry/find-bugs".to_string(),
                repo: "getsentry/find-bugs".to_string(),
                path: "skills/find-bugs".to_string(),
                declared_ref: Some("v1.2.3".to_string()),
                base_commit: "a".repeat(40),
            },
        );
        write_fork_registry(&home, &registry).unwrap();
        write_file(
            &fork_snapshot_dir(&app_data, "find-bugs").join("SKILL.md"),
            "body",
        );

        let ledger = FakeLedger::default();
        unfork_skill_with(&test_guard(&home), &home, &app_data, "find-bugs", &ledger).unwrap();

        assert!(!read_fork_registry(&home)
            .unwrap()
            .forks
            .contains_key("find-bugs"));
        assert!(!fork_snapshot_dir(&app_data, "find-bugs").exists());
        let calls = ledger.reinstall_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.declared_ref.as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn unfork_reinstall_has_no_ref_when_unpinned() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        let mut registry = read_fork_registry(&home).unwrap();
        registry.forks.insert(
            "find-bugs".to_string(),
            ForkRecord {
                deployment_id: String::new(),
                skill_dir: PathBuf::new(),
                forked_at: "2026-01-01T00:00:00Z".to_string(),
                origin_tool: OriginTool::SkillsSh,
                origin_source: "obra/find-bugs".to_string(),
                repo: "obra/find-bugs".to_string(),
                path: "skills/find-bugs".to_string(),
                declared_ref: None,
                base_commit: "a".repeat(40),
            },
        );
        write_fork_registry(&home, &registry).unwrap();

        let ledger = FakeLedger::default();
        unfork_skill_with(&test_guard(&home), &home, &app_data, "find-bugs", &ledger).unwrap();
        let calls = ledger.reinstall_calls.lock().unwrap();
        assert_eq!(calls[0].0.declared_ref, None);
    }

    // ------------------------------------------------------------------
    // Tarball extraction
    // ------------------------------------------------------------------

    #[test]
    fn locate_extracted_skill_dir_finds_top_and_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let extract_dir = tmp.path().join("extract");
        let top = extract_dir.join("owner-repo-abc1234");
        write_file(&top.join("skills/find-bugs/SKILL.md"), "body");
        fs::create_dir_all(&extract_dir).unwrap();
        // Build the extraction the way `tar -xzf` would leave it, by
        // actually round-tripping through a real tarball built with the
        // `tar` binary, so this test exercises the same tool the real
        // implementation shells out to.
        let build_dir = tmp.path().join("build");
        write_file(
            &build_dir.join("owner-repo-abc1234/skills/find-bugs/SKILL.md"),
            "body",
        );
        let tarball = tmp.path().join("test.tar.gz");
        let status = Command::new("tar")
            .args([
                "-czf",
                &tarball.to_string_lossy(),
                "-C",
                &build_dir.to_string_lossy(),
                "owner-repo-abc1234",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        fs::create_dir_all(&extract_dir).unwrap();
        let status = Command::new("tar")
            .args([
                "-xzf",
                &tarball.to_string_lossy(),
                "-C",
                &extract_dir.to_string_lossy(),
            ])
            .status()
            .unwrap();
        assert!(status.success());

        let found = locate_extracted_skill_dir(&extract_dir, "skills/find-bugs").unwrap();
        assert!(found.join("SKILL.md").exists());

        let err = locate_extracted_skill_dir(&extract_dir, "../../etc").unwrap_err();
        assert!(err.contains("outside") || err.contains("not found"));
    }
}
