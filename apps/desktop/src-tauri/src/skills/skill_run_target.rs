// ============================================================================
// Skills Module - skill_run_target
// Prepares the working directory a "Test" run executes in - a scratch
// folder, a detached git worktree, or the project's own tree in place - and
// the diff/apply/discard operations each kind supports afterward. Prepared
// targets are backend-owned state, keyed by an opaque id: the frontend never
// sees a cwd it could point another command at, and every id-based command
// re-checks that the stored cwd still lives under the root its kind expects
// before touching disk.
// ============================================================================

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use super::skill_agent_runner::copy_skill_dir_for_run_target;

/// Which kind of working directory a "Test" run executes in.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillRunTargetKind {
    Scratch,
    Worktree,
    InPlace,
}

/// Request to prepare one run target.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillRunTargetRequest {
    pub kind: SkillRunTargetKind,
    pub skill_name: String,
    pub skill_folder: String,
    /// Other own skills to also install (Scratch only): (name, folder path).
    #[serde(default)]
    pub extra_skills: Vec<(String, String)>,
    /// `=== path` fixture text (Scratch only) - see `write_fixture_files`.
    pub fixture: Option<String>,
    /// Required for Worktree and InPlace.
    pub project_path: Option<String>,
}

/// A prepared working directory a run can execute in, and what it takes to
/// clean it up or fold its changes back afterward. Stays private to the
/// backend - the frontend only ever holds the id it was returned.
#[derive(Debug, Clone)]
struct PreparedRunTarget {
    id: String,
    kind: SkillRunTargetKind,
    cwd: PathBuf,
    /// Set for Scratch (the scratch dir) and Worktree (the worktree path).
    cleanup_path: Option<PathBuf>,
    /// The project a Worktree/InPlace target was prepared against.
    project_path: Option<PathBuf>,
    /// The project HEAD sha the worktree was branched from (Worktree only).
    git_head: Option<String>,
}

/// The DTO handed back to the frontend once a target is prepared: just
/// enough to render the "Test" UI and drive it, none of the fields a
/// command would need to trust blindly (`cleanup_path`, `project_path`).
#[derive(Debug, Clone, Serialize)]
pub struct SkillRunTargetInfo {
    pub id: String,
    pub kind: SkillRunTargetKind,
    pub cwd: String,
    pub git_head: Option<String>,
}

impl From<&PreparedRunTarget> for SkillRunTargetInfo {
    fn from(target: &PreparedRunTarget) -> Self {
        Self {
            id: target.id.clone(),
            kind: target.kind,
            cwd: target.cwd.to_string_lossy().to_string(),
            git_head: target.git_head.clone(),
        }
    }
}

/// Managed app state: every run target prepared and not yet discarded or
/// (for Worktree) folded back, keyed by id.
#[derive(Default)]
pub struct SkillRunTargetState {
    targets: Mutex<HashMap<String, PreparedRunTarget>>,
}

static RUN_TARGET_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A run target id must be safe to use as a HashMap key and to log: short,
/// and drawn from a small alphabet - the same shape `validate_run_id` in
/// `skill_agent_runner` requires of a run id.
fn next_run_target_id() -> String {
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let counter = RUN_TARGET_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos}-{counter}")
}

/// The canonical roots a Scratch/Worktree target's stored `cwd` must live
/// under, resolved once per command from the app handle. Kept separate from
/// `AppHandle` so the containment check itself (`check_containment`) is
/// testable without a running Tauri app.
struct RunTargetRoots {
    scratch: PathBuf,
    worktrees: PathBuf,
}

impl RunTargetRoots {
    fn from_app(app: &AppHandle) -> Result<Self, String> {
        Ok(Self {
            scratch: scratch_root(app)?,
            worktrees: worktree_root(app)?,
        })
    }
}

/// Looks `id` up in `targets` and re-validates that its stored `cwd` still
/// lives where its kind expects, before any command mutates or reads it.
/// Refuses a forged/unknown id and a target whose backing directory moved
/// (or was replaced) since it was prepared, both with the same message so
/// neither case leaks which one occurred.
fn resolve_target(
    targets: &Mutex<HashMap<String, PreparedRunTarget>>,
    roots: &RunTargetRoots,
    id: &str,
) -> Result<PreparedRunTarget, String> {
    let target = {
        let map = targets
            .lock()
            .map_err(|_| "Unknown run target".to_string())?;
        map.get(id).cloned().ok_or("Unknown run target")?
    };
    check_containment(&target, roots)?;
    Ok(target)
}

fn check_containment(target: &PreparedRunTarget, roots: &RunTargetRoots) -> Result<(), String> {
    let canonical_cwd =
        fs::canonicalize(&target.cwd).map_err(|_| "Unknown run target".to_string())?;
    match target.kind {
        SkillRunTargetKind::Scratch => {
            let root =
                fs::canonicalize(&roots.scratch).map_err(|_| "Unknown run target".to_string())?;
            if !canonical_cwd.starts_with(&root) {
                return Err("Unknown run target".to_string());
            }
        }
        SkillRunTargetKind::Worktree => {
            let root =
                fs::canonicalize(&roots.worktrees).map_err(|_| "Unknown run target".to_string())?;
            if !canonical_cwd.starts_with(&root) {
                return Err("Unknown run target".to_string());
            }
        }
        SkillRunTargetKind::InPlace => {
            if target.project_path.as_deref() != Some(target.cwd.as_path()) {
                return Err("Unknown run target".to_string());
            }
        }
    }
    Ok(())
}

/// Prepares the working directory for a "Test" run, per `request.kind`, and
/// stores it under a fresh id.
#[tauri::command]
pub fn prepare_skill_run_target(
    app: AppHandle,
    state: State<SkillRunTargetState>,
    request: SkillRunTargetRequest,
) -> Result<SkillRunTargetInfo, String> {
    let id = next_run_target_id();
    let prepared = match request.kind {
        SkillRunTargetKind::Scratch => prepare_scratch(&app, &request, id)?,
        SkillRunTargetKind::Worktree => prepare_worktree(&app, &request, id)?,
        SkillRunTargetKind::InPlace => prepare_in_place(&request, id)?,
    };
    let info = SkillRunTargetInfo::from(&prepared);
    state
        .targets
        .lock()
        .map_err(|_| "Could not store run target".to_string())?
        .insert(prepared.id.clone(), prepared);
    Ok(info)
}

// ============================================================================
// Scratch
// ============================================================================

/// One file parsed out of a `=== path` fixture block.
struct FixtureFile {
    relative_path: String,
    body: String,
}

/// Parses the `=== path/relative.ext` fixture format: each line starting with
/// `=== ` opens a file whose body is every following line up to the next
/// `===` marker (or the end of the text). Rejects any path containing `..`
/// or starting with `/`, so a fixture can't write outside the run target.
fn parse_fixture(fixture: &str) -> Result<Vec<FixtureFile>, String> {
    let mut files = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;

    for line in fixture.lines() {
        if let Some(path) = line.strip_prefix("=== ") {
            if let Some((relative_path, body_lines)) = current.take() {
                files.push(FixtureFile {
                    relative_path,
                    body: body_lines.join("\n"),
                });
            }
            let path = path.trim().to_string();
            if path.contains("..") || path.starts_with('/') {
                return Err(format!("Invalid fixture path: {path:?}"));
            }
            if path.is_empty() {
                return Err("Fixture path cannot be empty".to_string());
            }
            current = Some((path, Vec::new()));
        } else if let Some((_, body_lines)) = current.as_mut() {
            body_lines.push(line);
        }
    }
    if let Some((relative_path, body_lines)) = current {
        files.push(FixtureFile {
            relative_path,
            body: body_lines.join("\n"),
        });
    }
    Ok(files)
}

/// Writes every file `parse_fixture` extracts from `fixture` under `root`,
/// creating parent directories as needed.
fn write_fixture_files(root: &Path, fixture: &str) -> Result<(), String> {
    for file in parse_fixture(fixture)? {
        let dest = root.join(&file.relative_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }
        fs::write(&dest, file.body)
            .map_err(|e| format!("Could not write {}: {e}", dest.display()))?;
    }
    Ok(())
}

fn scratch_root(app: &AppHandle) -> Result<PathBuf, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("Could not resolve app cache dir: {e}"))?;
    Ok(cache_dir.join("skill-studio").join("scratch"))
}

fn prepare_scratch(
    app: &AppHandle,
    request: &SkillRunTargetRequest,
    id: String,
) -> Result<PreparedRunTarget, String> {
    let mut skills = vec![(request.skill_name.clone(), request.skill_folder.clone())];
    skills.extend(request.extra_skills.iter().cloned());
    let scratch_dir = super::skill_agent_runner::create_skill_scratch_dir(app.clone(), skills)?;

    if let Some(fixture) = &request.fixture {
        write_fixture_files(Path::new(&scratch_dir), fixture)?;
    }

    let cwd = PathBuf::from(scratch_dir);
    Ok(PreparedRunTarget {
        id,
        kind: SkillRunTargetKind::Scratch,
        cwd: cwd.clone(),
        cleanup_path: Some(cwd),
        project_path: None,
        git_head: None,
    })
}

// ============================================================================
// Worktree
// ============================================================================

fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    run_git_raw(cwd, args).map(|out| out.trim().to_string())
}

/// Like `run_git`, but returns stdout untouched - needed for `git diff`,
/// where trimming would drop the trailing newline `git apply` requires on
/// the last line of a patch.
fn run_git_raw(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("Could not run git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// `run_git` with `paths` appended after `prefix` - used for the batched
/// `checkout`/`clean` calls, whose path lists are only known at runtime.
fn run_git_paths(cwd: &Path, prefix: &[&str], paths: &[String]) -> Result<String, String> {
    let mut args: Vec<&str> = prefix.to_vec();
    args.extend(paths.iter().map(String::as_str));
    run_git(cwd, &args)
}

fn worktree_root(app: &AppHandle) -> Result<PathBuf, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("Could not resolve app cache dir: {e}"))?;
    Ok(cache_dir.join("skill-studio").join("worktrees"))
}

fn prepare_worktree(
    app: &AppHandle,
    request: &SkillRunTargetRequest,
    id: String,
) -> Result<PreparedRunTarget, String> {
    let project_path = request
        .project_path
        .as_deref()
        .ok_or("Worktree requires a project")?;
    let project = Path::new(project_path);

    let toplevel = run_git(project, &["rev-parse", "--show-toplevel"])
        .map_err(|e| format!("Not a git repository: {e}"))?;
    let git_head = run_git(project, &["rev-parse", "HEAD"])?;

    let stamp = format!(
        "{}-{}",
        Utc::now().format("%Y%m%dT%H%M%S%.f"),
        std::process::id()
    );
    let worktree_path = worktree_root(app)?.join(stamp);
    fs::create_dir_all(worktree_path.parent().unwrap())
        .map_err(|e| format!("Could not create worktree root: {e}"))?;

    run_git(
        Path::new(&toplevel),
        &[
            "worktree",
            "add",
            "--detach",
            &worktree_path.to_string_lossy(),
            "HEAD",
        ],
    )?;

    let skill_dest = worktree_path
        .join(".agents")
        .join("skills")
        .join(&request.skill_name);
    if !skill_dest.exists() {
        copy_skill_dir_for_run_target(Path::new(&request.skill_folder), &skill_dest)
            .map_err(|e| format!("Could not install skill into worktree: {e}"))?;
        for agent_dir in [".claude/skills", ".pi/skills"] {
            let link_dir = worktree_path.join(agent_dir);
            fs::create_dir_all(&link_dir)
                .map_err(|e| format!("Could not create {agent_dir}: {e}"))?;
            let target = Path::new("../../.agents/skills").join(&request.skill_name);
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, link_dir.join(&request.skill_name))
                .map_err(|e| format!("Could not symlink {agent_dir}: {e}"))?;
        }
        // Commit the setup (the copied skill folder and its per-agent
        // symlinks) into the worktree's own detached history, so the later
        // `diff`/`apply` - taken relative to *this* commit, not the
        // project's original HEAD - never includes the setup files
        // themselves. `git_head` above stays the project's original HEAD;
        // only the worktree's own head moves.
        run_git(&worktree_path, &["add", "-A"])?;
        run_git(
            &worktree_path,
            &[
                "-c",
                "user.name=Skill Studio",
                "-c",
                "user.email=skill-studio@localhost",
                "commit",
                "-q",
                "-m",
                "skill-studio: test setup",
            ],
        )?;
    }

    Ok(PreparedRunTarget {
        id,
        kind: SkillRunTargetKind::Worktree,
        cwd: worktree_path.clone(),
        cleanup_path: Some(worktree_path),
        project_path: Some(PathBuf::from(project_path)),
        git_head: Some(git_head),
    })
}

// ============================================================================
// InPlace
// ============================================================================

fn prepare_in_place(
    request: &SkillRunTargetRequest,
    id: String,
) -> Result<PreparedRunTarget, String> {
    let project_path = request
        .project_path
        .as_deref()
        .ok_or("In place requires a project")?;
    let project = Path::new(project_path);

    let status = run_git(project, &["status", "--porcelain"])?;
    if !status.is_empty() {
        return Err(
            "Working tree has uncommitted changes. Commit or stash them first.".to_string(),
        );
    }

    Ok(PreparedRunTarget {
        id,
        kind: SkillRunTargetKind::InPlace,
        cwd: project.to_path_buf(),
        cleanup_path: None,
        project_path: Some(project.to_path_buf()),
        git_head: None,
    })
}

/// Reveals a Scratch target's folder in Finder. Restricted to `Scratch`
/// targets - Worktree and InPlace cwds live under the project the caller
/// already has `open_skill_path` access to, or under the app cache, neither
/// of which this command needs to widen access to.
#[tauri::command]
pub fn reveal_skill_run_target(
    app: AppHandle,
    state: State<SkillRunTargetState>,
    target_id: String,
) -> Result<(), String> {
    let roots = RunTargetRoots::from_app(&app)?;
    let target = resolve_target(&state.targets, &roots, &target_id)?;
    if target.kind != SkillRunTargetKind::Scratch {
        return Err("Only a scratch target's folder can be revealed this way".to_string());
    }
    Command::new("open")
        .args(["-R", &target.cwd.to_string_lossy()])
        .output()
        .map_err(|e| format!("Failed to reveal {}: {e}", target.cwd.display()))?;
    Ok(())
}

// ============================================================================
// Diff / apply / discard
// ============================================================================

/// The unified diff for everything that changed in `cwd` since it was
/// prepared, untracked files included (via a throwaway `git add -N .`, undone
/// right after). Empty string when the tree is clean.
fn diff_for(cwd: &Path) -> Result<String, String> {
    run_git(cwd, &["add", "-N", "."])?;
    let diff = run_git_raw(cwd, &["diff"]);
    // Always undo the intent-to-add, even if `git diff` failed.
    let _ = run_git(cwd, &["reset", "-q"]);
    diff
}

/// The unified diff for a prepared run target, looked up by id.
#[tauri::command]
pub fn skill_run_target_diff(
    app: AppHandle,
    state: State<SkillRunTargetState>,
    target_id: String,
) -> Result<String, String> {
    let roots = RunTargetRoots::from_app(&app)?;
    let target = resolve_target(&state.targets, &roots, &target_id)?;
    diff_for(&target.cwd)
}

/// Every path `git apply --numstat <patch>` reports the patch touching.
fn numstat_paths(project: &Path, patch: &Path) -> Result<Vec<String>, String> {
    let out = run_git(project, &["apply", "--numstat", &patch.to_string_lossy()])?;
    Ok(out
        .lines()
        .filter_map(|line| line.rsplit('\t').next())
        .map(str::to_string)
        .collect())
}

/// Whether `path` (relative to `project`'s root) already existed at HEAD -
/// distinguishes a patch-introduced file (restored by deleting it) from one
/// the patch only modified (restored from the index).
fn existed_at_head(project: &Path, path: &str) -> bool {
    Command::new("git")
        .args(["cat-file", "-e", &format!("HEAD:{path}")])
        .current_dir(project)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Undoes whatever a failed `git apply --3way <patch>` left behind in
/// `project`: tracked paths go back to HEAD via `checkout --`, paths the
/// patch introduced (absent at HEAD) are removed.
fn restore_after_failed_apply(project: &Path, patch: &Path) -> Result<(), String> {
    let paths = numstat_paths(project, patch)?;
    let (tracked, introduced): (Vec<String>, Vec<String>) =
        paths.into_iter().partition(|p| existed_at_head(project, p));
    if !tracked.is_empty() {
        run_git_paths(project, &["checkout", "--"], &tracked)?;
    }
    if !introduced.is_empty() {
        run_git_paths(project, &["clean", "-f", "--"], &introduced)?;
    }
    Ok(())
}

/// Applies a Worktree target's diff onto its stored project with `git apply
/// --3way`, so the worktree's changes fold back into the project. Refuses if
/// the project has uncommitted changes or moved off the commit the worktree
/// was branched from, dry-runs the apply first, and on a real apply failure
/// restores whatever it partially wrote before reporting the error.
fn apply_worktree_diff(target: &PreparedRunTarget) -> Result<(), String> {
    if target.kind != SkillRunTargetKind::Worktree {
        return Err("Only a worktree target's diff can be applied".to_string());
    }
    let project = target.project_path.as_deref().ok_or("Unknown run target")?;

    let diff = diff_for(&target.cwd)?;
    if diff.trim().is_empty() {
        return remove_worktree(target);
    }

    let status = run_git(project, &["status", "--porcelain", "-z"])?;
    if !status.is_empty() {
        return Err(
            "Working tree has uncommitted changes. Commit or stash them before applying."
                .to_string(),
        );
    }
    let head = run_git(project, &["rev-parse", "HEAD"])?;
    if Some(head.as_str()) != target.git_head.as_deref() {
        return Err(
            "The project moved to a different commit since the test started. Run the test again."
                .to_string(),
        );
    }

    let tmp = std::env::temp_dir().join(format!(
        "skill-studio-apply-{}-{}.patch",
        std::process::id(),
        target.id
    ));
    fs::write(&tmp, &diff).map_err(|e| format!("Could not write patch file: {e}"))?;

    let result = (|| -> Result<(), String> {
        run_git(
            project,
            &["apply", "--3way", "--check", &tmp.to_string_lossy()],
        )
        .map_err(|e| format!("Patch does not apply cleanly: {e}"))?;
        run_git(project, &["apply", "--3way", &tmp.to_string_lossy()]).map_err(|apply_err| {
            match restore_after_failed_apply(project, &tmp) {
                Ok(()) => format!("{apply_err}\nThe project was restored."),
                Err(restore_err) => format!("{apply_err}\nRestore failed: {restore_err}"),
            }
        })?;
        Ok(())
    })();
    let _ = fs::remove_file(&tmp);
    result?;

    remove_worktree(target)
}

/// Removes a Worktree target's checkout and directory - shared by a
/// successful/no-op `apply` and by `discard`.
fn remove_worktree(target: &PreparedRunTarget) -> Result<(), String> {
    let Some(path) = &target.cleanup_path else {
        return Ok(());
    };
    let path_str = path.to_string_lossy().to_string();
    // The project path is needed as the cwd `git worktree remove` runs in;
    // `target.cwd` *is* the worktree, so removing it from within itself
    // would fail once the directory is gone. Run from the worktree itself
    // for `list`/fallback `prune`, which don't need the directory to still
    // exist; `git worktree remove` accepts an absolute path regardless of
    // cwd as long as it's run inside *some* checkout of the same repo.
    if run_git(path, &["worktree", "remove", "--force", &path_str]).is_err() {
        let _ = run_git(path, &["worktree", "prune"]);
    }
    let _ = fs::remove_dir_all(path);
    Ok(())
}

/// Applies a prepared Worktree target's diff back onto its project.
#[tauri::command]
pub fn apply_skill_run_target_diff(
    app: AppHandle,
    state: State<SkillRunTargetState>,
    target_id: String,
) -> Result<(), String> {
    let roots = RunTargetRoots::from_app(&app)?;
    let target = resolve_target(&state.targets, &roots, &target_id)?;
    apply_worktree_diff(&target)?;
    state
        .targets
        .lock()
        .map_err(|_| "Could not update run target state".to_string())?
        .remove(&target_id);
    Ok(())
}

/// Splits a `git status --porcelain=v1 -z` entry's status code and path into
/// the buckets `discard_in_place` needs: `checkout_paths` for anything that
/// should come back from HEAD, `clean_files`/`clean_dirs` for anything that
/// should simply be removed. A rename/copy (`R`/`C`) reports two
/// NUL-separated fields - the new path, then the old one - both consumed
/// here so the next entry isn't misread as the old path's status code. `.git`
/// itself is never added to a bucket, but its NUL field is still consumed.
struct DiscardPaths {
    /// Tracked at HEAD - unstaged (if needed), then checked out back to it.
    checkout: Vec<String>,
    /// Staged but absent at HEAD (a plain add, or a rename/copy's new path) -
    /// unstaging turns these untracked, so they're cleaned, not checked out.
    staged_new: Vec<String>,
    /// Already untracked (`??`) - cleaned as-is, no unstaging needed.
    untracked_files: Vec<String>,
    untracked_dirs: Vec<String>,
}

fn parse_discard_paths(porcelain_z: &str) -> DiscardPaths {
    let fields: Vec<&str> = porcelain_z.split('\0').filter(|s| !s.is_empty()).collect();
    let mut result = DiscardPaths {
        checkout: Vec::new(),
        staged_new: Vec::new(),
        untracked_files: Vec::new(),
        untracked_dirs: Vec::new(),
    };

    let mut i = 0;
    while i < fields.len() {
        let entry = fields[i];
        i += 1;
        if entry.len() < 3 {
            continue;
        }
        let code = &entry[0..2];
        let path = entry[3..].to_string();

        let mut old_path: Option<String> = None;
        if code.starts_with('R') || code.starts_with('C') {
            old_path = fields.get(i).map(|s| s.to_string());
            i += 1;
        }

        if !path.starts_with(".git") {
            if code == "??" {
                if let Some(dir) = path.strip_suffix('/') {
                    result.untracked_dirs.push(dir.to_string());
                } else {
                    result.untracked_files.push(path);
                }
            } else if old_path.is_some() || code.starts_with('A') {
                // A rename/copy's new path, or a staged-but-never-committed
                // addition: after unstaging (below) it's untracked, so it
                // needs cleaning, not checkout.
                result.staged_new.push(path);
            } else {
                result.checkout.push(path);
            }
        }
        if let Some(old_path) = old_path {
            if !old_path.is_empty() && !old_path.starts_with(".git") {
                result.checkout.push(old_path);
            }
        }
    }
    result
}

/// Reverts every change `git status` attributes to the run: tracked paths
/// (staged or not) are unstaged and checked out back to HEAD; untracked
/// files/dirs are removed. The prepare-time clean-tree check is the baseline
/// this compares against, so every dirty path found here is the run's.
fn discard_in_place(cwd: &Path) -> Result<(), String> {
    let status = run_git_raw(cwd, &["status", "--porcelain=v1", "-z"])?;
    let paths = parse_discard_paths(&status);

    // `checkout --` restores from the index, so anything staged must be
    // unstaged first, or it would just re-copy its own staged content.
    let mut to_unstage = paths.checkout.clone();
    to_unstage.extend(paths.staged_new.iter().cloned());
    if !to_unstage.is_empty() {
        run_git_paths(cwd, &["reset", "-q", "--"], &to_unstage)?;
    }
    if !paths.checkout.is_empty() {
        run_git_paths(cwd, &["checkout", "--"], &paths.checkout)?;
    }
    let mut to_clean_files = paths.staged_new;
    to_clean_files.extend(paths.untracked_files);
    if !to_clean_files.is_empty() {
        run_git_paths(cwd, &["clean", "-f", "--"], &to_clean_files)?;
    }
    if !paths.untracked_dirs.is_empty() {
        run_git_paths(cwd, &["clean", "-fd", "--"], &paths.untracked_dirs)?;
    }
    Ok(())
}

/// Reverts (InPlace) or removes (Worktree/Scratch) whatever `prepare_skill_run_target`
/// produced.
fn discard_target(target: &PreparedRunTarget) -> Result<(), String> {
    match target.kind {
        SkillRunTargetKind::Worktree => remove_worktree(target),
        SkillRunTargetKind::InPlace => discard_in_place(&target.cwd),
        SkillRunTargetKind::Scratch => {
            let Some(path) = &target.cleanup_path else {
                return Ok(());
            };
            fs::remove_dir_all(path).map_err(|e| format!("Could not remove scratch dir: {e}"))
        }
    }
}

/// Discards a prepared run target, looked up by id.
#[tauri::command]
pub fn discard_skill_run_target(
    app: AppHandle,
    state: State<SkillRunTargetState>,
    target_id: String,
) -> Result<(), String> {
    let roots = RunTargetRoots::from_app(&app)?;
    let target = resolve_target(&state.targets, &roots, &target_id)?;
    discard_target(&target)?;
    state
        .targets
        .lock()
        .map_err(|_| "Could not update run target state".to_string())?
        .remove(&target_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn init_repo(dir: &Path) {
        run_git(dir, &["init", "-q"]).unwrap();
        run_git(dir, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(dir, &["config", "user.name", "Test"]).unwrap();
        fs::write(dir.join("README.md"), "hello\n").unwrap();
        run_git(dir, &["add", "."]).unwrap();
        run_git(dir, &["commit", "-q", "-m", "initial"]).unwrap();
    }

    fn scratch_target(id: &str, cwd: PathBuf) -> PreparedRunTarget {
        PreparedRunTarget {
            id: id.to_string(),
            kind: SkillRunTargetKind::Scratch,
            cwd: cwd.clone(),
            cleanup_path: Some(cwd),
            project_path: None,
            git_head: None,
        }
    }

    #[test]
    fn fixture_parser_rejects_traversal() {
        assert!(parse_fixture("=== ../escape.txt\nbody").is_err());
        assert!(parse_fixture("=== /abs.txt\nbody").is_err());
    }

    #[test]
    fn fixture_parser_splits_multiple_files() {
        let files = parse_fixture("=== a.txt\nfoo\nbar\n=== b/c.txt\nbaz").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].relative_path, "a.txt");
        assert_eq!(files[0].body, "foo\nbar");
        assert_eq!(files[1].relative_path, "b/c.txt");
        assert_eq!(files[1].body, "baz");
    }

    #[test]
    fn in_place_refuses_a_dirty_tree() {
        if !git_available() {
            return;
        }
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("README.md"), "dirty\n").unwrap();

        let request = SkillRunTargetRequest {
            kind: SkillRunTargetKind::InPlace,
            skill_name: "demo".to_string(),
            skill_folder: String::new(),
            extra_skills: vec![],
            fixture: None,
            project_path: Some(dir.path().to_string_lossy().to_string()),
        };
        let err = prepare_in_place(&request, "t".to_string()).unwrap_err();
        assert!(err.contains("uncommitted changes"));
    }

    // ------------------------------------------------------------------
    // F1: id-based lookup and containment
    // ------------------------------------------------------------------

    #[test]
    fn resolve_target_refuses_an_unknown_id() {
        let state = SkillRunTargetState::default();
        let roots = RunTargetRoots {
            scratch: PathBuf::from("/nonexistent-scratch-root"),
            worktrees: PathBuf::from("/nonexistent-worktree-root"),
        };
        // Every id-based command (`diff`, `apply`, `discard`, `reveal`) routes
        // through this same lookup before doing anything else.
        let err = resolve_target(&state.targets, &roots, "forged-id").unwrap_err();
        assert_eq!(err, "Unknown run target");
    }

    #[test]
    fn resolve_target_refuses_a_scratch_target_moved_outside_its_root() {
        let scratch_root = tempdir().unwrap();
        let moved = tempdir().unwrap();
        let state = SkillRunTargetState::default();
        state.targets.lock().unwrap().insert(
            "t".to_string(),
            scratch_target("t", moved.path().to_path_buf()),
        );
        let roots = RunTargetRoots {
            scratch: scratch_root.path().to_path_buf(),
            worktrees: PathBuf::from("/nonexistent-worktree-root"),
        };
        let err = resolve_target(&state.targets, &roots, "t").unwrap_err();
        assert_eq!(err, "Unknown run target");
    }

    #[test]
    fn resolve_target_accepts_a_scratch_target_under_its_root() {
        let scratch_root = tempdir().unwrap();
        let target_dir = scratch_root.path().join("run-1");
        fs::create_dir_all(&target_dir).unwrap();
        let state = SkillRunTargetState::default();
        state
            .targets
            .lock()
            .unwrap()
            .insert("t".to_string(), scratch_target("t", target_dir.clone()));
        let roots = RunTargetRoots {
            scratch: scratch_root.path().to_path_buf(),
            worktrees: PathBuf::from("/nonexistent-worktree-root"),
        };
        let resolved = resolve_target(&state.targets, &roots, "t").unwrap();
        assert_eq!(
            fs::canonicalize(resolved.cwd).unwrap(),
            fs::canonicalize(target_dir).unwrap()
        );
    }

    // ------------------------------------------------------------------
    // F2: in-place discard
    // ------------------------------------------------------------------

    #[test]
    fn in_place_discard_reverts_rename_new_and_modified_files() {
        if !git_available() {
            return;
        }
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("mod.txt"), "orig\n").unwrap();
        run_git(dir.path(), &["add", "."]).unwrap();
        run_git(dir.path(), &["commit", "-q", "-m", "add mod.txt"]).unwrap();

        // Simulate a run's edits: modify a tracked file, add a new one, and
        // rename a tracked file - then stage everything, so `git status`
        // reports the rename as `R` (unstaged renames aren't detected).
        fs::write(dir.path().join("mod.txt"), "changed\n").unwrap();
        fs::write(dir.path().join("new.txt"), "new\n").unwrap();
        fs::rename(dir.path().join("README.md"), dir.path().join("renamed.md")).unwrap();
        run_git(dir.path(), &["add", "-A"]).unwrap();

        discard_in_place(dir.path()).unwrap();

        assert_eq!(
            fs::read_to_string(dir.path().join("mod.txt")).unwrap(),
            "orig\n"
        );
        assert!(!dir.path().join("new.txt").exists());
        assert!(dir.path().join("README.md").exists());
        assert!(!dir.path().join("renamed.md").exists());
        assert!(run_git(dir.path(), &["status", "--porcelain"])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn in_place_discard_handles_paths_with_spaces_and_quotes() {
        if !git_available() {
            return;
        }
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("a file.txt"), "one\n").unwrap();
        fs::write(dir.path().join("a\"quote.txt"), "two\n").unwrap();

        discard_in_place(dir.path()).unwrap();

        assert!(!dir.path().join("a file.txt").exists());
        assert!(!dir.path().join("a\"quote.txt").exists());
    }

    // ------------------------------------------------------------------
    // F3: worktree apply safety
    // ------------------------------------------------------------------

    fn add_worktree(project: &Path, worktree_path: &Path) {
        let toplevel = run_git(project, &["rev-parse", "--show-toplevel"]).unwrap();
        run_git(
            Path::new(&toplevel),
            &[
                "worktree",
                "add",
                "--detach",
                &worktree_path.to_string_lossy(),
                "HEAD",
            ],
        )
        .unwrap();
    }

    #[test]
    fn apply_refuses_when_the_project_moved_off_the_stored_head() {
        if !git_available() {
            return;
        }
        let project = tempdir().unwrap();
        init_repo(project.path());
        let head = run_git(project.path(), &["rev-parse", "HEAD"]).unwrap();
        let cache = tempdir().unwrap();
        let worktree_path = cache.path().join("wt");
        add_worktree(project.path(), &worktree_path);
        fs::write(worktree_path.join("new-file.txt"), "content\n").unwrap();

        // The project moves to a new commit after the worktree was taken.
        fs::write(project.path().join("README.md"), "moved on\n").unwrap();
        run_git(project.path(), &["commit", "-aqm", "move on"]).unwrap();

        let target = PreparedRunTarget {
            id: "t".to_string(),
            kind: SkillRunTargetKind::Worktree,
            cwd: worktree_path.clone(),
            cleanup_path: Some(worktree_path),
            project_path: Some(project.path().to_path_buf()),
            git_head: Some(head),
        };
        let err = apply_worktree_diff(&target).unwrap_err();
        assert!(err.contains("moved to a different commit"));
    }

    #[test]
    fn apply_round_trip_leaves_no_worktree_behind() {
        if !git_available() {
            return;
        }
        let project = tempdir().unwrap();
        init_repo(project.path());
        let head = run_git(project.path(), &["rev-parse", "HEAD"]).unwrap();
        let cache = tempdir().unwrap();
        let worktree_path = cache.path().join("wt");
        add_worktree(project.path(), &worktree_path);
        fs::write(worktree_path.join("new-file.txt"), "content\n").unwrap();

        let target = PreparedRunTarget {
            id: "t".to_string(),
            kind: SkillRunTargetKind::Worktree,
            cwd: worktree_path.clone(),
            cleanup_path: Some(worktree_path),
            project_path: Some(project.path().to_path_buf()),
            git_head: Some(head),
        };
        apply_worktree_diff(&target).unwrap();

        assert!(project.path().join("new-file.txt").exists());
        assert!(!target.cwd.exists());
        let list = run_git(project.path(), &["worktree", "list"]).unwrap();
        assert_eq!(list.lines().count(), 1);
    }

    // ------------------------------------------------------------------
    // F7: worktree baseline excludes setup files
    // ------------------------------------------------------------------

    #[test]
    fn worktree_setup_commit_keeps_the_diff_free_of_skill_files() {
        if !git_available() {
            return;
        }
        let project = tempdir().unwrap();
        init_repo(project.path());
        let skill_source = tempdir().unwrap();
        fs::write(
            skill_source.path().join("SKILL.md"),
            "---\nname: demo\n---\n",
        )
        .unwrap();

        let request = SkillRunTargetRequest {
            kind: SkillRunTargetKind::Worktree,
            skill_name: "demo".to_string(),
            skill_folder: skill_source.path().to_string_lossy().to_string(),
            extra_skills: vec![],
            fixture: None,
            project_path: Some(project.path().to_string_lossy().to_string()),
        };
        let app_cache = tempdir().unwrap();
        let target = prepare_worktree_for_test(&request, app_cache.path(), "t".to_string());

        assert_eq!(diff_for(&target.cwd).unwrap(), "");

        fs::write(target.cwd.join("README.md"), "edited by the agent\n").unwrap();
        let diff = diff_for(&target.cwd).unwrap();
        assert!(diff.contains("README.md"));
        assert!(!diff.contains(".agents/skills/demo"));
        assert!(!diff.contains(".claude/skills/demo"));
    }

    /// `prepare_worktree` without an `AppHandle` - builds the same layout
    /// under a caller-supplied cache root instead of the app cache dir.
    fn prepare_worktree_for_test(
        request: &SkillRunTargetRequest,
        cache_root: &Path,
        id: String,
    ) -> PreparedRunTarget {
        let project_path = request.project_path.as_deref().unwrap();
        let project = Path::new(project_path);
        let toplevel = run_git(project, &["rev-parse", "--show-toplevel"]).unwrap();
        let git_head = run_git(project, &["rev-parse", "HEAD"]).unwrap();
        let worktree_path = cache_root.join("worktrees").join("wt");
        fs::create_dir_all(worktree_path.parent().unwrap()).unwrap();
        run_git(
            Path::new(&toplevel),
            &[
                "worktree",
                "add",
                "--detach",
                &worktree_path.to_string_lossy(),
                "HEAD",
            ],
        )
        .unwrap();
        let skill_dest = worktree_path
            .join(".agents")
            .join("skills")
            .join(&request.skill_name);
        copy_skill_dir_for_run_target(Path::new(&request.skill_folder), &skill_dest).unwrap();
        for agent_dir in [".claude/skills", ".pi/skills"] {
            let link_dir = worktree_path.join(agent_dir);
            fs::create_dir_all(&link_dir).unwrap();
            let target = Path::new("../../.agents/skills").join(&request.skill_name);
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, link_dir.join(&request.skill_name)).unwrap();
        }
        run_git(&worktree_path, &["add", "-A"]).unwrap();
        run_git(
            &worktree_path,
            &[
                "-c",
                "user.name=Skill Studio",
                "-c",
                "user.email=skill-studio@localhost",
                "commit",
                "-q",
                "-m",
                "skill-studio: test setup",
            ],
        )
        .unwrap();
        PreparedRunTarget {
            id,
            kind: SkillRunTargetKind::Worktree,
            cwd: worktree_path.clone(),
            cleanup_path: Some(worktree_path),
            project_path: Some(project.to_path_buf()),
            git_head: Some(git_head),
        }
    }
}
