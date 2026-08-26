// ============================================================================
// Skills Module - skill_fork
// Fork / Pull upstream / Un-fork for a dotagents- or skills.sh-managed skill:
// "Fork" detaches it from its owning ledger (so `sync`/`update` can't
// overwrite local edits) while keeping a snapshot of the last-synced copy;
// "Pull upstream" three-way merges that snapshot against the skill's current
// on-disk copy and a freshly fetched upstream copy; "Un-fork" discards local
// edits and reinstalls from the recorded origin. The CLI-shelling and
// GitHub-fetching bits are behind small traits so the merge/refusal logic is
// testable with fakes.
// ============================================================================

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use tauri::Manager;

use super::commands::{
    dotagents_add_args, dotagents_remove_args, skills_sh_add_args, skills_sh_remove_args,
};
use super::dotagents_ledger;
use super::lock_file;
use super::skill_fork_registry::{
    fork_snapshot_dir, read_fork_registry, write_fork_registry, ForkRecord, ForkRegistry,
    OriginTool,
};
use super::skill_refresh::{self, SkillRefreshState};
use super::skill_update_check::{self, CommitLookup, GhCommitLookup, UpdateCheckState};

// ============================================================================
// Traits - real implementations shell out / hit the network; tests use fakes.
// ============================================================================

/// Removes a skill from its owning ledger, or reinstalls it from its
/// recorded origin. The real implementation shells out to the same argv
/// `remove_skill` / `install_skill` / `dotagents_update_args` already build
/// (see the `dotagents_*`/`skills_sh_*` arg builders in `commands.rs`).
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
}

/// Real `LedgerTool`, shelling out to `npx`.
pub struct RealLedgerTool;

fn run_npx(args: &[String]) -> Result<(), String> {
    let output = Command::new("npx")
        .args(args)
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
            OriginTool::Dotagents => dotagents_remove_args(name),
            // Fork only ever applies to a global-scope skill (see
            // `skill_refresh::build_snapshot`), so remove/reinstall always
            // target the global scope.
            OriginTool::SkillsSh => skills_sh_remove_args(name, true),
        };
        run_npx(&args)
    }

    fn reinstall(&self, rec: &ForkRecord, name: &str) -> Result<(), String> {
        let args = match rec.origin_tool {
            OriginTool::Dotagents => {
                dotagents_add_args(&rec.origin_source, name, rec.declared_ref.as_deref())
            }
            OriginTool::SkillsSh => skills_sh_add_args(&rec.origin_source, Some(name), true, None),
        };
        run_npx(&args)
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
        fs::create_dir_all(&self.cache_dir)
            .map_err(|e| format!("Failed to create {}: {e}", self.cache_dir.display()))?;

        let unique = format!("{}-{}", std::process::id(), commit);
        let tarball_path = self.cache_dir.join(format!("fork-pull-{unique}.tar.gz"));
        let extract_dir = self.cache_dir.join(format!("fork-pull-extract-{unique}"));
        let _cleanup = TempCleanup {
            paths: vec![tarball_path.clone(), extract_dir.clone()],
        };

        let output = Command::new(&self.gh_bin)
            .args(["api", &format!("repos/{repo}/tarball/{commit}")])
            .output()
            .map_err(|e| format!("Failed to run gh: {e}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        fs::write(&tarball_path, &output.stdout)
            .map_err(|e| format!("Failed to write {}: {e}", tarball_path.display()))?;

        fs::create_dir_all(&extract_dir)
            .map_err(|e| format!("Failed to create {}: {e}", extract_dir.display()))?;
        let tar_output = Command::new("tar")
            .args([
                "-xzf",
                &tarball_path.to_string_lossy(),
                "-C",
                &extract_dir.to_string_lossy(),
            ])
            .output()
            .map_err(|e| format!("Failed to run tar: {e}"))?;
        if !tar_output.status.success() {
            return Err(String::from_utf8_lossy(&tar_output.stderr)
                .trim()
                .to_string());
        }

        let source_dir = locate_extracted_skill_dir(&extract_dir, path)?;
        copy_dir_all(&source_dir, into)
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
        .filter_map(|e| e.ok())
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

/// Recursively copies `src` into `dst`, creating `dst` if needed. Symlinks
/// are skipped - a forked skill's snapshot/upstream copies are plain trees.
fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create {}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("Failed to read a directory entry: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to stat {}: {e}", entry.path().display()))?;
        let dest_path = dst.join(entry.file_name());
        if file_type.is_symlink() {
            continue;
        } else if file_type.is_dir() {
            copy_dir_all(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), &dest_path)
                .map_err(|e| format!("Failed to copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Every relative file path (`/`-separated) under `dir`, skipping `.git` and
/// symlinks. Empty when `dir` doesn't exist.
fn collect_relative_files(dir: &Path, out: &mut BTreeSet<String>) {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(|e| e.ok()) {
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
    let dotagents_skills = dotagents_ledger::read_dotagents_ledger(agents_dir)?;
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

    let lock = lock_file::read_lock_file_at(&agents_dir.join(".skill-lock.json"))?;
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
        let base_commit = match store
            .skills
            .get(name)
            .and_then(|s| s.installed_commit.clone())
        {
            Some(commit) => commit,
            None => {
                let until = if entry.updated_at.is_empty() {
                    None
                } else {
                    Some(entry.updated_at.as_str())
                };
                match lookup.latest_commit(&repo, &path, until)? {
                    Some((sha, _)) => sha,
                    None => return Err(format!("Could not determine {name}'s installed commit")),
                }
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
            "Only the shared-folder copy (~/.agents/skills/<name>) can be forked".to_string(),
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
/// "already synced". The record is written before the ledger is touched, so
/// a registry-write failure never leaves a skill detached with no
/// provenance; the live tree is snapshotted to a recovery copy right before
/// the ledger's `remove` runs, so a folder that removal wipes can still be
/// restored.
pub fn fork_skill_with(
    home: &Path,
    app_data: &Path,
    name: &str,
    path: &Path,
    ledger: &dyn LedgerTool,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
) -> Result<ForkRecord, String> {
    validate_fork_path(home, name, path)?;

    let agents_dir = home.join(".agents");
    let skill_dir = agents_dir.join("skills").join(name);
    let origin = resolve_fork_origin(&agents_dir, app_data, name, lookup)?;

    // 1. Fetch the upstream tree at `base_commit` as the merge base - not a
    //    copy of the (possibly locally edited) live tree.
    let base_dir = fork_snapshot_dir(app_data, name);
    if base_dir.exists() {
        fs::remove_dir_all(&base_dir)
            .map_err(|e| format!("Failed to clear the stale snapshot for {name}: {e}"))?;
    }
    fetch
        .fetch_skill_dir(&origin.repo, &origin.path, &origin.base_commit, &base_dir)
        .map_err(|e| {
            format!(
                "Could not fetch {name}'s upstream copy at {}: {e}. Nothing was changed.",
                origin.base_commit
            )
        })?;

    // 2. Write the record before touching the ledger - a failure here means
    //    the skill is still fully attached, never detached with no record.
    let record = ForkRecord {
        forked_at: chrono::Utc::now().to_rfc3339(),
        origin_tool: origin.tool,
        origin_source: origin.origin_source,
        repo: origin.repo,
        path: origin.path,
        declared_ref: origin.declared_ref,
        base_commit: origin.base_commit,
    };
    let mut registry = read_fork_registry(home)?;
    registry.forks.insert(name.to_string(), record.clone());
    if let Err(e) = write_fork_registry(home, &registry) {
        let _ = fs::remove_dir_all(&base_dir);
        return Err(e);
    }

    // 3. Snapshot the live tree as a recovery copy before removing it from
    //    the ledger, in case that removal wipes the directory.
    let recovery_dir = fork_live_recovery_dir(app_data, name);
    if recovery_dir.exists() {
        fs::remove_dir_all(&recovery_dir)
            .map_err(|e| format!("Failed to clear the stale recovery copy for {name}: {e}"))?;
    }
    if let Err(e) = copy_dir_all(&skill_dir, &recovery_dir) {
        registry.forks.remove(name);
        let _ = write_fork_registry(home, &registry);
        let _ = fs::remove_dir_all(&base_dir);
        return Err(format!("Failed to snapshot {name} before forking: {e}"));
    }

    // 4. Remove it from the owning ledger.
    if let Err(e) = ledger.remove(origin.tool, name) {
        registry.forks.remove(name);
        let _ = write_fork_registry(home, &registry);
        let _ = fs::remove_dir_all(&base_dir);
        let _ = fs::remove_dir_all(&recovery_dir);
        return Err(e);
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

/// Serializes fork/pull/unfork/remove-forked so two concurrent calls can't
/// race on the registry, the snapshot, or the CLI. A single global lock (as
/// opposed to per-skill) is fine: forking is a rare, user-initiated action.
#[derive(Default)]
pub struct ForkMutationLock(std::sync::Mutex<()>);

impl ForkMutationLock {
    /// `Err` when another fork operation already holds the lock.
    pub fn try_acquire(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.0
            .try_lock()
            .map_err(|_| "Another fork operation is in progress".to_string())
    }
}

#[tauri::command]
pub fn fork_skill(
    name: String,
    path: String,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    update_check_state: tauri::State<UpdateCheckState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<ForkRecord, String> {
    let _guard = fork_lock.try_acquire()?;
    let _ = &update_check_state; // shares the same guard-free lookup path as pull/unfork
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not resolve app data dir: {e}"))?;
    let lookup = resolve_lookup();
    let gh_bin =
        skill_update_check::resolve_gh_binary().ok_or_else(|| "Run Check now first".to_string())?;
    let fetch = RealUpstreamFetch {
        gh_bin,
        cache_dir: app_data.join("skill-studio").join("cache"),
    };

    let result = fork_skill_with(
        &home,
        &app_data,
        &name,
        Path::new(&path),
        &RealLedgerTool,
        &fetch,
        lookup.as_ref(),
    );
    skill_refresh::request_snapshot_rebuild(&app);
    let _ = &refresh_state;
    result
}

// ============================================================================
// Pull upstream
// ============================================================================

/// What one `pull_fork_upstream` call did.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

/// True when `bytes` contains a NUL byte - `git merge-file` operates on
/// text, so a file with a NUL is treated as binary regardless of whether the
/// rest of it happens to be valid UTF-8.
fn is_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

/// What a finished `git merge-file -p mine base theirs` run means, decided
/// from its exit status alone. Pulled out of `three_way_merge_text` so it's
/// unit-testable without spawning a process. Per `git merge-file`'s
/// documented contract: exit 0 is a clean merge; a positive exit up to 127
/// is that many conflicted hunks, with stdout holding the marked-up merge to
/// keep either way; anything else - a signal, a status `>= 128`, or empty
/// stdout despite non-empty inputs (the merge silently produced nothing) -
/// means the result can't be trusted, and the caller must abort rather than
/// write it anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeExitClass {
    Clean,
    Conflicts(usize),
    Error,
}

fn classify_merge_exit(
    code: Option<i32>,
    stdout_len: usize,
    inputs_nonempty: bool,
) -> MergeExitClass {
    if inputs_nonempty && stdout_len == 0 {
        return MergeExitClass::Error;
    }
    match code {
        Some(0) => MergeExitClass::Clean,
        Some(n) if (1..=127).contains(&n) => MergeExitClass::Conflicts(n as usize),
        _ => MergeExitClass::Error,
    }
}

/// A resolved `three_way_merge_text` run: the merged bytes plus whether it
/// was clean or left conflict markers behind.
enum MergeOutcome {
    Clean(Vec<u8>),
    Conflicts(Vec<u8>),
}

/// Runs `git merge-file -p mine base theirs` in a scratch dir. Returns
/// `Err` (never writing `stdout` anywhere) when `classify_merge_exit` can't
/// trust the result - see its doc comment - so a `pull_fork_upstream` that
/// hits this aborts the whole pull instead of writing a bogus merge.
fn three_way_merge_text(
    mine: &[u8],
    base: &[u8],
    theirs: &[u8],
    rel: &str,
) -> Result<MergeOutcome, String> {
    // `tempfile` is a dev-only dependency, so production code builds its own
    // scratch dir under the system temp dir instead.
    let scratch = std::env::temp_dir().join(format!(
        "skill-studio-merge-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&scratch).map_err(|e| format!("Failed to create scratch dir: {e}"))?;
    let _cleanup = TempCleanup {
        paths: vec![scratch.clone()],
    };
    fs::write(scratch.join("mine"), mine)
        .map_err(|e| format!("Failed to write scratch file: {e}"))?;
    fs::write(scratch.join("base"), base)
        .map_err(|e| format!("Failed to write scratch file: {e}"))?;
    fs::write(scratch.join("theirs"), theirs)
        .map_err(|e| format!("Failed to write scratch file: {e}"))?;

    let output = Command::new("git")
        .args(["merge-file", "-p", "mine", "base", "theirs"])
        .current_dir(&scratch)
        .output()
        .map_err(|e| format!("Failed to run git merge-file on {rel}: {e}"))?;

    let inputs_nonempty = !mine.is_empty() || !base.is_empty() || !theirs.is_empty();
    match classify_merge_exit(output.status.code(), output.stdout.len(), inputs_nonempty) {
        MergeExitClass::Clean => Ok(MergeOutcome::Clean(output.stdout)),
        MergeExitClass::Conflicts(_) => Ok(MergeOutcome::Conflicts(output.stdout)),
        MergeExitClass::Error => Err(format!(
            "git merge-file on {rel} exited unexpectedly (status {:?}); aborting the pull",
            output.status.code()
        )),
    }
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
    if let Err(e) = write_fork_registry(home, registry) {
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
    home: &Path,
    app_data: &Path,
    name: &str,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
) -> Result<PullResult, String> {
    let mut registry = read_fork_registry(home)?;
    let record = registry
        .forks
        .get(name)
        .cloned()
        .ok_or_else(|| format!("`{name}` is not forked"))?;

    let store = skill_update_check::read_update_check_store(app_data);
    let to_commit = match store.skills.get(name).and_then(|s| s.latest_commit.clone()) {
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

    let mine_dir = home.join(".agents").join("skills").join(name);
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
                        // never hand it to `git merge-file`.
                        write_staged(&staging_live, rel, &mine)?;
                        result.conflicts.push(rel.clone());
                    } else {
                        match three_way_merge_text(&mine, base_bytes, &theirs, rel)? {
                            MergeOutcome::Clean(merged) => {
                                write_staged(&staging_live, rel, &merged)?;
                                result.merged.push(rel.clone());
                            }
                            MergeOutcome::Conflicts(merged) => {
                                write_staged(&staging_live, rel, &merged)?;
                                result.conflicts.push(rel.clone());
                            }
                        }
                    }
                }
            }
            // Deleted on both sides, or nothing anywhere: nothing to carry
            // into the merged tree.
            (Some(_), None, None) | (None, None, None) => {}
        }
    }

    swap_in_pull_result(
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

    Ok(result)
}

#[tauri::command]
pub fn pull_fork_upstream(
    name: String,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    update_check_state: tauri::State<UpdateCheckState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<PullResult, String> {
    let _guard = fork_lock.try_acquire()?;
    let _ = &update_check_state;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
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

    let result = pull_fork_upstream_with(&home, &app_data, &name, &fetch, lookup.as_ref());
    skill_refresh::request_snapshot_rebuild(&app);
    let _ = &refresh_state;
    result
}

// ============================================================================
// Un-fork
// ============================================================================

/// `unfork_skill`'s logic, taking `home`/`app_data` and the trait directly.
pub fn unfork_skill_with(
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
    write_fork_registry(home, &registry)?;
    let _ = fs::remove_dir_all(fork_snapshot_dir(app_data, name));
    Ok(())
}

#[tauri::command]
pub fn unfork_skill(
    name: String,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    update_check_state: tauri::State<UpdateCheckState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let _ = &update_check_state;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not resolve app data dir: {e}"))?;

    let result = unfork_skill_with(&home, &app_data, &name, &RealLedgerTool);
    skill_refresh::request_snapshot_rebuild(&app);
    let _ = &refresh_state;
    result
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every `remove`/`reinstall` call so tests can assert "called
    /// once with the right OriginTool" without shelling out to `npx`.
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
    /// `base_commit`, not a copy of the (possibly locally edited) live tree
    /// - otherwise a local edit made before forking would be treated as
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
        let result = pull_fork_upstream_with(
            &home,
            &app_data,
            "find-bugs",
            &fetch_at_pull,
            &NeverCalledLookup,
        )
        .unwrap();
        assert_eq!(result.conflicts, vec!["SKILL.md".to_string()]);
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
            &home,
            &app_data,
            "find-bugs",
            &home.join("project/.claude/skills/find-bugs"),
            &ledger,
            &NeverCalledFetch,
            &NeverCalledLookup,
        )
        .unwrap_err();
        assert!(err.contains("shared-folder"));
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

    /// Finding 2: a CLI-remove failure must leave no record, no base
    /// snapshot, and no recovery copy - and the folder untouched.
    #[test]
    fn fork_remove_failure_leaves_no_snapshot_or_record() {
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
            &home,
            &app_data,
            "find-bugs",
            &home.join(".agents/skills/find-bugs"),
            &ledger,
            &fetch,
            &NeverCalledLookup,
        )
        .unwrap_err();
        assert_eq!(err, "npx failed");

        assert!(!fork_snapshot_dir(&app_data, "find-bugs").exists());
        assert!(!fork_live_recovery_dir(&app_data, "find-bugs").exists());
        assert!(!read_fork_registry(&home)
            .unwrap()
            .forks
            .contains_key("find-bugs"));
        assert_eq!(
            fs::read_to_string(home.join(".agents/skills/find-bugs/SKILL.md")).unwrap(),
            "body"
        );
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
            checked_at: Some("2026-01-01T00:00:00Z".to_string()),
            gh_status: GhStatus::Ok,
            skills: BTreeMap::from([(
                name.to_string(),
                SkillUpdateState {
                    repo: "getsentry/find-bugs".to_string(),
                    path: "skills/find-bugs".to_string(),
                    installed_commit: None,
                    latest_commit: Some(latest_commit.to_string()),
                    latest_commit_at: None,
                    checked_at: "2026-01-01T00:00:00Z".to_string(),
                    error: None,
                    lock_updated_at: None,
                },
            )]),
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
        let result =
            pull_fork_upstream_with(&home, &app_data, "find-bugs", &fetch, &NeverCalledLookup)
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
        let result =
            pull_fork_upstream_with(&home, &app_data, "find-bugs", &fetch, &NeverCalledLookup)
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

    #[test]
    fn fork_mutation_lock_refuses_a_concurrent_second_acquire() {
        let lock = ForkMutationLock::default();
        let first = lock.try_acquire().unwrap();
        let second = lock.try_acquire();
        assert_eq!(second.unwrap_err(), "Another fork operation is in progress");
        drop(first);
        // Released - a later call succeeds.
        assert!(lock.try_acquire().is_ok());
    }

    #[test]
    fn classify_merge_exit_covers_clean_conflicts_and_untrustworthy_results() {
        // Clean merge.
        assert_eq!(
            classify_merge_exit(Some(0), 10, true),
            MergeExitClass::Clean
        );
        // 1..=127 conflicted hunks, with a non-empty merge on stdout.
        assert_eq!(
            classify_merge_exit(Some(1), 10, true),
            MergeExitClass::Conflicts(1)
        );
        assert_eq!(
            classify_merge_exit(Some(127), 10, true),
            MergeExitClass::Conflicts(127)
        );
        // Signal-terminated / spawn-failure caller convention: no exit code.
        assert_eq!(classify_merge_exit(None, 10, true), MergeExitClass::Error);
        // Status >= 128 is untrustworthy, not "128 conflicts".
        assert_eq!(
            classify_merge_exit(Some(128), 10, true),
            MergeExitClass::Error
        );
        // Empty stdout despite non-empty inputs means the merge produced
        // nothing worth trusting, even for an exit code that would otherwise
        // read as clean or conflicted.
        assert_eq!(classify_merge_exit(Some(0), 0, true), MergeExitClass::Error);
        assert_eq!(classify_merge_exit(Some(1), 0, true), MergeExitClass::Error);
        // All-empty inputs legitimately produce empty stdout - not an error.
        assert_eq!(
            classify_merge_exit(Some(0), 0, false),
            MergeExitClass::Clean
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
        let result =
            pull_fork_upstream_with(&home, &app_data, "find-bugs", &fetch, &NeverCalledLookup)
                .unwrap();

        assert_eq!(result.conflicts, vec!["SKILL.md".to_string()]);
        let mine = fs::read_to_string(home.join(".agents/skills/find-bugs/SKILL.md")).unwrap();
        assert!(mine.contains("<<<<<<<"));
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
        let _restore = RestorePerms(skills_root.clone(), original_perms.clone());
        let mut locked = original_perms;
        locked.set_mode(0o555);
        std::fs::set_permissions(&skills_root, locked).unwrap();

        let fetch = FakeFetch {
            files: vec![("SKILL.md", "upstream changed it")],
        };
        let err =
            pull_fork_upstream_with(&home, &app_data, "find-bugs", &fetch, &NeverCalledLookup)
                .unwrap_err();
        assert!(err.contains("Failed to back up the live tree"));

        drop(_restore); // restore write access before reading back through it

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
        let result =
            pull_fork_upstream_with(&home, &app_data, "find-bugs", &fetch, &NeverCalledLookup)
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
        let result =
            pull_fork_upstream_with(&home, &app_data, "find-bugs", &fetch, &NeverCalledLookup)
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
        let result =
            pull_fork_upstream_with(&home, &app_data, "find-bugs", &fetch, &NeverCalledLookup)
                .unwrap();

        assert_eq!(result.conflicts, vec!["SHARED.md".to_string()]);
        assert_eq!(
            fs::read_to_string(home.join(".agents/skills/find-bugs/SHARED.md")).unwrap(),
            "upstream changed it"
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
        let result =
            pull_fork_upstream_with(&home, &app_data, "find-bugs", &fetch, &NeverCalledLookup)
                .unwrap();

        assert_eq!(result.conflicts, vec!["SHARED.md".to_string()]);
        assert_eq!(
            fs::read_to_string(home.join(".agents/skills/find-bugs/SHARED.md")).unwrap(),
            "my local edit"
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
        let result =
            pull_fork_upstream_with(&home, &app_data, "find-bugs", &fetch, &NeverCalledLookup)
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
    fn unfork_removes_record_and_snapshot_and_reinstalls_with_declared_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        let mut registry = read_fork_registry(&home).unwrap();
        registry.forks.insert(
            "find-bugs".to_string(),
            ForkRecord {
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
        unfork_skill_with(&home, &app_data, "find-bugs", &ledger).unwrap();

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
        unfork_skill_with(&home, &app_data, "find-bugs", &ledger).unwrap();
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
