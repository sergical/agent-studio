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

use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use flate2::read::GzDecoder;
use tauri::Manager;

use super::commands::{dotagents_add_args, dotagents_remove_args};
use super::dotagents_ledger;
use super::lock_file;
use super::skill_deployment::SkillDestination;
use super::skill_dto::InstallScope;
use super::skill_fork_registry::{
    deployment_trial_key, fork_snapshot_dir, read_fork_registry, trial_key, write_fork_registry,
    ForkRecord, ForkRegistry, OriginTool, TrialScope,
};
use super::skill_fs::copy_dir_all;
use super::skill_install_plan::{skills_sh_universal_add_args, SkillInstallSpec};
use super::skill_lifecycle::skills_sh_remove_args_for_scope;
use super::skill_process::{
    run_controlled_command_to_file_accepting, run_controlled_command_to_file_limited,
    run_controlled_npx_with_control_and_guard, AddOperationControl, ControlledProcessError,
    MAX_PROCESS_OUTPUT_BYTES,
};
use super::skill_refresh::{self, SkillRefreshState};
use super::skill_update_check::{self, CommitLookup, GhCommitLookup};

// ============================================================================
// Traits - real implementations shell out / hit the network; tests use fakes.
// ============================================================================

/// Removes a skill from its owning ledger, or reinstalls it from its
/// recorded origin. The real implementation shells out to the same argv
/// `remove_skill` / `add_skill` / `dotagents_update_args` already build
/// (see the command and install-plan arg builders).
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

fn skills_sh_unfork_add_args(rec: &ForkRecord, name: &str) -> Result<Vec<String>, String> {
    let spec = SkillInstallSpec {
        scope: InstallScope::Global,
        destination: SkillDestination::Universal,
        project_path: None,
        harnesses: vec![],
    };
    skills_sh_universal_add_args(&rec.origin_source, Some(name), &spec)
}

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
            OriginTool::Dotagents => dotagents_remove_args(name, InstallScope::Global),
            // Fork only ever applies to a global-scope skill (see
            // `skill_refresh::build_snapshot`), so remove/reinstall always
            // target the global scope.
            OriginTool::SkillsSh => skills_sh_remove_args_for_scope(name, InstallScope::Global),
        };
        run_npx(&args)
    }

    fn reinstall(&self, rec: &ForkRecord, name: &str) -> Result<(), String> {
        let args = match rec.origin_tool {
            OriginTool::Dotagents => {
                dotagents_add_args(&rec.origin_source, name, rec.declared_ref.as_deref())
            }
            OriginTool::SkillsSh => skills_sh_unfork_add_args(rec, name)?,
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
        _: &super::skill_update_check::CommitQuery<'_>,
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

const MAX_FORK_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_FORK_EXPANDED_BYTES: usize = 256 * 1024 * 1024;
const MAX_FORK_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_FORK_ARCHIVE_DEPTH: usize = 32;

#[derive(Clone, Copy)]
struct ForkArchiveLimits {
    expanded_bytes: usize,
    entries: usize,
    depth: usize,
}

const FORK_ARCHIVE_LIMITS: ForkArchiveLimits = ForkArchiveLimits {
    expanded_bytes: MAX_FORK_EXPANDED_BYTES,
    entries: MAX_FORK_ARCHIVE_ENTRIES,
    depth: MAX_FORK_ARCHIVE_DEPTH,
};

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

        let unique = format!("{}-{}-{}", std::process::id(), commit, ulid::Ulid::new());
        let tarball_path = self.cache_dir.join(format!("fork-pull-{unique}.tar.gz"));
        let extract_dir = self.cache_dir.join(format!("fork-pull-extract-{unique}"));
        let cleanup = TempCleanup {
            paths: vec![tarball_path.clone(), extract_dir.clone()],
        };

        run_controlled_command_to_file_limited(
            &self.gh_bin,
            &["api".to_string(), format!("repos/{repo}/tarball/{commit}")],
            None,
            control,
            &tarball_path,
            MAX_FORK_ARCHIVE_BYTES,
            MAX_PROCESS_OUTPUT_BYTES,
        )
        .map_err(ControlledProcessError::into_message)?;

        control.check_message()?;
        fs::create_dir_all(&extract_dir)
            .map_err(|e| format!("Failed to create {}: {e}", extract_dir.display()))?;
        extract_fork_archive(&tarball_path, &extract_dir, control)?;
        control.check_message()?;

        Ok(ExtractedRepo {
            extract_dir,
            _cleanup: cleanup,
        })
    }
}

fn archive_path_components(
    path: &Path,
    limits: ForkArchiveLimits,
) -> Result<Vec<&std::ffi::OsStr>, String> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => components.push(name),
            _ => return Err("Refusing unsafe path in fetched tarball".to_string()),
        }
    }
    if components.is_empty() || components.len() > limits.depth {
        return Err("Fetched tarball path exceeds safety limits".to_string());
    }
    Ok(components)
}

fn ensure_extract_directory(
    root: &Path,
    components: &[&std::ffi::OsStr],
) -> Result<PathBuf, String> {
    let mut current = root.to_path_buf();
    for component in components {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err("Refusing tarball entry through a non-directory path".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .map_err(|error| format!("Failed to create {}: {error}", current.display()))?;
            }
            Err(error) => return Err(format!("Failed to inspect {}: {error}", current.display())),
        }
    }
    Ok(current)
}

fn extract_fork_archive(
    tarball_path: &Path,
    extract_dir: &Path,
    control: &AddOperationControl,
) -> Result<(), String> {
    extract_fork_archive_with_limits(tarball_path, extract_dir, control, FORK_ARCHIVE_LIMITS)
}

struct ControlledArchiveReader<R> {
    inner: R,
    control: AddOperationControl,
    remaining: usize,
}

impl<R: Read> Read for ControlledArchiveReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.control
            .check_message()
            .map_err(std::io::Error::other)?;
        let read_limit = buffer.len().min(self.remaining.saturating_add(1));
        let count = self.inner.read(&mut buffer[..read_limit])?;
        if count > self.remaining {
            return Err(std::io::Error::other(
                "Fetched tarball exceeded expanded byte limit",
            ));
        }
        self.remaining -= count;
        Ok(count)
    }
}

fn extract_fork_archive_with_limits(
    tarball_path: &Path,
    extract_dir: &Path,
    control: &AddOperationControl,
    limits: ForkArchiveLimits,
) -> Result<(), String> {
    let file = fs::File::open(tarball_path)
        .map_err(|error| format!("Failed to open {}: {error}", tarball_path.display()))?;
    let reader = ControlledArchiveReader {
        inner: GzDecoder::new(file),
        control: control.clone(),
        remaining: limits.expanded_bytes,
    };
    let mut archive = tar::Archive::new(reader);
    let mut expanded_bytes = 0usize;
    let mut entries = 0usize;
    let mut top_level = None;
    #[cfg(unix)]
    let mut directory_modes = Vec::new();
    for entry in archive
        .entries()
        .map_err(|error| format!("Failed to read fetched tarball: {error}"))?
    {
        control.check_message()?;
        entries += 1;
        if entries > limits.entries {
            return Err(format!(
                "Fetched tarball exceeded {} entry limit",
                limits.entries
            ));
        }
        let mut entry =
            entry.map_err(|error| format!("Failed to read fetched tarball entry: {error}"))?;
        let entry_path = entry
            .path()
            .map_err(|error| format!("Failed to read fetched tarball path: {error}"))?;
        let components = archive_path_components(&entry_path, limits)?;
        if let Some(ref top) = top_level {
            if top != components[0] {
                return Err("Fetched tarball has more than one top-level directory".to_string());
            }
        } else {
            top_level = Some(components[0].to_os_string());
        }
        let destination = extract_dir.join(&entry_path);
        ensure_extract_directory(extract_dir, &components[..components.len() - 1])?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            ensure_extract_directory(extract_dir, &components)?;
            #[cfg(unix)]
            directory_modes.push((
                destination.clone(),
                entry.header().mode().map_err(|e| e.to_string())?,
            ));
        } else if kind.is_file() {
            if fs::symlink_metadata(&destination).is_ok() {
                return Err("Refusing duplicate fetched tarball entry".to_string());
            }
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .map_err(|error| format!("Failed to create {}: {error}", destination.display()))?;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                control.check_message()?;
                let count = entry
                    .read(&mut buffer)
                    .map_err(|error| format!("Failed to read fetched tarball entry: {error}"))?;
                if count == 0 {
                    break;
                }
                if count > limits.expanded_bytes.saturating_sub(expanded_bytes) {
                    return Err(format!(
                        "Fetched tarball exceeded {} expanded byte limit",
                        limits.expanded_bytes
                    ));
                }
                output.write_all(&buffer[..count]).map_err(|error| {
                    format!("Failed to write {}: {error}", destination.display())
                })?;
                expanded_bytes += count;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = entry
                    .header()
                    .mode()
                    .map_err(|error| format!("Failed to read fetched tarball mode: {error}"))?;
                fs::set_permissions(&destination, fs::Permissions::from_mode(mode)).map_err(
                    |error| {
                        format!(
                            "Failed to set permissions on {}: {error}",
                            destination.display()
                        )
                    },
                )?;
            }
        } else if kind.is_symlink() {
            let target = entry
                .link_name()
                .map_err(|error| format!("Failed to read fetched tarball link: {error}"))?
                .ok_or_else(|| "Fetched tarball symlink has no target".to_string())?;
            if fs::symlink_metadata(&destination).is_ok() {
                return Err("Refusing duplicate fetched tarball entry".to_string());
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &destination)
                .map_err(|error| format!("Failed to create {}: {error}", destination.display()))?;
            #[cfg(not(unix))]
            return Err(
                "Symlink-preserving archive extraction is only supported on Unix".to_string(),
            );
        } else {
            return Err("Refusing unsupported fetched tarball entry type".to_string());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        directory_modes.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
        for (path, mode) in directory_modes {
            control.check_message()?;
            fs::set_permissions(path, fs::Permissions::from_mode(mode))
                .map_err(|e| e.to_string())?;
        }
    }
    if top_level.is_none() {
        return Err("Fetched tarball was empty".to_string());
    }
    Ok(())
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
        super::skill_fs::copy_dir_preserving_symlinks_controlled_bounded(
            &source_dir,
            into,
            control,
            MAX_FORK_EXPANDED_BYTES,
            MAX_FORK_ARCHIVE_ENTRIES,
            MAX_FORK_ARCHIVE_DEPTH,
        )
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

struct PullPreparationCleanup(Option<skill_studio_core::skill_fork_pull::ForkPullPreparation>);

impl PullPreparationCleanup {
    fn new(preparation: skill_studio_core::skill_fork_pull::ForkPullPreparation) -> Self {
        Self(Some(preparation))
    }

    fn preparation(&self) -> &skill_studio_core::skill_fork_pull::ForkPullPreparation {
        self.0.as_ref().expect("Pull preparation is available")
    }

    fn take(&mut self) -> skill_studio_core::skill_fork_pull::ForkPullPreparation {
        self.0.take().expect("Pull preparation is available")
    }

    fn cancel_with_error(mut self, message: String) -> Result<PullResult, String> {
        let preparation = self.0.take().expect("Pull preparation is available");
        match skill_studio_core::skill_fork_pull::cancel_fork_pull(&preparation) {
            Ok(()) => Err(message),
            Err(cleanup) => Err(format!(
                "{message}; Pull preparation was preserved: {cleanup}"
            )),
        }
    }
}

impl Drop for PullPreparationCleanup {
    fn drop(&mut self) {
        if let Some(preparation) = self.0.take() {
            let _ = skill_studio_core::skill_fork_pull::cancel_fork_pull(&preparation);
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
    let mut top_entries = fs::read_dir(extract_dir)
        .map_err(|e| format!("Failed to read {}: {e}", extract_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_dir() && !kind.is_symlink())
                .unwrap_or(false)
        });
    let top = top_entries
        .next()
        .ok_or("Tarball had no top-level directory")?
        .path();
    if top_entries.next().is_some() {
        return Err("Tarball had more than one top-level directory".to_string());
    }
    let components = archive_path_components(Path::new(path), FORK_ARCHIVE_LIMITS)?;
    let mut candidate = top;
    for component in components {
        candidate.push(component);
        let metadata = fs::symlink_metadata(&candidate)
            .map_err(|_| format!("{path} was not found in the fetched tarball"))?;
        if metadata.file_type().is_symlink() {
            return Err("Refusing to extract a path through a tarball symlink".to_string());
        }
        if !metadata.is_dir() {
            return Err(format!("{path} was not found in the fetched tarball"));
        }
    }
    Ok(candidate)
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
        let owner_id = format!("owner:v1/global/{name}");
        let base_commit = match store
            .owners
            .get(&owner_id)
            .and_then(|s| s.installed_commit.clone())
        {
            Some(commit) => commit,
            None => {
                let until = if entry.updated_at.is_empty() {
                    None
                } else {
                    Some(entry.updated_at.as_str())
                };
                match lookup.latest_commit(&super::skill_update_check::CommitQuery {
                    repo: &repo,
                    path: &path,
                    source_ref: None,
                    until,
                })? {
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
    fn write_registry(&self, home: &Path, registry: &ForkRegistry) -> Result<(), String>;
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

    fn write_registry(&self, home: &Path, registry: &ForkRegistry) -> Result<(), String> {
        write_fork_registry(home, registry)
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

enum ForkRecoveryRollback {
    RestorePrevious,
    KeepComplete,
}

fn rollback_fork_before_detach(
    storage: &dyn ForkTransactionStorage,
    primary_error: String,
    paths: &ForkPreDetachPaths<'_>,
    registry_before: Option<&ForkRegistry>,
    recovery: ForkRecoveryRollback,
) -> String {
    let mut rollback_errors = Vec::new();
    if let Some(registry_before) = registry_before {
        if let Err(error) = storage.write_registry(paths.home, registry_before) {
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
pub fn fork_skill_with(
    home: &Path,
    app_data: &Path,
    name: &str,
    path: &Path,
    ledger: &dyn LedgerTool,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
) -> Result<ForkRecord, String> {
    fork_skill_with_storage(
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
                error,
                &rollback_paths,
                None,
                ForkRecoveryRollback::RestorePrevious,
            ));
        }
    };
    let mut registry = registry_before.clone();
    registry.forks.insert(name.to_string(), record.clone());
    // A forked skill is no longer the same "add" that started a trial - drop
    // any trial record for it so forking doesn't leave a stale one behind.
    // Forking only ever applies to the shared (global) `.agents/skills`
    // root, so only the global-scoped key needs clearing.
    registry.trials.remove(&trial_key(TrialScope::Global, name));
    registry
        .trials
        .remove(&deployment_trial_key(&record.deployment_id));
    if let Err(error) = storage.write_registry(home, &registry) {
        return Err(rollback_fork_before_detach(
            storage,
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
pub async fn fork_skill(
    target: super::skill_dto::LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<ForkRecord, String> {
    let operation_app = app.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || fork_skill_blocking(target, operation_app))
            .await
            .map_err(|error| format!("Fork worker failed: {error}"));
    skill_refresh::request_snapshot_rebuild(&app);
    result?
}

fn require_exact_fork_deployment_target(
    target: &super::skill_dto::LifecycleTarget,
) -> Result<(), String> {
    target
        .deployment_id
        .as_deref()
        .ok_or("Fork needs one Global Universal deployment_id")?;
    if target.owner_id.is_some() {
        return Err("Fork targets one Global Universal deployment, not an owner group".to_string());
    }
    Ok(())
}

fn fork_skill_blocking(
    target: super::skill_dto::LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<ForkRecord, String> {
    let refresh_state = app.state::<SkillRefreshState>();
    let fork_lock = app.state::<ForkMutationLock>();
    let _guard = fork_lock.try_acquire()?;
    require_exact_fork_deployment_target(&target)?;
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

    let resolved = super::skill_lifecycle::resolve_fresh_lifecycle_target(
        &app,
        &refresh_state,
        &target,
        "Fork",
    )?;
    super::skill_lifecycle::require_global_universal_park_target(&resolved.deployment)
        .map_err(|_| "Fork is only available for the Global Universal folder.".to_string())?;
    if resolved.deployment.owner_kind == super::skill_ownership::LifecycleOwnerKind::SkillsSh {
        for variable in ["XDG_CONFIG_HOME", "XDG_STATE_HOME"] {
            if std::env::var_os(variable).is_some_and(|value| !value.is_empty()) {
                return Err(format!(
                    "Fork cannot run skills.sh while {variable} redirects its provider paths"
                ));
            }
        }
        let owner_revision = resolved
            .deployment
            .owner_revision
            .clone()
            .ok_or("Fork is not available: skills.sh owner revision is missing")?;
        let origin = resolve_fork_origin(
            &home.join(".agents"),
            &app_data,
            &resolved.skill.name,
            lookup.as_ref(),
        )?;
        if origin.tool != OriginTool::SkillsSh {
            return Err("Fresh Fork owner no longer matches the skills.sh lock".into());
        }
        let staging = app_data
            .join("skill-studio/cache")
            .join(format!("skills-sh-fork-{}", ulid::Ulid::new()));
        let _cleanup = TempCleanup {
            paths: vec![staging.clone()],
        };
        let control = AddOperationControl::bounded_default();
        fetch.fetch_skill_dir_controlled(
            &origin.repo,
            &origin.path,
            &origin.base_commit,
            &staging,
            &control,
        )?;
        control.check_message()?;
        let projects = resolved
            .snapshot
            .projects
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
        let mut service = skill_studio_core::skill_service::ScopedSkillService::bind(scope)
            .map_err(|error| error.to_string())?;
        let request = skill_studio_core::skill_skills_sh_fork_creation::SkillsShForkRequest {
            deployment_id: resolved.deployment.id,
            expected_owner_revision: owner_revision,
            expected_source: skill_studio_core::skill_skills_sh_fork_creation::SkillsShForkSource {
                origin_source: origin.origin_source,
                repo: origin.repo,
                path: origin.path,
                base_commit: origin.base_commit,
            },
        };
        let mut remove_args =
            skills_sh_remove_args_for_scope(&resolved.skill.name, InstallScope::Global);
        remove_args.extend(["--agent".into(), "universal".into()]);
        let event_state = app.state::<super::event_commands::EventStoreState>();
        let events = event_state
            .0
            .lock()
            .map_err(|_| "Event store lock is unavailable")?;
        let store = events.as_ref().ok_or("Event store is unavailable")?;
        return skill_studio_core::skill_skills_sh_fork_creation::create_skills_sh_fork(
            &mut service,
            store,
            &request,
            &staging,
            super::skill_copy_recovery::removal_limits(),
            Some(std::time::Duration::from_secs(30)),
            skill_studio_core::skill_service::CancellationToken::default(),
            |provider_guard| {
                run_controlled_npx_with_control_and_guard(
                    &remove_args,
                    None,
                    &control,
                    provider_guard,
                )
                .map_err(|error| match error {
                    ControlledProcessError::Cancelled => "Provider operation cancelled".into(),
                    ControlledProcessError::TimedOut => "Provider operation timed out".into(),
                    ControlledProcessError::Failed(message) => message,
                })
            },
        )
        .map(|outcome| outcome.record)
        .map_err(|error| match (error.event_id, error.recovery_required) {
            (Some(id), true) => format!("{} (event {id} requires recovery)", error.message),
            (Some(id), false) => format!("{} (event {id} is recorded as failed)", error.message),
            (None, _) => error.message,
        });
    }
    if resolved.deployment.owner_kind != super::skill_ownership::LifecycleOwnerKind::Dotagents {
        return fork_skill_with(
            &home,
            &app_data,
            &resolved.skill.name,
            Path::new(&resolved.deployment.path),
            &RealLedgerTool,
            &fetch,
            lookup.as_ref(),
        );
    }

    let owner_revision = resolved
        .deployment
        .owner_revision
        .clone()
        .ok_or("Fork is not available: dotagents owner revision is missing")?;
    let agents = home.join(".agents");
    let lock = std::fs::read_to_string(agents.join("agents.lock"))
        .map_err(|error| format!("Could not read fresh agents.lock: {error}"))?;
    let manifest = std::fs::read_to_string(agents.join("agents.toml"))
        .map_err(|error| format!("Could not read fresh agents.toml: {error}"))?;
    let expected_source =
        skill_studio_core::skill_dotagents_ledger::DotagentsDetachIntent::from_documents(
            &resolved.skill.name,
            &lock,
            &manifest,
        )?
        .fork_source()?;
    let staging = app_data
        .join("skill-studio/cache")
        .join(format!("dotagents-fork-{}", ulid::Ulid::new()));
    let _cleanup = TempCleanup {
        paths: vec![staging.clone()],
    };
    let control = AddOperationControl::bounded_default();
    fetch.fetch_skill_dir_controlled(
        expected_source.repo(),
        expected_source.path(),
        expected_source.commit(),
        &staging,
        &control,
    )?;
    control.check_message()?;

    let projects = resolved
        .snapshot
        .projects
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
    let mut service = skill_studio_core::skill_service::ScopedSkillService::bind(scope)
        .map_err(|error| error.to_string())?;
    let request = skill_studio_core::skill_fork_creation::ForkCreationRequest {
        deployment_id: resolved.deployment.id,
        expected_owner_revision: owner_revision,
        expected_source,
    };
    let event_state = app.state::<super::event_commands::EventStoreState>();
    let events = event_state
        .0
        .lock()
        .map_err(|_| "Event store lock is unavailable")?;
    let store = events.as_ref().ok_or("Event store is unavailable")?;
    skill_studio_core::skill_fork_creation::create_dotagents_fork(
        &mut service,
        store,
        &request,
        &staging,
        super::skill_copy_recovery::removal_limits(),
        Some(std::time::Duration::from_secs(30)),
        skill_studio_core::skill_service::CancellationToken::default(),
    )
    .map(|outcome| outcome.record)
    .map_err(|error| match (error.event_id, error.recovery_required) {
        (Some(id), true) => format!("{} (event {id} requires recovery)", error.message),
        (Some(id), false) => format!("{} (event {id} was rolled back)", error.message),
        (None, _) => error.message,
    })
}
// ============================================================================
// Pull upstream
// ============================================================================

pub type PullResult = skill_studio_core::skill_fork_pull::ForkPullResult;

struct DesktopPullTextMerge<'a> {
    control: &'a AddOperationControl,
    scratch_root: PathBuf,
}

impl skill_studio_core::skill_fork_pull::ForkPullTextMerge for DesktopPullTextMerge<'_> {
    fn merge(
        &self,
        mine: &[u8],
        base: &[u8],
        theirs: &[u8],
        relative_path: &Path,
        max_output: u64,
    ) -> Result<skill_studio_core::skill_fork_pull::ForkPullTextMergeResult, String> {
        self.control.check_message()?;
        fs::create_dir_all(&self.scratch_root).map_err(|error| error.to_string())?;
        let scratch = self
            .scratch_root
            .join(format!("merge-{}", ulid::Ulid::new()));
        fs::create_dir(&scratch).map_err(|error| error.to_string())?;
        let _cleanup = TempCleanup {
            paths: vec![scratch.clone()],
        };
        for (name, bytes) in [("mine", mine), ("base", base), ("theirs", theirs)] {
            fs::write(scratch.join(name), bytes).map_err(|error| error.to_string())?;
        }
        let output = scratch.join("merged");
        let args = vec![
            "merge-file".to_string(),
            "-p".to_string(),
            "mine".to_string(),
            "base".to_string(),
            "theirs".to_string(),
        ];
        let accepted = (0..=127).collect::<Vec<_>>();
        let status = run_controlled_command_to_file_accepting(
            Path::new("git"),
            &args,
            Some(&scratch),
            self.control,
            &output,
            usize::try_from(max_output).unwrap_or(usize::MAX),
            MAX_PROCESS_OUTPUT_BYTES,
            &accepted,
        )
        .map_err(ControlledProcessError::into_message)?;
        let bytes = fs::read(&output).map_err(|error| {
            format!(
                "Failed to read merged output for {}: {error}",
                relative_path.display()
            )
        })?;
        if status == 0 {
            Ok(skill_studio_core::skill_fork_pull::ForkPullTextMergeResult::Clean(bytes))
        } else {
            Ok(skill_studio_core::skill_fork_pull::ForkPullTextMergeResult::Conflicts(bytes))
        }
    }
}

#[tauri::command]
pub async fn pull_fork_upstream(
    target: super::skill_dto::LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<PullResult, String> {
    let operation_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        pull_fork_upstream_blocking(target, operation_app)
    })
    .await
    .map_err(|error| format!("Pull worker failed: {error}"))?;
    skill_refresh::request_snapshot_rebuild(&app);
    result
}

fn pull_fork_upstream_blocking(
    target: super::skill_dto::LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<PullResult, String> {
    let refresh_state = app.state::<SkillRefreshState>();
    let fork_lock = app.state::<ForkMutationLock>();
    let _guard = fork_lock.try_acquire()?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not resolve app data dir: {error}"))?;
    let resolved = super::skill_lifecycle::resolve_fresh_lifecycle_target(
        &app,
        &refresh_state,
        &target,
        "Pull upstream",
    )?;
    let (name, record) = resolve_recorded_fork_target(&resolved.snapshot, &target, &home)?;
    if resolved.deployment.owner_kind != super::skill_ownership::LifecycleOwnerKind::Fork {
        return Err("Pull requires one fresh Global Universal Fork".into());
    }
    let owner_revision = resolved
        .deployment
        .owner_revision
        .clone()
        .ok_or("Pull is not available: Fork owner revision is missing")?;
    let store_state = skill_update_check::read_update_check_store(&app_data);
    let owner_id = format!("owner:v1/global/{name}");
    let lookup = resolve_lookup();
    let to_commit = match store_state
        .owners
        .get(&owner_id)
        .and_then(|state| state.latest_commit.clone())
    {
        Some(commit) => commit,
        None => lookup
            .latest_commit(&super::skill_update_check::CommitQuery {
                repo: &record.repo,
                path: &record.path,
                source_ref: record.declared_ref.as_deref(),
                until: None,
            })?
            .map(|(commit, _)| commit)
            .ok_or_else(|| format!("Could not determine {name}'s latest upstream commit"))?,
    };
    let projects = resolved
        .snapshot
        .projects
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
    let mut service = skill_studio_core::skill_service::ScopedSkillService::bind(scope)
        .map_err(|error| error.to_string())?;
    let request = skill_studio_core::skill_fork_pull::ForkPullRequest {
        deployment_id: resolved.deployment.id,
        expected_owner_revision: owner_revision,
        to_commit,
    };
    let limits = super::skill_copy_recovery::removal_limits();
    let event_state = app.state::<super::event_commands::EventStoreState>();
    let preparation = {
        let events = event_state
            .0
            .lock()
            .map_err(|_| "Event store lock is unavailable")?;
        let store = events.as_ref().ok_or("Event store is unavailable")?;
        match skill_studio_core::skill_fork_pull::prepare_fork_pull_inputs(
            &mut service,
            store,
            &request,
            limits,
            Some(std::time::Duration::from_secs(30)),
            skill_studio_core::skill_service::CancellationToken::default(),
        )? {
            skill_studio_core::skill_fork_pull::ForkPullPreparationOutcome::UpToDate(result) => {
                return Ok(result)
            }
            skill_studio_core::skill_fork_pull::ForkPullPreparationOutcome::Prepared(prepared) => {
                prepared
            }
        }
    };
    let mut preparation_cleanup = PullPreparationCleanup::new(preparation);
    let control = AddOperationControl::bounded_default();
    let fetch_root = app_data
        .join("skill-studio/cache")
        .join(format!("pull-{}", preparation_cleanup.preparation().id()));
    let upstream = fetch_root.join("upstream");
    if fs::symlink_metadata(&fetch_root).is_ok() {
        return preparation_cleanup.cancel_with_error(format!(
            "Pull fetch staging already exists at {}; preserving it",
            fetch_root.display()
        ));
    }
    if let Err(error) = fs::create_dir_all(&fetch_root) {
        return preparation_cleanup.cancel_with_error(error.to_string());
    }
    let _fetch_cleanup = TempCleanup {
        paths: vec![fetch_root.clone()],
    };
    let Some(gh_bin) = skill_update_check::resolve_gh_binary() else {
        return preparation_cleanup.cancel_with_error("Run Check now first".into());
    };
    let fetch = RealUpstreamFetch {
        gh_bin,
        cache_dir: app_data.join("skill-studio/cache"),
    };
    if let Err(error) = fetch.fetch_skill_dir_controlled(
        preparation_cleanup.preparation().repo(),
        preparation_cleanup.preparation().source_path(),
        preparation_cleanup.preparation().to_commit(),
        &upstream,
        &control,
    ) {
        return preparation_cleanup.cancel_with_error(error);
    }
    let merger = DesktopPullTextMerge {
        control: &control,
        scratch_root: fetch_root.join("merges"),
    };
    let result = {
        let events = event_state
            .0
            .lock()
            .map_err(|_| "Event store lock is unavailable")?;
        let store = events.as_ref().ok_or("Event store is unavailable")?;
        let preparation = preparation_cleanup.take();
        skill_studio_core::skill_fork_pull::commit_fork_pull(
            &mut service,
            store,
            preparation,
            &upstream,
            &merger,
            limits,
            Some(std::time::Duration::from_secs(30)),
            skill_studio_core::skill_service::CancellationToken::default(),
        )
    };
    result.map_err(|error| match (error.event_id, error.recovery_required) {
        (Some(id), true) => format!("{} (event {id} requires recovery)", error.message),
        (Some(id), false) => format!("{} (event {id} was rolled back)", error.message),
        (None, _) => error.message,
    })
}

// ============================================================================
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
    registry.trials.remove(&trial_key(TrialScope::Global, name));
    if !record.deployment_id.is_empty() {
        registry
            .trials
            .remove(&deployment_trial_key(&record.deployment_id));
    }
    write_fork_registry(home, &registry)?;
    let _ = fs::remove_dir_all(fork_snapshot_dir(app_data, name));
    Ok(())
}
#[cfg(any(test, target_os = "macos"))]
fn uses_native_unfork(record: &ForkRecord) -> bool {
    matches!(
        record.origin_tool,
        OriginTool::Dotagents | OriginTool::SkillsSh
    )
}

#[tauri::command]
pub async fn unfork_skill(
    target: super::skill_dto::LifecycleTarget,
    app: tauri::AppHandle,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let fork_lock = app.state::<ForkMutationLock>();
        let _guard = fork_lock.try_acquire()?;
        let refresh_state = app.state::<SkillRefreshState>();
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
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
        #[cfg(target_os = "macos")]
        let (name, record) = resolve_recorded_fork_target(&resolved.snapshot, &target, &home)?;
        #[cfg(not(target_os = "macos"))]
        let (name, _) = resolve_recorded_fork_target(&resolved.snapshot, &target, &home)?;
        #[cfg(target_os = "macos")]
        let result = if uses_native_unfork(&record) {
            let id = target
                .deployment_id
                .as_deref()
                .ok_or("Unfork lifecycle needs one Global Universal deployment_id")?;
            let (_, deployment) = super::skill_lifecycle::find_deployment(&resolved.snapshot, id)?;
            let resource_root = app
                .path()
                .resource_dir()
                .map_err(|error| format!("Could not resolve packaged Unfork runtime: {error}"))?;
            let provider = super::skill_native_unfork::load_packaged_runtime(&resource_root)?;
            if record.origin_tool == OriginTool::SkillsSh {
                super::skill_skills_sh_unfork::apply(
                    super::skill_native_unfork::NativeUnforkTarget {
                        home: &home,
                        app_data: &app_data,
                        deployment_id: &deployment.id,
                        owner_revision: deployment
                            .owner_revision
                            .as_deref()
                            .ok_or("Unfork owner revision is missing")?,
                        live: Path::new(&deployment.path),
                    },
                    &record,
                    provider,
                )
            } else {
                super::skill_native_unfork::apply(
                    &home,
                    &app_data,
                    deployment.id.clone(),
                    deployment
                        .owner_revision
                        .clone()
                        .ok_or("Unfork owner revision is missing")?,
                    Path::new(&deployment.path),
                    provider,
                )
            }
        } else {
            unfork_skill_with(&home, &app_data, &name, &RealLedgerTool)
        };
        #[cfg(not(target_os = "macos"))]
        let result = unfork_skill_with(&home, &app_data, &name, &RealLedgerTool);
        skill_refresh::request_snapshot_rebuild(&app);
        result
    })
    .await
    .map_err(|error| format!("Unfork worker failed: {error}"))?
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
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::sync::Mutex;

    fn append_archive_file(
        builder: &mut tar::Builder<GzEncoder<fs::File>>,
        path: &str,
        bytes: &[u8],
    ) {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, bytes).unwrap();
    }

    #[cfg(unix)]
    fn append_archive_symlink(
        builder: &mut tar::Builder<GzEncoder<fs::File>>,
        path: &str,
        target: &str,
    ) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_link_name(target).unwrap();
        header.set_cksum();
        builder
            .append_data(&mut header, path, std::io::empty())
            .unwrap();
    }

    fn write_archive(path: &Path, add: impl FnOnce(&mut tar::Builder<GzEncoder<fs::File>>)) {
        let file = fs::File::create(path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        add(&mut builder);
        builder.finish().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn controlled_fetch_preserves_archive_symlinks_and_cleans_scratch() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("fixture.tar.gz");
        write_archive(&archive, |builder| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o750);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    "owner-repo-sha/skills/example",
                    std::io::empty(),
                )
                .unwrap();
            append_archive_file(builder, "owner-repo-sha/skills/example/SKILL.md", b"body");
            append_archive_symlink(builder, "owner-repo-sha/skills/example/valid", "SKILL.md");
            append_archive_symlink(builder, "owner-repo-sha/skills/example/dangling", "missing");
        });
        let gh = tmp.path().join("gh-fixture");
        fs::write(&gh, format!("#!/bin/sh\ncat '{}'\n", archive.display())).unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        let cache_dir = tmp.path().join("cache");
        let destination = tmp.path().join("staging");
        RealUpstreamFetch {
            gh_bin: gh,
            cache_dir: cache_dir.clone(),
        }
        .fetch_skill_dir_controlled(
            "owner/repo",
            "skills/example",
            "sha",
            &destination,
            &AddOperationControl::bounded_default(),
        )
        .unwrap();

        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert_eq!(
            fs::read_link(destination.join("valid")).unwrap(),
            Path::new("SKILL.md")
        );
        assert_eq!(
            fs::read_link(destination.join("dangling")).unwrap(),
            Path::new("missing")
        );
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).unwrap(),
            "body"
        );
        assert_eq!(fs::read_dir(cache_dir).unwrap().count(), 0);
    }

    #[test]
    fn controlled_archive_refuses_unsafe_paths() {
        assert!(
            archive_path_components(Path::new("../../escape"), FORK_ARCHIVE_LIMITS)
                .unwrap_err()
                .contains("unsafe")
        );
    }

    #[test]
    fn controlled_archive_limits_reject_entries_depth_and_expanded_bytes_before_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("limits.tar.gz");
        write_archive(&archive, |builder| {
            append_archive_file(builder, "top/first", b"1234");
            append_archive_file(builder, "top/second", b"5678");
        });
        let control = AddOperationControl::bounded_default();
        let entries = tmp.path().join("entries");
        fs::create_dir(&entries).unwrap();
        assert!(extract_fork_archive_with_limits(
            &archive,
            &entries,
            &control,
            ForkArchiveLimits {
                expanded_bytes: 8192,
                entries: 1,
                depth: 4
            },
        )
        .unwrap_err()
        .contains("entry limit"));

        let expanded = tmp.path().join("expanded");
        fs::create_dir(&expanded).unwrap();
        assert!(extract_fork_archive_with_limits(
            &archive,
            &expanded,
            &control,
            ForkArchiveLimits {
                expanded_bytes: 3,
                entries: 4,
                depth: 4
            },
        )
        .unwrap_err()
        .contains("expanded byte limit"));
        assert!(!expanded.join("top/first").exists());

        let depth = tmp.path().join("depth");
        fs::create_dir(&depth).unwrap();
        assert!(extract_fork_archive_with_limits(
            &archive,
            &depth,
            &control,
            ForkArchiveLimits {
                expanded_bytes: 8192,
                entries: 4,
                depth: 1
            },
        )
        .unwrap_err()
        .contains("path exceeds"));
    }

    #[cfg(unix)]
    #[test]
    fn controlled_archive_refuses_file_through_archive_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("link-traversal.tar.gz");
        write_archive(&archive, |builder| {
            append_archive_symlink(builder, "top/skills", "/tmp");
            append_archive_file(builder, "top/skills/escape", b"no");
        });
        let extract = tmp.path().join("extract");
        fs::create_dir(&extract).unwrap();
        let error =
            extract_fork_archive(&archive, &extract, &AddOperationControl::bounded_default())
                .unwrap_err();
        assert!(error.contains("non-directory"));
        assert!(!Path::new("/tmp/escape").exists());
    }

    #[test]
    fn fork_command_requires_one_exact_deployment_target() {
        let exact = super::super::skill_dto::LifecycleTarget {
            deployment_id: Some("deployment".into()),
            owner_id: None,
        };
        assert!(require_exact_fork_deployment_target(&exact).is_ok());

        let grouped = super::super::skill_dto::LifecycleTarget {
            deployment_id: Some("deployment".into()),
            owner_id: Some("owner".into()),
        };
        assert!(require_exact_fork_deployment_target(&grouped)
            .unwrap_err()
            .contains("not an owner group"));

        let missing = super::super::skill_dto::LifecycleTarget {
            deployment_id: None,
            owner_id: None,
        };
        assert!(require_exact_fork_deployment_target(&missing)
            .unwrap_err()
            .contains("deployment_id"));
    }

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
            _: &super::skill_update_check::CommitQuery<'_>,
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

        fn write_registry(&self, home: &Path, registry: &ForkRegistry) -> Result<(), String> {
            let mut calls = self.registry_write_calls.lock().unwrap();
            *calls += 1;
            if self.fail_registry_write_call == Some(*calls) {
                return Err("injected registry write failure".to_string());
            }
            write_fork_registry(home, registry)
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

    #[test]
    fn fork_drops_a_stale_trial_record() {
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
        let now = chrono::Utc::now();
        let mut registry = read_fork_registry(&home).unwrap();
        registry.trials.insert(
            trial_key(TrialScope::Global, "find-bugs"),
            super::super::skill_fork_registry::TrialRecord {
                deployment_id: String::new(),
                started_at: now.to_rfc3339(),
                expires_at: (now + chrono::Duration::hours(24)).to_rfc3339(),
                status: super::super::skill_fork_registry::TrialStatus::Active,
                method: super::super::skill_fork_registry::AddMethod::Dotagents,
                scope: super::super::skill_fork_registry::TrialScope::Global,
                project_path: None,
                skill_dir: home.join(".agents/skills/find-bugs"),
                deployment_fingerprint: String::new(),
                claude_link: None,
                claude_link_target: None,
            },
        );
        write_fork_registry(&home, &registry).unwrap();

        let ledger = FakeLedger::default();
        let fetch = FakeFetch {
            files: vec![("SKILL.md", "---\nname: find-bugs\n---\nupstream body")],
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

        assert!(!read_fork_registry(&home)
            .unwrap()
            .trials
            .contains_key(&trial_key(TrialScope::Global, "find-bugs")));
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
        assert_eq!(fs::read(&registry_path).unwrap(), registry_before);
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

        assert_eq!(
            skills_sh_unfork_add_args(&record, "find-bugs").unwrap(),
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
    fn unfork_drops_a_stale_trial_record() {
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
                declared_ref: None,
                base_commit: "a".repeat(40),
            },
        );
        let now = chrono::Utc::now();
        registry.trials.insert(
            trial_key(TrialScope::Global, "find-bugs"),
            super::super::skill_fork_registry::TrialRecord {
                deployment_id: String::new(),
                started_at: now.to_rfc3339(),
                expires_at: (now + chrono::Duration::hours(24)).to_rfc3339(),
                status: super::super::skill_fork_registry::TrialStatus::Active,
                method: super::super::skill_fork_registry::AddMethod::Copy,
                scope: super::super::skill_fork_registry::TrialScope::Global,
                project_path: None,
                skill_dir: home.join(".agents/skills/find-bugs"),
                deployment_fingerprint: String::new(),
                claude_link: None,
                claude_link_target: None,
            },
        );
        write_fork_registry(&home, &registry).unwrap();

        let ledger = FakeLedger::default();
        unfork_skill_with(&home, &app_data, "find-bugs", &ledger).unwrap();

        assert!(!read_fork_registry(&home)
            .unwrap()
            .trials
            .contains_key(&trial_key(TrialScope::Global, "find-bugs")));
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
        assert!(
            err.contains("unsafe") || err.contains("outside") || err.contains("not found"),
            "{err}"
        );
    }
}
