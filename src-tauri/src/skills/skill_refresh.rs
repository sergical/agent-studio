// ============================================================================
// Skills Module - Background Refresh
// "Never stale, never blocks": a background std thread builds a
// `SkillSnapshot` at startup, stores it in managed state, and rebuilds it
// whenever the filesystem sources it depends on change (skill roots, plugin
// caches, the lock file, Codex config, Claude Code transcripts). Every
// command that needs the current skills reads the cached snapshot instead of
// re-scanning the filesystem on the calling thread.
// ============================================================================

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, Timelike, Utc};
use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::Debouncer;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use super::agents;
use super::lock_file;
use super::project_discovery;
use super::skill_assembly;
use super::skill_discovery;
use super::skill_dto::{Deployment, InstalledSkill};
use super::skill_invocations::{
    InvocationHeatmap, RefreshReport, SkillInvocationIndex, SkillInvocationStats,
};
use super::skill_run_history::{self, SkillRunSummary};

/// Event emitted on the main window whenever the snapshot is (re)built.
pub const SNAPSHOT_EVENT: &str = "skills://snapshot";

/// Debounce window: filesystem events within this window of each other
/// coalesce into a single rebuild.
const DEBOUNCE: Duration = Duration::from_millis(750);

/// How often the background loop wakes up to check the dirty flags, rather
/// than blocking indefinitely on filesystem events, so `request_skill_rescan`
/// (which only sets a flag from another thread) is picked up promptly.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// A lingering invocations-only backlog forces a full rebuild after this long
/// even without a skills-affecting change, so `snapshot.projects` etc. never
/// go too stale just because only transcripts are still being indexed.
const FULL_REBUILD_BACKLOG: Duration = Duration::from_secs(60);

/// Minimum spacing between invocations-only rebuilds, so a burst of
/// transcript writes doesn't reparse and re-emit on every debounce tick.
const INVOCATIONS_REBUILD_INTERVAL: Duration = Duration::from_secs(5);

/// Everything the frontend needs about installed skills, discovered
/// projects, and invocation history, built together in one background pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSnapshot {
    pub skills: Vec<InstalledSkill>,
    pub projects: Vec<String>,
    pub invocations: Vec<SkillInvocationStats>,
    pub heatmap: InvocationHeatmap,
    pub scanned_at: String,
    /// The newest "Test" run outcome per skill, read cheaply from
    /// `skill_run_history::read_last_test_index` - not affected by the
    /// invocations-only rebuild path, only refreshed on a full rebuild.
    #[serde(default)]
    pub last_test_by_skill: BTreeMap<String, SkillRunSummary>,
}

/// One filesystem path the background watcher should track, and whether
/// `notify` should watch it recursively.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WatchPath {
    pub path: PathBuf,
    pub recursive: bool,
}

/// Managed Tauri state, shared between the background refresh thread and
/// every command that can trigger or read a rebuild. Cheap to clone: every
/// field is an `Arc` (or a small owned `PathBuf`), so the background thread
/// works off its own clone rather than borrowing the managed instance.
#[derive(Clone)]
pub struct SkillRefreshState {
    pub snapshot: Arc<RwLock<Option<SkillSnapshot>>>,
    /// Held for the duration of a rebuild so the background loop and the
    /// synchronous command-triggered rebuilds never run concurrently.
    rebuild_lock: Arc<Mutex<()>>,
    /// Project paths a caller (e.g. `get_installed_skills`) asked to be
    /// included, on top of whatever `project_discovery` finds on its own.
    extra_projects: Arc<Mutex<BTreeSet<String>>>,
    /// Project paths the user explicitly stopped tracking (the sidebar's
    /// "Stop tracking" action). Subtracted from the discovered ∪ extra set
    /// on every rebuild, so a project the user removed doesn't reappear just
    /// because `project_discovery` still finds it.
    excluded_projects: Arc<Mutex<BTreeSet<String>>>,
    /// Something that can affect the skills list, project list, or plugin
    /// caches changed; the next rebuild should be a full one.
    skills_dirty: Arc<AtomicBool>,
    /// A Claude Code transcript changed; the next rebuild only needs to
    /// refresh the invocation index, not rescan skill directories.
    invocations_dirty: Arc<AtomicBool>,
    invocation_index: Arc<Mutex<SkillInvocationIndex>>,
    /// The (UTC date, hour) of the last snapshot rebuild - full or
    /// invocations-only. The refresh loop compares this against the current
    /// hour on every tick so the wall-clock-dependent invocation windows in
    /// `SkillInvocationIndex::stats` (24h/7d/14d/30d, by_day) get rebuilt on
    /// an hour boundary even when nothing on disk changed.
    last_built_hour: Arc<Mutex<Option<(NaiveDate, u32)>>>,
    cache_path: PathBuf,
    /// `<app data dir>/skill-studio/runs`, where `skill_run_history` persists
    /// records - read on every full rebuild to fill `last_test_by_skill`.
    runs_root: PathBuf,
}

impl SkillRefreshState {
    /// Add project paths to the always-included set and mark skills dirty,
    /// so both `get_installed_skills` and `register_skill_projects` funnel
    /// through the same bookkeeping.
    pub(crate) fn add_extra_projects(&self, paths: impl IntoIterator<Item = String>) {
        if let Ok(mut extra) = self.extra_projects.lock() {
            extra.extend(paths);
        }
        self.skills_dirty.store(true, Ordering::SeqCst);
    }

    /// Remove a caller-registered project path so future rebuilds stop
    /// including it, mark it excluded so `project_discovery` can't bring it
    /// back on its own, and mark skills dirty so the next background pass
    /// reflects the removal.
    pub(crate) fn remove_extra_project(&self, path: &str) {
        if let Ok(mut extra) = self.extra_projects.lock() {
            extra.remove(path);
        }
        if let Ok(mut excluded) = self.excluded_projects.lock() {
            excluded.insert(path.to_string());
        }
        self.skills_dirty.store(true, Ordering::SeqCst);
    }

    /// Remove project paths from the excluded set, so a caller that
    /// explicitly registers a project (e.g. re-adding it in the sidebar)
    /// overrides a previous "stop tracking".
    pub(crate) fn unexclude_projects(&self, paths: impl IntoIterator<Item = String>) {
        if let Ok(mut excluded) = self.excluded_projects.lock() {
            for path in paths {
                excluded.remove(&path);
            }
        }
    }

    /// The caller-registered project paths, as `PathBuf`s.
    fn extra_project_paths(&self) -> Vec<PathBuf> {
        self.extra_projects
            .lock()
            .map(|guard| guard.iter().map(PathBuf::from).collect())
            .unwrap_or_default()
    }

    /// The excluded project paths.
    fn excluded_project_set(&self) -> BTreeSet<String> {
        self.excluded_projects
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Record that a rebuild just completed at `now`, so `is_hour_stale`
    /// doesn't immediately fire again for the same hour.
    fn mark_built_at(&self, now: DateTime<Utc>) {
        if let Ok(mut guard) = self.last_built_hour.lock() {
            *guard = Some(hour_key(now));
        }
    }

    /// True when the wall-clock hour has moved on since the last rebuild (or
    /// there's never been one), meaning the rolling invocation windows in
    /// `stats()` may now be stale even though nothing on disk changed.
    fn is_hour_stale(&self, now: DateTime<Utc>) -> bool {
        self.last_built_hour
            .lock()
            .map(|guard| *guard != Some(hour_key(now)))
            .unwrap_or(true)
    }
}

/// The (UTC date, hour) `now` falls in, used to detect an hour boundary
/// crossing between refresh-loop ticks.
fn hour_key(now: DateTime<Utc>) -> (NaiveDate, u32) {
    (now.date_naive(), now.hour())
}

/// Start the background refresh thread and return the state to register
/// with `tauri::Builder::manage`.
pub fn init(app: &AppHandle) -> SkillRefreshState {
    let cache_path = invocation_cache_path(app);
    let invocation_index = SkillInvocationIndex::load_or_empty(&cache_path);

    let state = SkillRefreshState {
        snapshot: Arc::new(RwLock::new(None)),
        rebuild_lock: Arc::new(Mutex::new(())),
        extra_projects: Arc::new(Mutex::new(BTreeSet::new())),
        excluded_projects: Arc::new(Mutex::new(BTreeSet::new())),
        skills_dirty: Arc::new(AtomicBool::new(false)),
        invocations_dirty: Arc::new(AtomicBool::new(false)),
        invocation_index: Arc::new(Mutex::new(invocation_index)),
        last_built_hour: Arc::new(Mutex::new(None)),
        cache_path,
        runs_root: app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("skill-studio")
            .join("runs"),
    };

    let app_handle = app.clone();
    let loop_state = state.clone();
    std::thread::spawn(move || run_refresh_loop(app_handle, loop_state));

    state
}

/// Instant read of the current snapshot from managed state.
#[tauri::command]
pub fn get_skill_snapshot(state: tauri::State<SkillRefreshState>) -> Option<SkillSnapshot> {
    state.snapshot.read().ok().and_then(|guard| guard.clone())
}

/// Ask the background thread to rebuild the snapshot. Returns immediately;
/// the rebuild happens asynchronously and a fresh `SNAPSHOT_EVENT` follows.
#[tauri::command]
pub fn request_skill_rescan(state: tauri::State<SkillRefreshState>) {
    state.skills_dirty.store(true, Ordering::SeqCst);
}

/// True when `path` is the same directory as `home` - the global scope, not
/// a project. Compares canonicalized paths so `~` vs. its resolved form (or
/// a trailing slash) still matches; falls back to a direct comparison when
/// either side can't be canonicalized (e.g. a path that doesn't exist yet).
fn is_home_directory(path: &Path, home: &Path) -> bool {
    match (std::fs::canonicalize(path), std::fs::canonicalize(home)) {
        (Ok(p), Ok(h)) => p == h,
        _ => path == home,
    }
}

/// Drops any path in `paths` that is `home` - the global scope, not a
/// project, even though it can contain `.claude/skills` etc. - keeping the
/// rest. A single legacy home-dir entry (e.g. from a persisted project list)
/// shouldn't disable every other path in the same batch. Pulled out of
/// `register_skill_projects` so it's testable without a `tauri::State`.
fn drop_home_directory_from_batch(paths: Vec<String>, home: &Path) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| {
            let keep = !is_home_directory(Path::new(path), home);
            if !keep {
                eprintln!("[register_skill_projects] dropping home directory from batch: {path}");
            }
            keep
        })
        .collect()
}

/// Register project paths the caller cares about (e.g. one the user just
/// opened) so future rebuilds always include them, even though
/// `project_discovery` hasn't found them via a Codex/Claude Code config yet.
/// Returns immediately on success; a full rebuild follows on the background
/// thread.
#[tauri::command]
pub fn register_skill_projects(
    paths: Vec<String>,
    state: tauri::State<SkillRefreshState>,
) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let valid = drop_home_directory_from_batch(paths, &home);
    state.unexclude_projects(valid.clone());
    state.add_extra_projects(valid);
    Ok(())
}

/// Un-register a caller-registered project path (e.g. one the user closed)
/// so future rebuilds stop including it. Returns immediately; a full rebuild
/// follows on the background thread.
#[tauri::command]
pub fn unregister_skill_project(path: String, state: tauri::State<SkillRefreshState>) {
    state.remove_extra_project(&path);
}

/// Build a full snapshot right now on the calling thread, store it, and emit
/// `SNAPSHOT_EVENT`. Used both by the background loop's full-rebuild path and
/// by commands that need the caller's next read to see fresh data (a new
/// project's skills, or the result of an install/remove/update). Blocks on
/// `rebuild_lock` so it never overlaps another rebuild. On error the
/// previous snapshot is left in place.
pub fn rebuild_snapshot_now(
    app: &AppHandle,
    state: &SkillRefreshState,
) -> Result<SkillSnapshot, String> {
    let _guard = state
        .rebuild_lock
        .lock()
        .map_err(|e| format!("rebuild lock poisoned: {e}"))?;

    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let extra_projects = state.extra_project_paths();
    let excluded_projects = state.excluded_project_set();

    let mut invocation_index = state
        .invocation_index
        .lock()
        .map_err(|e| format!("invocation index lock poisoned: {e}"))?;
    // Captured once and threaded through stats/heatmap/scanned_at/mark_built_at
    // below, so a rebuild that straddles an hour boundary doesn't record the
    // new hour against cutoffs computed for the old one.
    let now = Utc::now();
    let (built, report) = build_snapshot(
        &home,
        &extra_projects,
        &excluded_projects,
        &mut invocation_index,
        &state.cache_path,
        &state.runs_root,
        now,
    );
    drop(invocation_index);

    if report.incomplete {
        state.invocations_dirty.store(true, Ordering::SeqCst);
    }

    match state.snapshot.write() {
        Ok(mut guard) => *guard = Some(built.clone()),
        Err(e) => return Err(format!("snapshot lock poisoned: {e}")),
    }
    state.mark_built_at(now);

    app.emit(SNAPSHOT_EVENT, &built)
        .map_err(|e| format!("failed to emit {SNAPSHOT_EVENT}: {e}"))?;
    Ok(built)
}

/// Refresh only the invocation index and recompute stats/heatmap, reusing
/// `skills`/`projects` from the current snapshot rather than rescanning skill
/// directories. Cheaper than `rebuild_snapshot_now`, used for the frequent
/// case of "a transcript changed" so a burst of agent activity doesn't
/// trigger a full directory rescan every few seconds.
fn rebuild_invocations_only(app: &AppHandle, state: &SkillRefreshState) -> Result<(), String> {
    let _guard = state
        .rebuild_lock
        .lock()
        .map_err(|e| format!("rebuild lock poisoned: {e}"))?;

    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let mut invocation_index = state
        .invocation_index
        .lock()
        .map_err(|e| format!("invocation index lock poisoned: {e}"))?;
    let report = invocation_index.refresh(&home.join(".claude/projects"));
    if let Err(e) = invocation_index.save(&state.cache_path) {
        eprintln!("skill refresh: failed to save invocation cache: {e}");
    }
    // Captured once and threaded through stats/heatmap/scanned_at/mark_built_at
    // below, so a rebuild that straddles an hour boundary doesn't record the
    // new hour against cutoffs computed for the old one.
    let now = Utc::now();
    let invocations = invocation_index.stats_at(now);
    let heatmap = invocation_index.heatmap_at(365, now);
    drop(invocation_index);

    if report.incomplete {
        state.invocations_dirty.store(true, Ordering::SeqCst);
    }

    let built = {
        let mut guard = state
            .snapshot
            .write()
            .map_err(|e| format!("snapshot lock poisoned: {e}"))?;
        let Some(snapshot) = guard.as_mut() else {
            return Ok(()); // no full snapshot yet; the next full rebuild covers this
        };
        snapshot.invocations = invocations;
        snapshot.heatmap = heatmap;
        snapshot.scanned_at = now.to_rfc3339();
        snapshot.clone()
    };
    state.mark_built_at(now);

    app.emit(SNAPSHOT_EVENT, &built)
        .map_err(|e| format!("failed to emit {SNAPSHOT_EVENT}: {e}"))
}

/// True when `path` is inside `snapshot`: it canonicalizes to the same path
/// as one of its deployments' folders, or that folder's `SKILL.md`. Used to
/// reject `read_installed_skill_md` / `open_skill_path` requests for paths
/// outside anything the snapshot actually deployed, so a caller can't read or
/// open an arbitrary file on disk.
pub fn snapshot_owns_path(snapshot: &SkillSnapshot, path: &Path) -> bool {
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return false;
    };
    snapshot
        .skills
        .iter()
        .flat_map(|s| &s.deployments)
        .any(|d| {
            let Ok(dep_path) = std::fs::canonicalize(&d.path) else {
                return false;
            };
            canonical == dep_path || canonical == dep_path.join("SKILL.md")
        })
}

/// The deployment in `snapshot` that owns `path`: its folder canonicalizes to
/// `path`'s parent, or to `path` itself when `path` is `SKILL.md`. Used by
/// `write_installed_skill_md` to find the deployment's `plugin` field (writes
/// to a plugin-owned skill are refused) without re-deriving the same
/// containment check `snapshot_owns_path` already does.
pub fn snapshot_deployment_owning_path<'a>(
    snapshot: &'a SkillSnapshot,
    path: &Path,
) -> Option<&'a Deployment> {
    let canonical = std::fs::canonicalize(path).ok()?;
    snapshot
        .skills
        .iter()
        .flat_map(|s| &s.deployments)
        .find(|d| {
            let Ok(dep_path) = std::fs::canonicalize(&d.path) else {
                return false;
            };
            canonical == dep_path || canonical == dep_path.join("SKILL.md")
        })
}

/// The cache file the invocation index is persisted to between runs.
fn invocation_cache_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("skill-invocations.json")
}

/// Runs for the app's lifetime on its own std thread (never the async
/// runtime): starts the filesystem watcher, builds the initial snapshot, then
/// rebuilds on change (full or invocations-only, depending on what's dirty)
/// or on an explicit rescan request. Every error is logged with `eprintln!`
/// and never panics the thread; a failed rebuild simply keeps the previous
/// snapshot in place.
fn run_refresh_loop(app: AppHandle, state: SkillRefreshState) {
    let Some(home) = dirs::home_dir() else {
        eprintln!("skill refresh: could not find home directory, giving up");
        return;
    };
    let claude_projects_dir = home.join(".claude/projects");

    let (tx, rx) = mpsc::channel();
    let mut debouncer = match new_debouncer(DEBOUNCE, tx) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("skill refresh: failed to start filesystem watcher: {e}");
            return;
        }
    };
    let mut watched: BTreeSet<PathBuf> = BTreeSet::new();

    // Start watching before the initial scan so a change made while the
    // first scan is running is never missed.
    let initial_projects = project_discovery::discover_skill_projects(&home);
    reconcile_watchers(
        &mut debouncer,
        &mut watched,
        &desired_watch_paths(&home, &initial_projects),
    );

    if let Err(e) = rebuild_snapshot_now(&app, &state) {
        eprintln!("skill refresh: initial rebuild failed: {e}");
    }
    reconcile_watchers_from_snapshot(&home, &state, &mut debouncer, &mut watched);

    let mut last_full_rebuild = Instant::now();
    let mut last_invocations_rebuild = Instant::now();

    loop {
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(Ok(events)) => {
                for event in events {
                    let known_transcript = state
                        .invocation_index
                        .lock()
                        .map(|idx| idx.knows_file(&event.path))
                        .unwrap_or(false);
                    match classify_watch_event(&event.path, &claude_projects_dir, known_transcript)
                    {
                        WatchEventKind::Skills => state.skills_dirty.store(true, Ordering::SeqCst),
                        WatchEventKind::Invocations => {
                            state.invocations_dirty.store(true, Ordering::SeqCst)
                        }
                    }
                }
            }
            Ok(Err(err)) => eprintln!("skill refresh: watch error: {err}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let skills_dirty = state.skills_dirty.load(Ordering::SeqCst);
        let invocations_dirty = state.invocations_dirty.load(Ordering::SeqCst);
        let backlog_stale = invocations_dirty && last_full_rebuild.elapsed() > FULL_REBUILD_BACKLOG;

        if skills_dirty || backlog_stale {
            // Clear the flags before rebuilding so an event that arrives
            // mid-rebuild sets them again rather than being lost.
            state.skills_dirty.store(false, Ordering::SeqCst);
            state.invocations_dirty.store(false, Ordering::SeqCst);
            match rebuild_snapshot_now(&app, &state) {
                Ok(_) => {
                    last_full_rebuild = Instant::now();
                    last_invocations_rebuild = Instant::now();
                    reconcile_watchers_from_snapshot(&home, &state, &mut debouncer, &mut watched);
                }
                Err(e) => eprintln!("skill refresh: full rebuild failed: {e}"),
            }
        } else if invocations_dirty
            && last_invocations_rebuild.elapsed() > INVOCATIONS_REBUILD_INTERVAL
        {
            state.invocations_dirty.store(false, Ordering::SeqCst);
            if let Err(e) = rebuild_invocations_only(&app, &state) {
                eprintln!("skill refresh: invocations-only rebuild failed: {e}");
            }
            last_invocations_rebuild = Instant::now();
        } else if state.is_hour_stale(Utc::now()) {
            // Nothing on disk changed, but the wall clock crossed an hour
            // boundary: the rolling invocation windows need recomputing even
            // though `skills`/`projects` don't.
            if let Err(e) = rebuild_invocations_only(&app, &state) {
                eprintln!("skill refresh: hourly rebuild failed: {e}");
            }
            last_invocations_rebuild = Instant::now();
        }
    }
}

/// Reconcile the watch set against the paths implied by the current
/// snapshot's projects (falling back to a fresh discovery pass if there's no
/// snapshot yet, which only happens before the very first rebuild).
fn reconcile_watchers_from_snapshot(
    home: &Path,
    state: &SkillRefreshState,
    debouncer: &mut Debouncer<RecommendedWatcher>,
    watched: &mut BTreeSet<PathBuf>,
) {
    let projects: Vec<PathBuf> = state
        .snapshot
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(|s| s.projects.clone()))
        .unwrap_or_else(|| {
            project_discovery::discover_skill_projects(home)
                .into_iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect()
        })
        .into_iter()
        .map(PathBuf::from)
        .collect();
    reconcile_watchers(debouncer, watched, &desired_watch_paths(home, &projects));
}

/// Watch every existing path in `desired` that isn't already watched, and
/// unwatch every currently-watched path that's no longer in `desired` (it
/// vanished, or the project it belonged to left the desired set). Notify
/// errors are logged, never propagated: a watch failure on one path
/// shouldn't stop the others from being (un)watched.
fn reconcile_watchers(
    debouncer: &mut Debouncer<RecommendedWatcher>,
    watched: &mut BTreeSet<PathBuf>,
    desired: &[WatchPath],
) {
    let desired_paths: BTreeSet<&PathBuf> = desired.iter().map(|w| &w.path).collect();

    let stale: Vec<PathBuf> = watched
        .iter()
        .filter(|p| !desired_paths.contains(p))
        .cloned()
        .collect();
    for path in stale {
        if let Err(e) = debouncer.watcher().unwatch(&path) {
            eprintln!("skill refresh: failed to unwatch {}: {e}", path.display());
        }
        watched.remove(&path);
    }

    for wp in desired {
        if watched.contains(&wp.path) || !wp.path.exists() {
            continue;
        }
        let mode = if wp.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        match debouncer.watcher().watch(&wp.path, mode) {
            Ok(()) => {
                watched.insert(wp.path.clone());
            }
            Err(e) => eprintln!("skill refresh: failed to watch {}: {e}", wp.path.display()),
        }
    }
}

/// Build a fresh snapshot from `home` plus `extra_projects`, refreshing the
/// invocation index along the way. Pure aside from the filesystem reads, so
/// it's the unit under test for "a caller-registered project's skills show
/// up in the snapshot" without needing a running Tauri app.
fn build_snapshot(
    home: &Path,
    extra_projects: &[PathBuf],
    excluded_projects: &BTreeSet<String>,
    invocation_index: &mut SkillInvocationIndex,
    cache_path: &Path,
    runs_root: &Path,
    now: DateTime<Utc>,
) -> (SkillSnapshot, RefreshReport) {
    let mut project_paths: BTreeSet<PathBuf> = project_discovery::discover_skill_projects(home)
        .into_iter()
        .collect();
    project_paths.extend(extra_projects.iter().cloned());
    let project_paths: Vec<PathBuf> = project_paths
        .into_iter()
        .filter(|p| !excluded_projects.contains(&p.to_string_lossy().to_string()))
        // The home directory is the global scope (it holds ~/.claude/skills,
        // ~/.agents/skills, ...), never a project - even if a stray session
        // transcript recorded it as a cwd.
        .filter(|p| !is_home_directory(p, home))
        .collect();

    let candidates = skill_discovery::discover_skill_candidates(home, &project_paths);
    let lock = lock_file::read_lock_file().unwrap_or_else(|e| {
        eprintln!("skill refresh: failed to read lock file: {e}");
        lock_file::SkillLockFile {
            version: 3,
            skills: std::collections::HashMap::new(),
        }
    });
    let skills = skill_assembly::assemble_installed_skills(candidates, &lock);

    let report = invocation_index.refresh(&home.join(".claude/projects"));
    if let Err(e) = invocation_index.save(cache_path) {
        eprintln!("skill refresh: failed to save invocation cache: {e}");
    }

    let skill_names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
    let last_test_by_skill = skill_run_history::read_last_test_index(runs_root, &skill_names)
        .into_iter()
        .collect();

    let snapshot = SkillSnapshot {
        skills,
        projects: project_paths
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        invocations: invocation_index.stats_at(now),
        heatmap: invocation_index.heatmap_at(365, now),
        scanned_at: now.to_rfc3339(),
        last_test_by_skill,
    };
    (snapshot, report)
}

/// Which kind of rebuild a single filesystem-watch event implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEventKind {
    /// Rebuild the full snapshot (skills, projects, plugin caches, ...).
    Skills,
    /// Only the invocation index needs to be refreshed.
    Invocations,
}

/// Classify a single filesystem-watch event, given whether `path` is already
/// a transcript the invocation index tracks. An event outside
/// `claude_projects_dir` always needs a full rebuild - everything that lives
/// there (skill roots, plugin caches, the lock file, project discovery
/// sources) can change `snapshot.skills` or `snapshot.projects`. An event
/// inside it needs a full rebuild too when it's for a path the invocation
/// index doesn't already know about: a brand-new transcript file, or a file
/// in a brand-new project directory, either of which can also change
/// `snapshot.projects`. A change to an already-tracked transcript that still
/// exists only needs the invocation index refreshed; a deleted one needs a
/// full rebuild because project discovery may have depended on it.
pub fn classify_watch_event(
    path: &Path,
    claude_projects_dir: &Path,
    known_transcript: bool,
) -> WatchEventKind {
    if !path.starts_with(claude_projects_dir) {
        return WatchEventKind::Skills;
    }
    // A known transcript that no longer exists was deleted or renamed: that
    // can remove a transcript-discovered project, so it needs a full rebuild.
    if known_transcript && path.is_file() {
        WatchEventKind::Invocations
    } else {
        WatchEventKind::Skills
    }
}

/// Every filesystem path a change to which should trigger a rebuild, given
/// the currently known project paths: each global skill root and its parent
/// (so a directory created later is still picked up), each native plugin
/// cache and its parent, the lock file's and Codex config's containing
/// directories, the Claude Code transcripts directory (recursive, since
/// invocations and project discovery both depend on it) and its parent, and
/// each project's first-class-agent skill directories plus the project root
/// itself (non-recursive, so a `.claude` etc. created later is still seen).
pub fn desired_watch_paths(home: &Path, projects: &[PathBuf]) -> Vec<WatchPath> {
    let mut merged: BTreeMap<PathBuf, bool> = BTreeMap::new();
    let add = |merged: &mut BTreeMap<PathBuf, bool>, path: PathBuf, recursive: bool| {
        let entry = merged.entry(path).or_insert(false);
        *entry = *entry || recursive;
    };

    for root in agents::skill_roots(home, &[]) {
        if root.project_path.is_some() {
            continue; // global roots only; project roots are handled below
        }
        add(&mut merged, root.path.clone(), true);
        if let Some(parent) = root.path.parent() {
            add(&mut merged, parent.to_path_buf(), false);
        }
    }

    for cache_dir in [
        home.join(".claude/plugins/cache"),
        home.join(".codex/plugins/cache"),
    ] {
        if let Some(parent) = cache_dir.parent() {
            add(&mut merged, parent.to_path_buf(), false);
        }
        add(&mut merged, cache_dir, true);
    }

    add(&mut merged, home.join(".agents"), false);
    add(&mut merged, home.join(".codex"), false);
    add(&mut merged, home.join(".claude/projects"), true);
    add(&mut merged, home.join(".claude"), false);

    for project in projects {
        for sub in [".claude", ".codex", ".opencode", ".pi", ".agents"] {
            add(&mut merged, project.join(sub), true);
        }
        add(&mut merged, project.clone(), false);
    }

    merged
        .into_iter()
        .map(|(path, recursive)| WatchPath { path, recursive })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn desired_watch_paths_includes_global_roots_and_parents() {
        let home = PathBuf::from("/home/tester");
        let paths = desired_watch_paths(&home, &[]);

        let claude_skills = home.join(".claude/skills");
        assert!(paths.iter().any(|w| w.path == claude_skills && w.recursive));
        assert!(paths
            .iter()
            .any(|w| w.path == home.join(".claude") && !w.recursive));
    }

    #[test]
    fn desired_watch_paths_includes_project_entries() {
        let home = PathBuf::from("/home/tester");
        let project = PathBuf::from("/work/my-project");
        let paths = desired_watch_paths(&home, std::slice::from_ref(&project));

        assert!(paths
            .iter()
            .any(|w| w.path == project.join(".claude") && w.recursive));
        assert!(paths.iter().any(|w| w.path == project && !w.recursive));
    }

    #[test]
    fn desired_watch_paths_watches_claude_projects_recursively() {
        let home = PathBuf::from("/home/tester");
        let paths = desired_watch_paths(&home, &[]);
        assert!(paths
            .iter()
            .any(|w| w.path == home.join(".claude/projects") && w.recursive));
    }

    #[test]
    fn desired_watch_paths_has_no_duplicate_paths() {
        let home = PathBuf::from("/home/tester");
        let mut paths: Vec<PathBuf> = desired_watch_paths(&home, &[])
            .into_iter()
            .map(|w| w.path)
            .collect();
        let before = paths.len();
        paths.sort();
        paths.dedup();
        assert_eq!(before, paths.len());
    }

    #[test]
    fn classify_watch_event_outside_claude_projects_is_skills() {
        let claude_projects = PathBuf::from("/home/tester/.claude/projects");
        let path = PathBuf::from("/home/tester/.claude/skills/foo/SKILL.md");
        assert_eq!(
            classify_watch_event(&path, &claude_projects, false),
            WatchEventKind::Skills
        );
    }

    #[test]
    fn classify_watch_event_new_transcript_is_skills() {
        let claude_projects = PathBuf::from("/home/tester/.claude/projects");
        let path = claude_projects.join("-my-project/session.jsonl");
        assert_eq!(
            classify_watch_event(&path, &claude_projects, false),
            WatchEventKind::Skills
        );
    }

    #[test]
    fn classify_watch_event_known_transcript_is_invocations() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_projects = tmp.path().join("projects");
        let path = claude_projects.join("-my-project/session.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{}\n").unwrap();
        assert_eq!(
            classify_watch_event(&path, &claude_projects, true),
            WatchEventKind::Invocations
        );
    }

    #[test]
    fn classify_watch_event_deleted_known_transcript_is_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_projects = tmp.path().join("projects");
        let path = claude_projects.join("-my-project/session.jsonl");
        assert_eq!(
            classify_watch_event(&path, &claude_projects, true),
            WatchEventKind::Skills
        );
    }

    #[test]
    fn build_snapshot_includes_caller_only_project() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("caller-project");
        fs::create_dir_all(project.join(".claude/skills/foo")).unwrap();
        fs::write(
            project.join(".claude/skills/foo/SKILL.md"),
            "---\nname: foo\ndescription: test\n---\nbody",
        )
        .unwrap();
        fs::create_dir_all(&home).unwrap();

        let mut invocation_index = SkillInvocationIndex::default();
        let cache_path = tmp.path().join("cache.json");
        let (snapshot, _report) = build_snapshot(
            &home,
            std::slice::from_ref(&project),
            &BTreeSet::new(),
            &mut invocation_index,
            &cache_path,
            tmp.path(),
            Utc::now(),
        );

        assert!(snapshot
            .projects
            .contains(&project.to_string_lossy().to_string()));
        assert!(snapshot.skills.iter().any(|s| s.name == "foo"));
    }

    #[test]
    fn build_snapshot_excludes_stopped_tracking_project() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("caller-project");
        fs::create_dir_all(project.join(".claude/skills/foo")).unwrap();
        fs::write(
            project.join(".claude/skills/foo/SKILL.md"),
            "---\nname: foo\ndescription: test\n---\nbody",
        )
        .unwrap();
        fs::create_dir_all(&home).unwrap();

        let mut excluded = BTreeSet::new();
        excluded.insert(project.to_string_lossy().to_string());

        let mut invocation_index = SkillInvocationIndex::default();
        let cache_path = tmp.path().join("cache.json");
        let (snapshot, _report) = build_snapshot(
            &home,
            std::slice::from_ref(&project),
            &excluded,
            &mut invocation_index,
            &cache_path,
            tmp.path(),
            Utc::now(),
        );

        assert!(!snapshot
            .projects
            .contains(&project.to_string_lossy().to_string()));
    }

    #[test]
    fn build_snapshot_excludes_home_directory_from_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".claude/skills/foo")).unwrap();
        fs::write(
            home.join(".claude/skills/foo/SKILL.md"),
            "---\nname: foo\ndescription: test\n---\nbody",
        )
        .unwrap();

        let mut invocation_index = SkillInvocationIndex::default();
        let cache_path = tmp.path().join("cache.json");
        // The home dir sneaks in as an "extra project" here the same way a
        // stray session transcript with cwd == home would via discovery.
        let (snapshot, _report) = build_snapshot(
            &home,
            std::slice::from_ref(&home),
            &BTreeSet::new(),
            &mut invocation_index,
            &cache_path,
            tmp.path(),
            Utc::now(),
        );

        assert!(!snapshot
            .projects
            .contains(&home.to_string_lossy().to_string()));
        // The skill is still discovered - just not attributed to a project.
        assert!(snapshot.skills.iter().any(|s| s.name == "foo"));
        assert!(snapshot
            .skills
            .iter()
            .any(|s| s.deployments.iter().all(|d| d.scope != "project")));
    }

    #[test]
    fn register_skill_projects_drops_only_the_home_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let valid_project = tmp.path().join("project");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&valid_project).unwrap();

        let batch = vec![
            home.to_string_lossy().to_string(),
            valid_project.to_string_lossy().to_string(),
        ];
        let result = drop_home_directory_from_batch(batch, &home);

        assert_eq!(result, vec![valid_project.to_string_lossy().to_string()]);
    }

    /// Build a minimal `SkillSnapshot` with one skill deployed at `dep_dir`,
    /// for `snapshot_owns_path` tests.
    fn fixture_snapshot(dep_dir: &Path) -> SkillSnapshot {
        use super::super::provenance::SourceKind;
        use super::super::skill_dto::{Deployment, InstalledSkill};

        SkillSnapshot {
            skills: vec![InstalledSkill {
                name: "foo".to_string(),
                source: "manual".to_string(),
                source_type: "manual".to_string(),
                source_url: None,
                skill_path: None,
                installed_at: Utc::now().to_rfc3339(),
                updated_at: None,
                has_update: false,
                source_kind: SourceKind::Manual,
                deployments: vec![Deployment {
                    agent: "Claude Code".to_string(),
                    scope: "project".to_string(),
                    path: dep_dir.to_string_lossy().to_string(),
                    is_symlink: false,
                    plugin: None,
                    symlink_target: None,
                    symlink_is_broken: false,
                    symlink_error: None,
                    project_path: None,
                    content_hash: String::new(),
                }],
                has_spec: false,
                description: None,
                spec_violations: Vec::new(),
                skill_md_tokens: 0,
                folder_bytes: 0,
                file_count: 0,
                content_hash: String::new(),
                content_hashes: Vec::new(),
                modified_at: None,
                frontmatter_fields: BTreeMap::new(),
                folder_truncated: false,
            }],
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: InvocationHeatmap::default(),
            scanned_at: Utc::now().to_rfc3339(),
            last_test_by_skill: Default::default(),
        }
    }

    #[test]
    fn snapshot_owns_path_rejects_path_outside_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let dep_dir = tmp.path().join("foo");
        fs::create_dir_all(&dep_dir).unwrap();
        fs::write(dep_dir.join("SKILL.md"), "body").unwrap();
        let outside = tmp.path().join("outside.md");
        fs::write(&outside, "body").unwrap();

        let snapshot = fixture_snapshot(&dep_dir);
        assert!(!snapshot_owns_path(&snapshot, &outside));
    }

    /// A `SkillRefreshState` with no snapshot, for `mark_built_at`/
    /// `is_hour_stale` tests that don't need a running Tauri app.
    fn fixture_state() -> SkillRefreshState {
        SkillRefreshState {
            snapshot: Arc::new(RwLock::new(None)),
            rebuild_lock: Arc::new(Mutex::new(())),
            extra_projects: Arc::new(Mutex::new(BTreeSet::new())),
            excluded_projects: Arc::new(Mutex::new(BTreeSet::new())),
            skills_dirty: Arc::new(AtomicBool::new(false)),
            invocations_dirty: Arc::new(AtomicBool::new(false)),
            invocation_index: Arc::new(Mutex::new(SkillInvocationIndex::default())),
            last_built_hour: Arc::new(Mutex::new(None)),
            cache_path: PathBuf::from("/dev/null"),
            runs_root: PathBuf::from("/dev/null"),
        }
    }

    #[test]
    fn is_hour_stale_reports_fresh_for_the_same_captured_now() {
        let state = fixture_state();
        let now = Utc::now();
        state.mark_built_at(now);
        assert!(!state.is_hour_stale(now));
    }

    #[test]
    fn is_hour_stale_reports_stale_an_hour_after_the_captured_now() {
        let state = fixture_state();
        let now = Utc::now();
        state.mark_built_at(now);
        let an_hour_later = now + chrono::Duration::hours(1);
        assert!(state.is_hour_stale(an_hour_later));
    }

    #[test]
    fn snapshot_owns_path_accepts_deployment_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let dep_dir = tmp.path().join("foo");
        fs::create_dir_all(&dep_dir).unwrap();
        let skill_md = dep_dir.join("SKILL.md");
        fs::write(&skill_md, "body").unwrap();

        let snapshot = fixture_snapshot(&dep_dir);
        assert!(snapshot_owns_path(&snapshot, &skill_md));
    }
}
