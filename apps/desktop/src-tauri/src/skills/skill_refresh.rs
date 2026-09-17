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
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_mini::Debouncer;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use super::agents;
use super::project_discovery;
use super::skill_discovery;
use super::skill_dto::{Deployment, InstalledSkill};
use super::skill_fork_registry::TrialScope;
use super::skill_invocations::{
    InvocationHeatmap, RefreshReport, SkillInvocationIndex, SkillInvocationStats,
};
use super::skill_refresh_demand::RefreshDemand;
use super::skill_run_history::{self, SkillRunSummary};
use super::skill_update_check::{self, UpdateCheckSummary};
use skill_studio_core::skill_ledger_inventory::LedgerOnlySkill;
use skill_studio_core::skill_service::{
    InventoryRead, ReplacementSafety, ScanError, ScopedSkillService, SkillScope,
};

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
    /// Process-local publication order. Zero is reserved for snapshots read
    /// from older serialized data that predates revisions.
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub full_refresh: Option<super::skill_refresh_demand::SkillRefreshPosition>,
    #[serde(default)]
    pub ledger_only: Vec<LedgerOnlySkill>,
    #[serde(default)]
    pub diagnosis: Option<skill_studio_core::skill_diagnosis::Diagnosis>,
    pub skills: Vec<InstalledSkill>,
    #[serde(default)]
    pub read_warnings: Vec<SkillSnapshotReadWarning>,
    pub projects: Vec<String>,
    pub invocations: Vec<SkillInvocationStats>,
    pub heatmap: InvocationHeatmap,
    pub scanned_at: String,
    /// The newest "Test" run outcome per skill, read cheaply from
    /// `skill_run_history::read_last_test_index` - not affected by the
    /// invocations-only rebuild path, only refreshed on a full rebuild.
    #[serde(default)]
    pub last_test_by_skill: BTreeMap<String, SkillRunSummary>,
    /// The latest background update-check result - see `skill_update_check`.
    #[serde(default)]
    pub update_check: UpdateCheckSummary,
    /// Which OpenCode config format is present, if any - `None` when
    /// OpenCode isn't configured, `Some(Jsonc)` when Skill Studio can only
    /// read (not write) its per-skill disables. See
    /// `opencode_skill_permission::detect_config_kind`.
    #[serde(default)]
    pub opencode_config_kind: Option<super::opencode_skill_permission::OpencodeConfigKind>,
}

/// A scoped input failure that makes one part of snapshot metadata incomplete.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SkillSnapshotReadWarning {
    OwnershipIncomplete {
        message: String,
        issues: Vec<skill_studio_core::skill_ownership::OwnershipReadIssue>,
    },
    DiscoveryIncomplete {
        message: String,
        issues: Vec<skill_studio_core::skill_read::DiscoveryReadIssue>,
    },
}

/// One filesystem path the background watcher should track, and whether
/// `notify` should watch it recursively.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WatchPath {
    pub path: PathBuf,
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchRegistration {
    recursive: bool,
    physical_path: PathBuf,
    #[cfg(unix)]
    identity: (u64, u64),
}

impl WatchRegistration {
    fn read(path: &Path, recursive: bool) -> std::io::Result<Self> {
        let physical_path = path.canonicalize()?;
        let metadata = std::fs::metadata(&physical_path)?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            recursive,
            physical_path,
            #[cfg(unix)]
            identity: (metadata.dev(), metadata.ino()),
        })
    }
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
    refresh_demand: Arc<RefreshDemand>,
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
    /// `<app data dir>/skill-studio/update-check.json`, where
    /// `skill_update_check` persists its result - read on every full rebuild
    /// to fill `has_update`/`update_check`.
    update_check_path: PathBuf,
    /// Keeps the scoped discovery cache for consecutive refreshes of the same roots.
    inventory_service: Arc<Mutex<Option<ScopedSkillService>>>,
}

impl SkillRefreshState {
    /// True when something that can affect the skills list, project list, or
    /// plugin caches changed since the last rebuild and the background loop
    /// hasn't picked it up yet - see `get_installed_skills`, which uses this
    /// to decide whether the published snapshot is safe to read as-is.
    pub(crate) fn is_skills_dirty(&self) -> bool {
        self.refresh_demand.is_pending()
    }

    /// Mark the next rebuild as full, without touching the extra/excluded
    /// project sets - the responsive path a mutation command takes instead of
    /// an inline `rebuild_snapshot_now`. Equivalent to `request_skill_rescan`,
    /// just callable on the state directly rather than through Tauri IPC.
    pub(crate) fn mark_skills_dirty(&self) {
        self.refresh_demand.request();
    }

    /// Add project paths to the always-included set and mark skills dirty,
    /// so both `get_installed_skills` and `register_skill_projects` funnel
    /// through the same bookkeeping.
    pub(crate) fn add_extra_projects(&self, paths: impl IntoIterator<Item = String>) {
        if let Ok(mut extra) = self.extra_projects.lock() {
            let before = extra.len();
            extra.extend(paths);
            if extra.len() == before {
                return;
            }
        }
        self.refresh_demand.request();
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
        self.refresh_demand.request();
    }

    /// Remove project paths from the excluded set, so a caller that
    /// explicitly registers a project (e.g. re-adding it in the sidebar)
    /// overrides a previous "stop tracking".
    pub(crate) fn unexclude_projects(&self, paths: impl IntoIterator<Item = String>) {
        if let Ok(mut excluded) = self.excluded_projects.lock() {
            let before = excluded.len();
            for path in paths {
                excluded.remove(&path);
            }
            if excluded.len() != before {
                self.refresh_demand.request();
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
        refresh_demand: Arc::new(RefreshDemand::default()),
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
        update_check_path: skill_update_check::update_check_path(
            &app.path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from(".")),
        ),
        inventory_service: Arc::new(Mutex::new(None)),
    };

    let app_handle = app.clone();
    let loop_state = state.clone();
    std::thread::spawn(move || run_refresh_loop(app_handle, loop_state));

    state
}

/// Instant read of the current snapshot from managed state.
#[tauri::command]
pub fn get_skill_snapshot(
    state: tauri::State<SkillRefreshState>,
    telemetry_trace: Option<String>,
) -> Option<SkillSnapshot> {
    skill_studio_telemetry::ReadContext::capture_ipc(
        skill_studio_telemetry::ReadOperation::Snapshot,
        telemetry_trace.as_deref(),
    )
    .run(|| state.snapshot.read().ok().and_then(|guard| guard.clone()))
}

/// Ask the background thread to rebuild the snapshot and return its receipt.
#[tauri::command]
pub fn request_skill_rescan(
    state: tauri::State<SkillRefreshState>,
) -> Result<super::skill_refresh_demand::SkillRefreshPosition, String> {
    let generation = state.refresh_demand.request();
    if generation == u64::MAX {
        return Err("Refresh generation exhausted; restart the application".into());
    }
    Ok(state.refresh_demand.position(generation))
}

/// Mark the next rebuild as full, from a caller (`skill_update_check`) that
/// only has an `AppHandle`, not a `tauri::State`. A no-op before
/// `SkillRefreshState` is managed (there's nothing to rebuild yet).
pub fn request_snapshot_rebuild(app: &AppHandle) {
    if let Some(state) = app.try_state::<SkillRefreshState>() {
        state.mark_skills_dirty();
    }
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
pub(crate) fn drop_home_directory_from_batch(paths: Vec<String>, home: &Path) -> Vec<String> {
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
/// Persists the authority before updating memory; a full rebuild follows on
/// the background thread.
#[tauri::command]
pub fn register_skill_projects(
    paths: Vec<String>,
    state: tauri::State<SkillRefreshState>,
) -> Result<Vec<String>, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let valid =
        super::skill_project_authority::track(&home, drop_home_directory_from_batch(paths, &home))?;
    state.unexclude_projects(valid.clone());
    state.add_extra_projects(valid.clone());
    Ok(valid)
}

/// Un-register a caller-registered project path (e.g. one the user closed)
/// so future rebuilds and recovery exclude it. Persists the exclusion before
/// updating memory; a full rebuild follows on the background thread.
#[tauri::command]
pub fn unregister_skill_project(
    path: String,
    state: tauri::State<SkillRefreshState>,
) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    super::skill_project_authority::exclude(&home, &path)?;
    state.remove_extra_project(&path);
    Ok(())
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

    let batch = state.refresh_demand.begin();
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
    let mut inventory_service = state
        .inventory_service
        .lock()
        .map_err(|e| format!("facts cache lock poisoned: {e}"))?;
    let (mut built, report) = build_snapshot(
        &home,
        &extra_projects,
        &excluded_projects,
        &mut invocation_index,
        &mut inventory_service,
        BuildPaths {
            cache_path: &state.cache_path,
            runs_root: &state.runs_root,
            update_check_path: &state.update_check_path,
        },
        now,
    )
    .inspect_err(|_| {
        state.refresh_demand.request();
    })?;
    drop(inventory_service);
    drop(invocation_index);

    if report.incomplete {
        state.invocations_dirty.store(true, Ordering::SeqCst);
    }

    built.full_refresh = Some(batch.position());
    let built = publish_skill_snapshot(app, state, built)?;
    batch.complete();
    state.mark_built_at(now);
    Ok(built)
}

/// Publish one snapshot while the caller holds `rebuild_lock`. This is the
/// only place that assigns revisions or replaces the current projection.
fn publish_skill_snapshot(
    app: &AppHandle,
    state: &SkillRefreshState,
    built: SkillSnapshot,
) -> Result<SkillSnapshot, String> {
    let built = store_skill_snapshot(state, built)?;
    app.emit(SNAPSHOT_EVENT, &built)
        .map_err(|e| format!("failed to emit {SNAPSHOT_EVENT}: {e}"))?;
    Ok(built)
}

fn store_skill_snapshot(
    state: &SkillRefreshState,
    mut built: SkillSnapshot,
) -> Result<SkillSnapshot, String> {
    let mut guard = state
        .snapshot
        .write()
        .map_err(|e| format!("snapshot lock poisoned: {e}"))?;
    built.revision = match guard.as_ref() {
        Some(current) => current
            .revision
            .checked_add(1)
            .ok_or("snapshot revision exhausted")?,
        None => 1,
    };
    *guard = Some(built.clone());
    Ok(built)
}

/// Apply a surgical edit to the in-memory snapshot, emit it, and mark skills
/// dirty so the background loop reconciles with disk within its next poll.
/// Mutation commands whose disk change is small (one frontmatter rewrite, one
/// symlink) use this instead of an inline `rebuild_snapshot_now`, which
/// rescans every skill directory on the command thread and freezes the UI
/// for however long that takes.
pub fn patch_snapshot_and_emit(
    app: &AppHandle,
    state: &SkillRefreshState,
    patch: impl FnOnce(&mut SkillSnapshot),
) -> Result<(), String> {
    let _guard = state
        .rebuild_lock
        .lock()
        .map_err(|e| format!("rebuild lock poisoned: {e}"))?;
    let built = {
        let guard = state
            .snapshot
            .read()
            .map_err(|e| format!("snapshot lock poisoned: {e}"))?;
        let Some(snapshot) = guard.as_ref() else {
            // No snapshot yet - the pending full build will pick up the change.
            state.mark_skills_dirty();
            return Ok(());
        };
        let mut built = snapshot.clone();
        patch(&mut built);
        built
    };
    state.mark_skills_dirty();
    publish_skill_snapshot(app, state, built).map(|_| ())
}

/// Reconcile named skills at all configured global and project roots, replace
/// only those rows in the current projection, emit, then request the ordinary
/// watcher-backed full reconciliation.
pub fn reconcile_skill_names_and_emit(
    app: &AppHandle,
    state: &SkillRefreshState,
    names: impl IntoIterator<Item = String>,
    affected_projects: &[PathBuf],
) -> Result<(), String> {
    let names: BTreeSet<String> = names.into_iter().collect();
    if names.is_empty()
        || names
            .iter()
            .any(|name| Path::new(name).components().count() != 1 || name == "." || name == "..")
    {
        state.mark_skills_dirty();
        return Err("Targeted skill reconciliation needs plain skill names".to_string());
    }

    let _guard = state
        .rebuild_lock
        .lock()
        .map_err(|error| format!("rebuild lock poisoned: {error}"))?;
    let current = state
        .snapshot
        .read()
        .map_err(|error| format!("snapshot lock poisoned: {error}"))?
        .clone();
    let Some(current) = current else {
        state.mark_skills_dirty();
        return Ok(());
    };
    let home = dirs::home_dir().ok_or_else(|| {
        state.mark_skills_dirty();
        "Could not find home directory".to_string()
    })?;
    let excluded_projects = state.excluded_project_set();
    let mut projects: BTreeSet<PathBuf> = current.projects.iter().map(PathBuf::from).collect();
    projects.extend(state.extra_project_paths());
    projects.extend(affected_projects.iter().cloned());
    let projects: Vec<PathBuf> = projects
        .into_iter()
        .filter(|project| !is_home_directory(project, &home))
        .filter(|project| !excluded_projects.contains(&project.to_string_lossy().to_string()))
        .collect();

    let inventory = {
        let mut service = state
            .inventory_service
            .lock()
            .map_err(|error| format!("inventory service lock poisoned: {error}"))?;
        read_snapshot_inventory(&home, &projects, &mut service, Some(&names))
            .inspect_err(|_| state.mark_skills_dirty())?
    };
    if !matches!(&inventory.replacement_safety, ReplacementSafety::Safe { names: selected } if selected == &names)
    {
        state.mark_skills_dirty();
        return Err(
            "Named inventory membership or ownership is incomplete; a full refresh is required"
                .into(),
        );
    }
    let mut targeted_paths: BTreeSet<PathBuf> = agents::skill_roots(&home, &projects)
        .into_iter()
        .flat_map(|root| {
            names.iter().flat_map(move |name| {
                [
                    root.path.join(name),
                    root.path
                        .join(skill_discovery::STUDIO_DISABLED_DIR_NAME)
                        .join(name),
                ]
            })
        })
        .collect();
    targeted_paths.extend(
        inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .map(|deployment| PathBuf::from(&deployment.path)),
    );
    let diagnosis_update = skill_studio_core::skill_diagnosis::diagnose(&inventory);
    let (mut replacements, ledger_only, read_warnings, fork_registry) =
        snapshot_inventory_projection(&home, inventory, Some(&names));
    let current_owner_ids: Vec<String> = current
        .skills
        .iter()
        .flat_map(|skill| skill.deployments.iter())
        .filter(|deployment| {
            !skill_studio_core::skill_reconciliation::deployment_is_selected(
                Path::new(&deployment.path),
                &names,
                &targeted_paths,
            )
        })
        .chain(
            replacements
                .iter()
                .flat_map(|skill| skill.deployments.iter()),
        )
        .filter_map(|deployment| deployment.owner_id.clone())
        .collect();
    let update_store = skill_update_check::read_update_check_store_at(&state.update_check_path);
    apply_skill_snapshot_overlays(
        &home,
        &mut replacements,
        &fork_registry,
        &update_store,
        &current_owner_ids,
    );
    let mut built = current;
    skill_studio_core::skill_reconciliation::replace_named_skills(
        &mut built.skills,
        &names,
        &targeted_paths,
        replacements,
    )
    .map_err(|error| {
        state.mark_skills_dirty();
        error.to_string()
    })?;
    let diagnosis = built.diagnosis.as_ref().ok_or_else(|| {
        state.mark_skills_dirty();
        "Snapshot has no diagnosis baseline; a full refresh is required".to_string()
    })?;
    built.diagnosis = Some(
        skill_studio_core::skill_diagnosis::reconcile_named_diagnosis(
            diagnosis,
            &built.skills,
            &names,
            &diagnosis_update,
        )
        .map_err(|error| {
            state.mark_skills_dirty();
            error.to_string()
        })?,
    );
    built
        .ledger_only
        .retain(|record| !names.contains(&record.name));
    built.ledger_only.extend(ledger_only);
    built
        .ledger_only
        .sort_by(|left, right| left.owner_id.cmp(&right.owner_id));
    for warning in read_warnings {
        if !built.read_warnings.contains(&warning) {
            built.read_warnings.push(warning);
        }
    }
    built.scanned_at = Utc::now().to_rfc3339();
    state.mark_skills_dirty();
    publish_skill_snapshot(app, state, built)?;
    Ok(())
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
        let guard = state
            .snapshot
            .read()
            .map_err(|e| format!("snapshot lock poisoned: {e}"))?;
        let Some(snapshot) = guard.as_ref() else {
            return Ok(()); // no full snapshot yet; the next full rebuild covers this
        };
        let mut built = snapshot.clone();
        built.invocations = invocations;
        built.heatmap = heatmap;
        built.scanned_at = now.to_rfc3339();
        built
    };
    publish_skill_snapshot(app, state, built)?;
    state.mark_built_at(now);
    Ok(())
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
fn rebuild_background_snapshot<T>(
    state: &SkillRefreshState,
    retry_after: &mut Option<Instant>,
    now: Instant,
    rebuild: impl FnOnce() -> Result<T, String>,
) -> Option<Result<T, String>> {
    if retry_after.is_some_and(|deadline| now < deadline) {
        return None;
    }
    state.invocations_dirty.store(false, Ordering::SeqCst);
    let result = rebuild();
    if result.is_err() {
        state.mark_skills_dirty();
        *retry_after = Some(Instant::now() + Duration::from_secs(5));
    } else {
        *retry_after = None;
    }
    Some(result)
}

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
    let mut watched: BTreeMap<PathBuf, WatchRegistration> = BTreeMap::new();

    // Start watching before the initial scan so a change made while the
    // first scan is running is never missed.
    let initial_projects = project_discovery::discover_skill_projects(&home);
    reconcile_watchers(
        debouncer.watcher(),
        &mut watched,
        &desired_watch_paths(&home, &initial_projects),
    );

    let mut retry_after = None;
    if let Some(Err(e)) =
        rebuild_background_snapshot(&state, &mut retry_after, Instant::now(), || {
            rebuild_snapshot_now(&app, &state)
        })
    {
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
                        WatchEventKind::Skills => state.mark_skills_dirty(),
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

        let skills_dirty = state.is_skills_dirty();
        let invocations_dirty = state.invocations_dirty.load(Ordering::SeqCst);
        let backlog_stale = invocations_dirty && last_full_rebuild.elapsed() > FULL_REBUILD_BACKLOG;

        if skills_dirty || backlog_stale {
            match rebuild_background_snapshot(&state, &mut retry_after, Instant::now(), || {
                rebuild_snapshot_now(&app, &state)
            }) {
                Some(Ok(_)) => {
                    last_full_rebuild = Instant::now();
                    last_invocations_rebuild = Instant::now();
                    reconcile_watchers_from_snapshot(&home, &state, &mut debouncer, &mut watched);
                }
                Some(Err(e)) => eprintln!("skill refresh: full rebuild failed: {e}"),
                None => {}
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
    watched: &mut BTreeMap<PathBuf, WatchRegistration>,
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
    reconcile_watchers(
        debouncer.watcher(),
        watched,
        &desired_watch_paths(home, &projects),
    );
}

/// Replace watches when their path or recursion mode changes. Notify
/// errors are logged, never propagated: a watch failure on one path
/// shouldn't stop the others from being (un)watched.
fn reconcile_watchers(
    watcher: &mut dyn Watcher,
    watched: &mut BTreeMap<PathBuf, WatchRegistration>,
    desired: &[WatchPath],
) {
    let desired: BTreeMap<PathBuf, WatchRegistration> = desired
        .iter()
        .filter_map(
            |watch| match WatchRegistration::read(&watch.path, watch.recursive) {
                Ok(registration) => Some((watch.path.clone(), registration)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    eprintln!(
                        "skill refresh: failed to inspect {}: {error}",
                        watch.path.display()
                    );
                    None
                }
            },
        )
        .collect();
    let stale: Vec<PathBuf> = watched
        .iter()
        .filter(|(path, registration)| desired.get(*path) != Some(*registration))
        .map(|(path, _)| path.clone())
        .collect();
    for path in stale {
        match watcher.unwatch(&path) {
            Ok(()) => {
                watched.remove(&path);
            }
            Err(error)
                if matches!(
                    error.kind,
                    notify_debouncer_mini::notify::ErrorKind::WatchNotFound
                ) =>
            {
                watched.remove(&path);
            }
            Err(error) => eprintln!(
                "skill refresh: failed to unwatch {}: {error}",
                path.display()
            ),
        }
    }

    for (path, registration) in desired {
        if watched.get(&path) == Some(&registration) {
            continue;
        }
        let mode = if registration.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        match watcher.watch(&path, mode) {
            Ok(()) => {
                watched.insert(path, registration);
            }
            Err(error) => eprintln!("skill refresh: failed to watch {}: {error}", path.display()),
        }
    }
}

/// The on-disk paths `build_snapshot` reads from, grouped so the function
/// doesn't need one parameter per file - all three come straight from
/// `SkillRefreshState`.
struct BuildPaths<'a> {
    cache_path: &'a Path,
    runs_root: &'a Path,
    update_check_path: &'a Path,
}

/// Build a fresh snapshot from `home` plus `extra_projects`, refreshing the
/// invocation index along the way. Pure aside from the filesystem reads, so
/// it's the unit under test for "a caller-registered project's skills show
/// up in the snapshot" without needing a running Tauri app.
/// Codex's own `agents/openai.yaml` `policy.allow_implicit_invocation` value
/// for the skill deployed at `skill_dir`, read straight off disk. `None`
/// when the file is missing, isn't YAML, or doesn't set that key - this is a
/// note-only field (see `skill_invocation`'s module docs), not something the
/// scanner needs to fail a rebuild over.
fn read_codex_allow_implicit_invocation(skill_dir: &Path) -> Option<bool> {
    let content = std::fs::read_to_string(skill_dir.join("agents").join("openai.yaml")).ok()?;
    let value: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;
    value
        .get("policy")?
        .get("allow_implicit_invocation")?
        .as_bool()
}

fn snapshot_owner_ids(skills: &[InstalledSkill]) -> Vec<String> {
    skills
        .iter()
        .flat_map(|skill| skill.deployments.iter())
        .filter_map(|deployment| deployment.owner_id.clone())
        .collect()
}

/// Recompute registry, update, disable, and invocation fields on freshly
/// assembled skills. Both full and targeted discovery use this same path.
fn apply_skill_snapshot_overlays(
    home: &Path,
    skills: &mut [InstalledSkill],
    fork_registry: &super::skill_fork_registry::ForkRegistry,
    update_store: &skill_update_check::UpdateCheckStore,
    current_owner_ids: &[String],
) {
    for skill in skills.iter_mut() {
        skill.has_update = false;
        skill.update_owner_ids.clear();
        skill.update_owners.clear();
        skill.update_commit = None;
        skill.update_commit_at = None;
        skill.fork = None;
        skill.trial = None;
        skill.trials.clear();
        skill.parked = false;
        skill.parked_at = None;
        for deployment in &mut skill.deployments {
            if deployment.disabled_by != Some(super::skill_dto::DisabledBy::StudioMoved) {
                deployment.disabled = false;
                deployment.disabled_by = None;
            }
            deployment.disabled_readers.clear();
            deployment.codex_implicit_invocation = None;
        }
    }

    // A forked skill is no longer in any ledger, so `classify_source_kind`
    // (which only sees on-disk facts) can't tell it apart from a plain
    // manual directory - the fork registry is the only source of truth for
    // it. Forking only ever applies to the shared `.agents/skills` root, so
    // a same-named project-scoped skill is left alone.
    for skill in skills.iter_mut() {
        let Some(record) = fork_registry.forks.get(&skill.name) else {
            continue;
        };
        let expected_path = if record.skill_dir.as_os_str().is_empty() {
            home.join(".agents/skills").join(&skill.name)
        } else {
            record.skill_dir.clone()
        };
        let Some(deployment) = skill.deployments.iter_mut().find(|deployment| {
            deployment.scope == "global"
                && deployment.destination == super::skill_deployment::SkillDestination::Universal
                && matches!(
                    deployment.backing,
                    super::skill_deployment::BackingRelationship::Canonical
                )
                && Path::new(&deployment.path) == expected_path
                && (record.deployment_id.is_empty() || deployment.id == record.deployment_id)
        }) else {
            continue;
        };
        deployment.owner_kind = super::skill_ownership::LifecycleOwnerKind::Fork;
        deployment.owner_id = Some(format!("owner:v1/global/{}", skill.name));
        deployment.mutability = super::skill_deployment::DeploymentMutability::Mutable;
        skill.source_kind = super::provenance::SourceKind::Fork;
        skill.fork = Some(super::skill_dto::ForkInfo {
            origin_tool: record.origin_tool,
            origin_source: record.origin_source.clone(),
            repo: record.repo.clone(),
            base_commit: record.base_commit.clone(),
            forked_at: record.forked_at.clone(),
        });
    }

    for skill in skills.iter_mut() {
        for source in &skill.update_sources {
            let owner_id = &source.owner_id;
            let Some(state) = skill_update_check::state_for_owner(
                update_store,
                owner_id,
                current_owner_ids,
                source,
            ) else {
                skill.update_owners.push(super::skill_dto::OwnerUpdateInfo {
                    owner_id: owner_id.clone(),
                    latest_commit: None,
                    latest_commit_at: None,
                    error: None,
                    comparison: Default::default(),
                    last_verified_comparison: None,
                    actionable: false,
                });
                continue;
            };
            let actionable = skill_update_check::has_update(state);
            if actionable && !skill.update_owner_ids.iter().any(|id| id == owner_id) {
                skill.update_owner_ids.push(owner_id.clone());
            }
            skill.update_owners.push(super::skill_dto::OwnerUpdateInfo {
                owner_id: owner_id.clone(),
                latest_commit: state.latest_commit.clone(),
                latest_commit_at: state.latest_commit_at.clone(),
                error: state.error.clone(),
                comparison: state.comparison.clone(),
                last_verified_comparison: state.last_verified_comparison.clone(),
                actionable,
            });
        }
        skill.has_update = !skill.update_owner_ids.is_empty();
        let shared_metadata = skill.update_owners.first().filter(|first| {
            skill.update_owners.iter().all(|update| {
                update.latest_commit == first.latest_commit
                    && update.latest_commit_at == first.latest_commit_at
            })
        });
        skill.update_commit = shared_metadata.and_then(|update| update.latest_commit.clone());
        skill.update_commit_at = shared_metadata.and_then(|update| update.latest_commit_at.clone());
    }

    // New trial records identify one exact deployment. Version 1 records use
    // scope/name keys and are accepted only when their stored path and scope
    // resolve to exactly one current deployment.
    for skill in skills.iter_mut() {
        let matches: Vec<_> = fork_registry
            .trials
            .values()
            .filter_map(|trial| {
                if trial.status == super::skill_fork_registry::TrialStatus::RecoveryRequired
                    && super::skill_deployment::parse_deployment_id(&trial.deployment_id)
                        .is_some_and(|parsed| parsed.name == skill.name)
                {
                    return Some((trial, trial.deployment_id.clone()));
                }
                let candidates: Vec<_> = skill
                    .deployments
                    .iter()
                    .filter(|deployment| {
                        if !trial.deployment_id.is_empty() {
                            return deployment.id == trial.deployment_id;
                        }
                        let scope_matches = match trial.scope {
                            TrialScope::Global => deployment.scope == "global",
                            TrialScope::Project => {
                                deployment.scope == "project"
                                    && deployment.project_path.as_deref()
                                        == trial.project_path.as_deref()
                            }
                        };
                        scope_matches && Path::new(&deployment.path) == trial.skill_dir
                    })
                    .collect();
                (candidates.len() == 1).then(|| (trial, candidates[0].id.clone()))
            })
            .collect();
        skill.trials = matches
            .into_iter()
            .map(|(trial, deployment_id)| super::skill_dto::TrialInfo {
                deployment_id,
                expires_at: trial.expires_at.clone(),
                method: trial.method,
                status: trial.status,
                scope: trial.scope,
                project_path: trial.project_path.clone(),
            })
            .collect();
        if skill.trials.len() == 1 {
            skill.trial = skill.trials.first().cloned();
        }
    }

    // Parked skills have no deployment left for `classify_source_kind` to
    // look at, so both the "parked" flag and the badge come straight from
    // the registry's `parked` record instead.
    for skill in skills.iter_mut() {
        if let Some(record) = fork_registry.parked.get(&skill.name).filter(|record| {
            let expected = if record.skill_dir.as_os_str().is_empty() {
                home.join(".agents/skills-parked").join(&skill.name)
            } else {
                record.skill_dir.clone()
            };
            skill.deployments.iter().any(|deployment| {
                deployment.scope == "parked"
                    && Path::new(&deployment.path) == expected
                    && (record.deployment_id.is_empty() || deployment.id == record.deployment_id)
            })
        }) {
            skill.parked = true;
            skill.parked_at = Some(record.parked_at.clone());
            skill.source_kind = record.source_kind;
        }
    }

    // Per-harness disable: Codex and OpenCode read their own config, Claude
    // Code has no native switch so it's tracked in the registry instead -
    // see `skill_harness_disable`.
    let codex_disabled_paths: BTreeSet<PathBuf> =
        super::codex_skill_config::read_disabled_skill_md_paths(home)
            .into_iter()
            .collect();
    let opencode_denied: BTreeSet<String> =
        super::opencode_skill_permission::read_denied_patterns(home)
            .into_iter()
            .collect();
    for skill in skills.iter_mut() {
        let open_code_deployment_count = skill
            .deployments
            .iter()
            .filter(|deployment| deployment.agent == "OpenCode")
            .count();
        let claude_deployment_count = skill
            .deployments
            .iter()
            .filter(|deployment| deployment.agent == "Claude Code")
            .count();
        for deployment in &mut skill.deployments {
            if deployment.agent == "Codex" {
                let skill_md = PathBuf::from(&deployment.path).join("SKILL.md");
                let canonical = std::fs::canonicalize(&skill_md).unwrap_or(skill_md);
                if codex_disabled_paths.contains(&canonical) {
                    deployment.disabled = true;
                    deployment.disabled_by = Some(super::skill_dto::DisabledBy::CodexConfig);
                }
                deployment.codex_implicit_invocation =
                    read_codex_allow_implicit_invocation(&PathBuf::from(&deployment.path));
            } else if deployment.agent == "OpenCode" {
                if open_code_deployment_count == 1
                    && opencode_denied.iter().any(|pattern| {
                        super::opencode_skill_permission::pattern_matches(pattern, &skill.name)
                    })
                {
                    deployment.disabled = true;
                    deployment.disabled_by = Some(super::skill_dto::DisabledBy::OpencodePermission);
                }
            } else if deployment.agent == "Claude Code"
                && fork_registry
                    .harness_disabled
                    .values()
                    .filter_map(|by_harness| by_harness.get("claude-code"))
                    .any(|record| {
                        (record.deployment_id.is_empty() && claude_deployment_count == 1)
                            || record.deployment_id == deployment.id
                    })
            {
                deployment.disabled = true;
                deployment.disabled_by = Some(super::skill_dto::DisabledBy::ClaudeLinkRemoved);
            } else if deployment.agent == "shared" {
                let skill_md = PathBuf::from(&deployment.path).join("SKILL.md");
                let canonical = std::fs::canonicalize(&skill_md).unwrap_or(skill_md);
                if codex_disabled_paths.contains(&canonical) {
                    deployment.disabled_readers.push("codex".to_string());
                }
                if opencode_denied.iter().any(|pattern| {
                    super::opencode_skill_permission::pattern_matches(pattern, &skill.name)
                }) {
                    deployment.disabled_readers.push("open-code".to_string());
                }
            }
        }
    }

    // Invocation policy comes straight from the already-parsed frontmatter
    // fields (`frontmatter_fields` is stringified, since that's shared with
    // the dashboard's "extra fields" display).
    for skill in skills.iter_mut() {
        let disable_model = skill
            .frontmatter_fields
            .get("disable-model-invocation")
            .map(|v| v == "true");
        let user_invocable = skill
            .frontmatter_fields
            .get("user-invocable")
            .map(|v| v == "true");
        skill.invocation =
            super::frontmatter::invocation_policy_from(disable_model, user_invocable).0;
    }
}

fn bind_inventory_service<'a>(
    home: &Path,
    project_paths: &[PathBuf],
    service: &'a mut Option<ScopedSkillService>,
) -> Result<&'a mut ScopedSkillService, String> {
    let scope = super::skill_scope_config::desktop_skill_scope(home, project_paths)?;
    if service
        .as_ref()
        .is_none_or(|current| current.scope() != scope)
    {
        *service = Some(ScopedSkillService::bind(scope).map_err(|error| error.to_string())?);
    }
    Ok(service.as_mut().expect("service is bound above"))
}

fn read_snapshot_inventory(
    home: &Path,
    project_paths: &[PathBuf],
    service: &mut Option<ScopedSkillService>,
    names: Option<&BTreeSet<String>>,
) -> Result<InventoryRead, String> {
    skill_studio_telemetry::ReadContext::capture(
        skill_studio_telemetry::TelemetrySurface::Desktop,
        skill_studio_telemetry::ReadOperation::Scan,
    )
    .run(|| {
        let result = bind_inventory_service(home, project_paths, service)?
            .scan(names, Some(Duration::from_secs(30)));
        if matches!(
            &result,
            Err(ScanError::Coordination(
                skill_studio_core::skill_service::CoordinationFailure::Changed
                    | skill_studio_core::skill_service::CoordinationFailure::Unavailable { .. }
            ))
        ) {
            *service = None;
        }
        result.map_err(|error| error.to_string())
    })
}

fn snapshot_inventory_projection(
    home: &Path,
    mut inventory: InventoryRead,
    names: Option<&BTreeSet<String>>,
) -> (
    Vec<InstalledSkill>,
    Vec<LedgerOnlySkill>,
    Vec<SkillSnapshotReadWarning>,
    skill_studio_core::skill_fork_registry::ForkRegistry,
) {
    let mut lock = inventory.ownership.global_lock();
    let deployed_names = inventory
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<BTreeSet<_>>();
    lock.skills.retain(|name, _| {
        !deployed_names.contains(name.as_str()) && names.is_none_or(|names| names.contains(name))
    });
    let mut compatibility_rows = skill_studio_core::skill_assembly::assemble_installed_skills(
        Vec::new(),
        &lock,
        &inventory.ownership,
        &Default::default(),
    );
    if let skill_studio_core::skill_ownership::OwnershipInput::Loaded(registry) =
        &inventory.ownership.registry
    {
        skill_studio_core::skill_registry_projection::apply_registry_facts(
            home,
            &mut compatibility_rows,
            registry,
        );
    }
    inventory.skills.extend(compatibility_rows);
    inventory
        .skills
        .sort_by(|left, right| left.name.cmp(&right.name));
    let unknown_owner = inventory.skills.iter().any(|skill| {
        skill.deployments.iter().any(|deployment| {
            deployment.owner_kind == super::skill_ownership::LifecycleOwnerKind::Unknown
        })
    });
    inventory.discovery_issues.retain(|issue| {
        issue.kind != skill_studio_core::skill_read::DiscoveryReadIssueKind::GitScopeBoundary
            || unknown_owner
    });
    let mut warnings = Vec::new();
    if !inventory.discovery_issues.is_empty() {
        warnings.push(SkillSnapshotReadWarning::DiscoveryIncomplete {
            message: format!(
                "{} discovery issues. Some skills may be missing or have incomplete details.",
                inventory.discovery_issues.len()
            ),
            issues: inventory.discovery_issues,
        });
    }
    let failures = inventory.ownership.failures();
    if !failures.is_empty() {
        warnings.push(SkillSnapshotReadWarning::OwnershipIncomplete {
            message: format!("{} ownership inputs could not be read. Affected deployments are unavailable for lifecycle changes.", failures.len()),
            issues: failures,
        });
    }
    let registry = match inventory.ownership.registry {
        skill_studio_core::skill_ownership::OwnershipInput::Loaded(registry) => registry,
        _ => Default::default(),
    };
    (inventory.skills, inventory.ledger_only, warnings, registry)
}

fn build_snapshot(
    home: &Path,
    extra_projects: &[PathBuf],
    excluded_projects: &BTreeSet<String>,
    invocation_index: &mut SkillInvocationIndex,
    inventory_service: &mut Option<ScopedSkillService>,
    paths: BuildPaths,
    now: DateTime<Utc>,
) -> Result<(SkillSnapshot, RefreshReport), String> {
    let total_start = Instant::now();
    let BuildPaths {
        cache_path,
        runs_root,
        update_check_path,
    } = paths;
    let projects_start = Instant::now();
    let project_paths: Vec<PathBuf> =
        super::skill_project_authority::scoped_projects(home, extra_projects.iter().cloned())?
            .into_iter()
            .filter(|p| !excluded_projects.contains(&p.to_string_lossy().to_string()))
            .collect();
    let projects_ms = projects_start.elapsed().as_millis();

    let discovery_start = Instant::now();
    let inventory = read_snapshot_inventory(home, &project_paths, inventory_service, None)?;
    let discovery_ms = discovery_start.elapsed().as_millis();
    let (facts_hits, facts_total) = inventory_service
        .as_ref()
        .map_or((0, 0), ScopedSkillService::last_pass_stats);
    let diagnosis = Some(skill_studio_core::skill_diagnosis::diagnose(&inventory));
    let (mut skills, ledger_only, read_warnings, fork_registry) =
        snapshot_inventory_projection(home, inventory, None);

    let update_store = skill_update_check::read_update_check_store_at(update_check_path);
    let update_check = skill_update_check::summarize(&update_store);

    let current_owner_ids = snapshot_owner_ids(&skills);
    apply_skill_snapshot_overlays(
        home,
        &mut skills,
        &fork_registry,
        &update_store,
        &current_owner_ids,
    );

    let invocations_start = Instant::now();
    let report = invocation_index.refresh(&home.join(".claude/projects"));
    if let Err(e) = invocation_index.save(cache_path) {
        eprintln!("skill refresh: failed to save invocation cache: {e}");
    }
    let invocations_ms = invocations_start.elapsed().as_millis();

    let skill_names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
    let last_test_by_skill = skill_run_history::read_last_test_index(runs_root, &skill_names)
        .into_iter()
        .collect();
    let skill_count = skills.len();

    let snapshot = SkillSnapshot {
        revision: 0,
        full_refresh: None,
        ledger_only,
        diagnosis,
        skills,
        read_warnings,
        projects: project_paths
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        invocations: invocation_index.stats_at(now),
        heatmap: invocation_index.heatmap_at(365, now),
        scanned_at: now.to_rfc3339(),
        last_test_by_skill,
        update_check,
        opencode_config_kind: super::opencode_skill_permission::detect_config_kind(home),
    };

    let total_ms = total_start.elapsed().as_millis();
    let rest_ms = total_ms
        .saturating_sub(projects_ms)
        .saturating_sub(discovery_ms)
        .saturating_sub(invocations_ms);
    eprintln!(
        "skill refresh: full rebuild {total_ms} ms (projects {projects_ms} ms, discovery {discovery_ms} ms, invocations {invocations_ms} ms, assembly+rest {rest_ms} ms; {skill_count} skills, facts cache hits {facts_hits}/{facts_total})"
    );

    Ok((snapshot, report))
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
        for sub in [
            ".claude",
            ".codex",
            ".opencode",
            ".pi",
            ".cursor",
            ".grok",
            ".agents",
        ] {
            add(&mut merged, project.join(sub), true);
        }
        add(&mut merged, project.clone(), false);
    }

    if let Ok(scope) = super::skill_scope_config::desktop_skill_scope(home, projects) {
        add_scope_watch_paths(&mut merged, &scope);
    }

    merged
        .into_iter()
        .map(|(path, recursive)| WatchPath { path, recursive })
        .collect()
}

fn add_scope_watch_paths(merged: &mut BTreeMap<PathBuf, bool>, scope: &SkillScope) {
    for root in scope
        .backing_roots
        .iter()
        .chain(&scope.plugin_ownership_roots)
    {
        merged.insert(root.clone(), true);
        if let Some(parent) = root.parent().filter(|parent| parent.parent().is_some()) {
            merged.entry(parent.to_path_buf()).or_insert(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn failed_background_fallback_retries_without_another_watch_event() {
        let state = fixture_state();
        state.mark_skills_dirty();
        state.invocations_dirty.store(true, Ordering::SeqCst);
        let mut retry_after = None;
        let failed = rebuild_background_snapshot(&state, &mut retry_after, Instant::now(), || {
            Err::<(), _>("temporary scan failure".to_string())
        });
        assert!(matches!(failed, Some(Err(_))));
        assert!(state.is_skills_dirty());
        let deadline = retry_after.unwrap();
        assert!(rebuild_background_snapshot(
            &state,
            &mut retry_after,
            deadline - Duration::from_millis(1),
            || -> Result<(), String> { panic!("must not retry before the backoff ends") },
        )
        .is_none());
        assert!(state.is_skills_dirty());
        assert_eq!(
            rebuild_background_snapshot(&state, &mut retry_after, deadline, || {
                state.refresh_demand.begin().complete();
                Ok(())
            }),
            Some(Ok(()))
        );
        assert!(!state.is_skills_dirty());
        assert!(retry_after.is_none());

        // A new request during a successful scan must survive publication.
        rebuild_background_snapshot(&state, &mut retry_after, Instant::now(), || {
            state.mark_skills_dirty();
            Ok(())
        });
        assert!(state.is_skills_dirty());
    }

    #[test]
    fn repeated_project_registration_preserves_refresh_coverage() {
        let state = fixture_state();
        state.add_extra_projects(Vec::<String>::new());
        assert!(
            !state.is_skills_dirty(),
            "empty registration requested a scan"
        );

        state.add_extra_projects(["/fixture/project".to_string()]);
        let batch = state.refresh_demand.begin();
        state.unexclude_projects(["/fixture/project".to_string()]);
        state.add_extra_projects(["/fixture/project".to_string()]);
        batch.complete();
        assert!(
            !state.is_skills_dirty(),
            "duplicate registration invalidated the active scan"
        );

        state
            .excluded_projects
            .lock()
            .unwrap()
            .insert("/fixture/project".into());
        state.unexclude_projects(["/fixture/project".to_string()]);
        assert!(state.is_skills_dirty(), "re-enabled scope needs a scan");
    }

    #[test]
    fn watch_registration_changes_when_a_directory_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let watched = root.path().join("skills");
        fs::create_dir(&watched).unwrap();
        let first = WatchRegistration::read(&watched, true).unwrap();
        fs::rename(&watched, root.path().join("previous-skills")).unwrap();
        fs::create_dir(&watched).unwrap();
        let second = WatchRegistration::read(&watched, true).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn folder_settings_replace_existing_watch_modes() {
        use notify_debouncer_mini::notify::{
            Config, EventHandler, Result as NotifyResult, WatcherKind,
        };
        #[derive(Default)]
        struct RecordingWatcher {
            calls: Vec<Option<RecursiveMode>>,
        }
        impl Watcher for RecordingWatcher {
            fn new<F: EventHandler>(_: F, _: Config) -> NotifyResult<Self> {
                Ok(Self::default())
            }
            fn watch(&mut self, _: &Path, mode: RecursiveMode) -> NotifyResult<()> {
                self.calls.push(Some(mode));
                Ok(())
            }
            fn unwatch(&mut self, _: &Path) -> NotifyResult<()> {
                self.calls.push(None);
                Ok(())
            }
            fn kind() -> WatcherKind {
                WatcherKind::NullWatcher
            }
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_path_buf();
        let mut watched = BTreeMap::new();
        let mut watcher = RecordingWatcher::default();
        for recursive in [false, true, true, false] {
            reconcile_watchers(
                &mut watcher,
                &mut watched,
                &[WatchPath {
                    path: path.clone(),
                    recursive,
                }],
            );
            assert_eq!(
                watched
                    .get(&path)
                    .map(|registration| registration.recursive),
                Some(recursive)
            );
        }
        assert_eq!(
            watcher.calls,
            vec![
                Some(RecursiveMode::NonRecursive),
                None,
                Some(RecursiveMode::Recursive),
                None,
                Some(RecursiveMode::NonRecursive)
            ]
        );
    }

    #[test]
    fn configured_roots_are_watched_recursively_without_downgrading_parent_watches() {
        let scope = super::super::skill_scope_config::configured_scope(
            Path::new("/home/tester"),
            &[],
            Some(
                r#"{"backing_roots":["/data/skills","/external"],"plugin_ownership_roots":["/plugins/cache"]}"#,
            ),
        )
        .unwrap();
        let mut watched = BTreeMap::from([(PathBuf::from("/data"), true)]);
        add_scope_watch_paths(&mut watched, &scope);
        assert_eq!(watched.get(Path::new("/data/skills")), Some(&true));
        assert_eq!(watched.get(Path::new("/plugins/cache")), Some(&true));
        assert_eq!(watched.get(Path::new("/plugins")), Some(&false));
        assert_eq!(watched.get(Path::new("/data")), Some(&true));
        assert_eq!(watched.get(Path::new("/external")), Some(&true));
        assert!(!watched.contains_key(Path::new("/")));
    }

    #[test]
    fn desired_watch_paths_includes_global_roots_and_parents() {
        let home = PathBuf::from("/home/tester");
        let paths = desired_watch_paths(&home, &[]);

        let claude_skills = home.join(".claude/skills");
        assert!(paths.iter().any(|w| w.path == claude_skills && w.recursive));
        assert!(paths
            .iter()
            .any(|w| w.path == home.join(".claude") && !w.recursive));
        assert!(paths
            .iter()
            .any(|w| w.path == home.join(".cursor/skills") && w.recursive));
        assert!(paths
            .iter()
            .any(|w| w.path == home.join(".grok/skills") && w.recursive));
    }

    #[test]
    fn desired_watch_paths_includes_project_entries() {
        let home = PathBuf::from("/home/tester");
        let project = PathBuf::from("/work/my-project");
        let paths = desired_watch_paths(&home, std::slice::from_ref(&project));

        assert!(paths
            .iter()
            .any(|w| w.path == project.join(".claude") && w.recursive));
        assert!(paths
            .iter()
            .any(|w| w.path == project.join(".cursor") && w.recursive));
        assert!(paths
            .iter()
            .any(|w| w.path == project.join(".grok") && w.recursive));
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
            &mut None,
            BuildPaths {
                cache_path: &cache_path,
                runs_root: tmp.path(),
                update_check_path: &tmp.path().join("update-check.json"),
            },
            Utc::now(),
        )
        .unwrap();

        assert!(snapshot
            .projects
            .contains(&project.to_string_lossy().to_string()));
        assert!(snapshot.skills.iter().any(|s| s.name == "foo"));
    }

    /// Regression for the "parked-but-reinstalled" case: `skill_park`'s
    /// module docs note that `dotagents install`/`npx skills add` can
    /// recreate the shared folder while a skill is parked - the snapshot
    /// must still mark the skill `parked` (from the registry record) while
    /// also surfacing the reinstalled deployment, so the frontend's health
    /// check can flag the conflict rather than hiding it.
    #[test]
    fn build_snapshot_marks_parked_skills_and_surfaces_a_reinstalled_deployment() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".agents/skills-parked/find-bugs")).unwrap();
        fs::write(
            home.join(".agents/skills-parked/find-bugs/SKILL.md"),
            "---\nname: find-bugs\ndescription: test\n---\nbody",
        )
        .unwrap();
        fs::create_dir_all(home.join(".agents/skills/find-bugs")).unwrap();
        fs::write(
            home.join(".agents/skills/find-bugs/SKILL.md"),
            "---\nname: find-bugs\ndescription: reinstalled\n---\nbody",
        )
        .unwrap();
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents/skill-studio.json"),
            r#"{"version":1,"forks":{},"trials":{},"parked":{"find-bugs":{"parked_at":"2026-01-01T00:00:00Z","source_kind":"manual"}},"harness_disabled":{}}"#,
        )
        .unwrap();

        let mut invocation_index = SkillInvocationIndex::default();
        let cache_path = tmp.path().join("cache.json");
        let (snapshot, _report) = build_snapshot(
            &home,
            &[],
            &BTreeSet::new(),
            &mut invocation_index,
            &mut None,
            BuildPaths {
                cache_path: &cache_path,
                runs_root: tmp.path(),
                update_check_path: &tmp.path().join("update-check.json"),
            },
            Utc::now(),
        )
        .unwrap();

        let skill = snapshot
            .skills
            .iter()
            .find(|s| s.name == "find-bugs")
            .unwrap();
        assert!(skill.parked);
        assert!(skill.deployments.iter().any(|d| d.scope == "parked"));
        assert!(skill.deployments.iter().any(|d| d.scope == "global"));
    }

    #[test]
    fn build_snapshot_does_not_overlay_fork_onto_same_name_project_deployment() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let project_skill = project.join(".agents/skills/find-bugs");
        fs::create_dir_all(&project_skill).unwrap();
        fs::write(
            project_skill.join("SKILL.md"),
            "---\nname: find-bugs\ndescription: project copy\n---\nbody",
        )
        .unwrap();

        let global_skill = home.join(".agents/skills/find-bugs");
        let global_id = super::super::skill_deployment::deployment_id(
            "find-bugs",
            "global",
            super::super::skill_deployment::SkillDestination::Universal,
            "universal",
            None,
            &global_skill,
        );
        let mut registry = super::super::skill_fork_registry::read_fork_registry(&home).unwrap();
        registry.forks.insert(
            "find-bugs".to_string(),
            super::super::skill_fork_registry::ForkRecord {
                deployment_id: global_id,
                skill_dir: global_skill,
                forked_at: "2026-01-01T00:00:00Z".to_string(),
                origin_tool: super::super::skill_fork_registry::OriginTool::Dotagents,
                origin_source: "getsentry/find-bugs".to_string(),
                repo: "getsentry/find-bugs".to_string(),
                path: "skills/find-bugs".to_string(),
                declared_ref: None,
                base_commit: "a".repeat(40),
            },
        );
        super::super::skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

        let mut invocation_index = SkillInvocationIndex::default();
        let cache_path = tmp.path().join("cache.json");
        let (snapshot, _) = build_snapshot(
            &home,
            std::slice::from_ref(&project),
            &BTreeSet::new(),
            &mut invocation_index,
            &mut None,
            BuildPaths {
                cache_path: &cache_path,
                runs_root: tmp.path(),
                update_check_path: &tmp.path().join("update-check.json"),
            },
            Utc::now(),
        )
        .unwrap();

        let skill = snapshot
            .skills
            .iter()
            .find(|skill| skill.name == "find-bugs")
            .unwrap();
        assert_eq!(
            skill.source_kind,
            super::super::provenance::SourceKind::Unknown
        );
        assert!(!snapshot.read_warnings.is_empty());
        assert!(skill.fork.is_none());
        assert_eq!(skill.deployments.len(), 1);
        assert_eq!(skill.deployments[0].scope, "project");
    }

    #[test]
    fn known_ownership_skips_git_boundary_warning_but_keeps_metadata_failures() {
        for escaped_git_marker in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let skill = home.join(".agents/skills/example");
            fs::create_dir_all(&skill).unwrap();
            fs::write(
                skill.join("SKILL.md"),
                "---\nname: example\ndescription: fixture\n---\nbody",
            )
            .unwrap();
            fs::write(
                home.join(".agents/.skill-lock.json"),
                serde_json::json!({
                    "version": 3, "skills": {"example": {
                        "source": "fixture/example", "sourceType": "github",
                        "sourceUrl": "https://github.com/fixture/example",
                        "skillFolderHash": "fixture", "installedAt": "2026-09-15T00:00:00Z",
                        "updatedAt": "2026-09-15T00:00:00Z"
                    }}
                })
                .to_string(),
            )
            .unwrap();
            if escaped_git_marker {
                let outside = temp.path().join("outside");
                fs::create_dir(&outside).unwrap();
                std::os::unix::fs::symlink(outside, skill.join(".git")).unwrap();
            }
            let inventory = read_snapshot_inventory(&home, &[], &mut None, None).unwrap();
            assert_eq!(
                inventory.skills[0].deployments[0].owner_kind,
                super::super::skill_ownership::LifecycleOwnerKind::SkillsSh
            );
            let (_, _, warnings, _) = snapshot_inventory_projection(&home, inventory, None);
            assert_eq!(!warnings.is_empty(), escaped_git_marker, "{warnings:?}");
        }
    }

    #[test]
    fn build_snapshot_reads_update_store_and_supplied_home_lock_file() {
        // `update_check_path` is already the full file
        // path (`<app data>/skill-studio/update-check.json`), computed the
        // same way `skill_refresh::init` computes it. Reading it through
        // `read_update_check_store` (which joins that suffix again) would
        // look under a nonexistent nested path and never see this seeded
        // state.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("app-data");
        fs::create_dir_all(home.join(".agents/skills/foo")).unwrap();
        fs::write(
            home.join(".agents/skills/foo/SKILL.md"),
            "---\nname: foo\ndescription: test\n---\nbody",
        )
        .unwrap();
        fs::write(
            home.join(".agents/.skill-lock.json"),
            serde_json::json!({
                "version": 3,
                "skills": {
                    "foo": {
                        "source": "someorg/foo",
                        "sourceType": "github",
                        "sourceUrl": "https://github.com/someorg/foo",
                        "skillPath": "skills/foo/SKILL.md",
                        "skillFolderHash": "abc",
                        "installedAt": "2026-01-01T00:00:00Z",
                        "updatedAt": "2026-01-01T00:00:00Z"
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let seeded_owner_id = "owner:v1/global/foo";
        let update_check_path = skill_update_check::update_check_path(&app_data);
        fs::create_dir_all(update_check_path.parent().unwrap()).unwrap();
        let store = serde_json::json!({
            "version": 2,
            "checked_at": Utc::now().to_rfc3339(),
            "gh_status": { "kind": "ok" },
            "owners": {
                (seeded_owner_id): {
                    "repo": "someorg/foo",
                    "path": "skills/foo",
                    "installed_commit": "a".repeat(40),
                    "latest_commit": "b".repeat(40),
                    "latest_commit_at": Utc::now().to_rfc3339(),
                    "checked_at": Utc::now().to_rfc3339(),
                    "error": null,
                    "lock_updated_at": null,
                    "source_ref": null,
                    "baseline_identity": "global-updated-at:2026-01-01T00:00:00Z:folder-hash:abc",
                    "comparison": { "kind": "different" },
                }
            }
        });
        fs::write(&update_check_path, serde_json::to_string(&store).unwrap()).unwrap();

        let mut invocation_index = SkillInvocationIndex::default();
        let cache_path = tmp.path().join("cache.json");
        let (snapshot, _report) = build_snapshot(
            &home,
            &[],
            &BTreeSet::new(),
            &mut invocation_index,
            &mut None,
            BuildPaths {
                cache_path: &cache_path,
                runs_root: tmp.path(),
                update_check_path: &update_check_path,
            },
            Utc::now(),
        )
        .unwrap();

        let foo = snapshot.skills.iter().find(|s| s.name == "foo").unwrap();
        // The lock file belongs to this temporary home, not the process home.
        assert_eq!(foo.source, "someorg/foo");
        assert_eq!(
            foo.source_kind,
            super::super::provenance::SourceKind::SkillsSh
        );
        assert_eq!(
            foo.deployments[0].owner_id.as_deref(),
            Some(seeded_owner_id)
        );
        assert!(foo.has_update);
        assert_eq!(foo.update_owner_ids, vec![seeded_owner_id]);
        assert_eq!(foo.update_commit.as_deref(), Some("b".repeat(40).as_str()));
        assert_eq!(foo.update_owners.len(), 1);
    }

    #[test]
    fn build_snapshot_exposes_simultaneous_global_and_project_trials() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let global_dir = home.join(".agents/skills/foo");
        let project_dir = project.join(".agents/skills/foo");
        for skill_dir in [&global_dir, &project_dir] {
            fs::create_dir_all(skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: foo\ndescription: test\n---\nbody",
            )
            .unwrap();
        }
        let mut registry = super::super::skill_fork_registry::ForkRegistry::default();
        for (key, scope, project_path, skill_dir) in [
            ("global/foo", TrialScope::Global, None, global_dir),
            (
                "project/foo",
                TrialScope::Project,
                Some(project.to_string_lossy().to_string()),
                project_dir,
            ),
        ] {
            registry.trials.insert(
                key.to_string(),
                super::super::skill_fork_registry::TrialRecord {
                    deployment_id: String::new(),
                    started_at: "2026-09-05T00:00:00Z".to_string(),
                    expires_at: "2026-09-06T00:00:00Z".to_string(),
                    status: super::super::skill_fork_registry::TrialStatus::Active,
                    method: super::super::skill_fork_registry::AddMethod::Copy,
                    scope,
                    project_path,
                    skill_dir,
                    deployment_fingerprint: String::new(),
                    claude_link: None,
                    claude_link_target: None,
                },
            );
        }
        super::super::skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

        let mut invocation_index = SkillInvocationIndex::default();
        let (snapshot, _) = build_snapshot(
            &home,
            std::slice::from_ref(&project),
            &BTreeSet::new(),
            &mut invocation_index,
            &mut None,
            BuildPaths {
                cache_path: &tmp.path().join("cache.json"),
                runs_root: tmp.path(),
                update_check_path: &tmp.path().join("update-check.json"),
            },
            Utc::now(),
        )
        .unwrap();

        let foo = snapshot
            .skills
            .iter()
            .find(|skill| skill.name == "foo")
            .unwrap();
        assert_eq!(foo.trials.len(), 2);
        assert!(foo.trial.is_none());
        assert!(foo
            .trials
            .iter()
            .any(|trial| trial.scope == TrialScope::Global));
        assert!(foo
            .trials
            .iter()
            .any(|trial| trial.scope == TrialScope::Project));
        assert!(foo
            .trials
            .iter()
            .all(|trial| !trial.deployment_id.is_empty()));
    }

    #[test]
    fn build_snapshot_surfaces_recovery_when_only_a_claude_replacement_remains() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let replacement = home.join(".claude/skills/foo");
        fs::create_dir_all(&replacement).unwrap();
        fs::write(
            replacement.join("SKILL.md"),
            "---\nname: foo\ndescription: replacement\n---\nbody",
        )
        .unwrap();
        let missing = home.join(".agents/skills/foo");
        let deployment_id = super::super::skill_deployment::deployment_id(
            "foo",
            "global",
            super::super::skill_deployment::SkillDestination::Universal,
            "universal",
            None,
            &missing,
        );
        let mut registry = super::super::skill_fork_registry::ForkRegistry::default();
        registry.trials.insert(
            super::super::skill_fork_registry::deployment_trial_key(&deployment_id),
            super::super::skill_fork_registry::TrialRecord {
                deployment_id,
                started_at: "2026-09-05T00:00:00Z".to_string(),
                expires_at: "2026-09-06T00:00:00Z".to_string(),
                status: super::super::skill_fork_registry::TrialStatus::RecoveryRequired,
                method: super::super::skill_fork_registry::AddMethod::SkillsSh,
                scope: TrialScope::Global,
                project_path: None,
                skill_dir: missing,
                deployment_fingerprint: "old".to_string(),
                claude_link: Some(replacement),
                claude_link_target: Some(PathBuf::from("replacement")),
            },
        );
        super::super::skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

        let mut invocation_index = SkillInvocationIndex::default();
        let (snapshot, _) = build_snapshot(
            &home,
            &[],
            &BTreeSet::new(),
            &mut invocation_index,
            &mut None,
            BuildPaths {
                cache_path: &tmp.path().join("cache.json"),
                runs_root: tmp.path(),
                update_check_path: &tmp.path().join("update-check.json"),
            },
            Utc::now(),
        )
        .unwrap();

        let foo = snapshot
            .skills
            .iter()
            .find(|skill| skill.name == "foo")
            .unwrap();
        assert_eq!(foo.trials.len(), 1);
        assert_eq!(
            foo.trials[0].status,
            super::super::skill_fork_registry::TrialStatus::RecoveryRequired
        );
    }

    #[test]
    fn differing_owner_updates_keep_only_per_owner_commit_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = home.join("project");
        let lock = serde_json::json!({
            "version": 3,
            "skills": { "foo": {
                "source": "someorg/foo", "sourceType": "github",
                "sourceUrl": "https://github.com/someorg/foo",
                "skillPath": "skills/foo/SKILL.md", "skillFolderHash": "abc",
                "installedAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
            }}
        })
        .to_string();
        for root in [home.join(".agents"), project.join(".agents")] {
            let skill_dir = root.join("skills/foo");
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: foo\ndescription: test\n---\nbody",
            )
            .unwrap();
            fs::write(root.join(".skill-lock.json"), &lock).unwrap();
        }
        let update_check_path = tmp.path().join("update-check.json");
        let cache_path = tmp.path().join("cache.json");
        let mut invocation_index = SkillInvocationIndex::default();
        let (initial, _) = build_snapshot(
            &home,
            std::slice::from_ref(&project),
            &BTreeSet::new(),
            &mut invocation_index,
            &mut None,
            BuildPaths {
                cache_path: &cache_path,
                runs_root: tmp.path(),
                update_check_path: &update_check_path,
            },
            Utc::now(),
        )
        .unwrap();
        let foo = initial
            .skills
            .iter()
            .find(|skill| skill.name == "foo")
            .unwrap();
        let owner_ids: Vec<_> = foo
            .deployments
            .iter()
            .filter_map(|deployment| deployment.owner_id.clone())
            .collect();
        assert_eq!(owner_ids.len(), 2);
        let owners = serde_json::Map::from_iter(owner_ids.iter().enumerate().map(
            |(index, owner_id)| {
                (
                    owner_id.clone(),
                    serde_json::json!({
                        "repo": "someorg/foo", "path": "skills/foo",
                        "installed_commit": "a".repeat(40),
                        "latest_commit": if index == 0 { "b".repeat(40) } else { "c".repeat(40) },
                        "latest_commit_at": if index == 0 { "2026-02-01T00:00:00Z" } else { "2026-03-01T00:00:00Z" },
                        "checked_at": Utc::now().to_rfc3339(), "error": null,
                        "lock_updated_at": null, "source_ref": null,
                        "baseline_identity": "global-updated-at:2026-01-01T00:00:00Z:folder-hash:abc",
                        "comparison": { "kind": "different" }
                    }),
                )
            },
        ));
        fs::write(
            &update_check_path,
            serde_json::json!({
                "version": 2, "checked_at": Utc::now().to_rfc3339(),
                "gh_status": { "kind": "ok" }, "owners": owners
            })
            .to_string(),
        )
        .unwrap();

        let (snapshot, _) = build_snapshot(
            &home,
            std::slice::from_ref(&project),
            &BTreeSet::new(),
            &mut invocation_index,
            &mut None,
            BuildPaths {
                cache_path: &cache_path,
                runs_root: tmp.path(),
                update_check_path: &update_check_path,
            },
            Utc::now(),
        )
        .unwrap();
        let foo = snapshot
            .skills
            .iter()
            .find(|skill| skill.name == "foo")
            .unwrap();
        assert_eq!(foo.update_owners.len(), 2);
        assert!(foo.update_commit.is_none());
        assert!(foo.update_commit_at.is_none());
        assert_ne!(
            foo.update_owners[0].latest_commit,
            foo.update_owners[1].latest_commit
        );
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
            &mut None,
            BuildPaths {
                cache_path: &cache_path,
                runs_root: tmp.path(),
                update_check_path: &tmp.path().join("update-check.json"),
            },
            Utc::now(),
        )
        .unwrap();

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
            &mut None,
            BuildPaths {
                cache_path: &cache_path,
                runs_root: tmp.path(),
                update_check_path: &tmp.path().join("update-check.json"),
            },
            Utc::now(),
        )
        .unwrap();

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

        let home_alias = tmp.path().join("home-alias");
        std::os::unix::fs::symlink(&home, &home_alias).unwrap();
        let batch = vec![
            home_alias.to_string_lossy().to_string(),
            home.join(".").to_string_lossy().to_string(),
            format!("{}/", home.display()),
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
            ledger_only: Vec::new(),
            diagnosis: None,
            read_warnings: Vec::new(),
            revision: 0,
            full_refresh: None,
            skills: vec![InstalledSkill {
                update_sources: Vec::new(),
                name: "foo".to_string(),
                source: "manual".to_string(),
                source_type: "manual".to_string(),
                source_url: None,
                skill_path: None,
                installed_at: Utc::now().to_rfc3339(),
                updated_at: None,
                has_update: false,
                update_owner_ids: Vec::new(),
                update_owners: Vec::new(),
                update_commit: None,
                update_commit_at: None,
                source_kind: SourceKind::Manual,
                deployments: vec![Deployment {
                    agent: "Claude Code".to_string(),
                    scope: "project".to_string(),
                    path: dep_dir.to_string_lossy().to_string(),
                    is_symlink: false,
                    plugin: None,
                    ..Default::default()
                }],
                has_spec: false,
                description: None,
                spec_violations: Vec::new(),
                skill_md_tokens: 0,
                description_tokens: 0,
                folder_bytes: 0,
                file_count: 0,
                content_hash: String::new(),
                content_hashes: Vec::new(),
                modified_at: None,
                frontmatter_fields: BTreeMap::new(),
                folder_truncated: false,
                fork: None,
                trial: None,
                trials: Vec::new(),
                parked: false,
                parked_at: None,
                invocation: super::super::frontmatter::InvocationPolicy::Both,
            }],
            projects: Vec::new(),
            invocations: Vec::new(),
            heatmap: InvocationHeatmap::default(),
            scanned_at: Utc::now().to_rfc3339(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
            opencode_config_kind: None,
        }
    }

    #[test]
    fn shared_refresh_service_preserves_project_ledger_records_and_accepts_named_partial_facts() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        fs::create_dir_all(home.join(".agents")).unwrap();
        let skill = project.join(".agents/skills/alpha");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: alpha\ndescription: fixture\n---\nbody\n",
        )
        .unwrap();
        fs::write(project.join("skills-lock.json"), r#"{"version":1,"skills":{"orphan":{"source":"../local","sourceType":"local","computedHash":"hash"}}}"#).unwrap();
        let mut service = None;
        let (snapshot, _) = build_snapshot(
            &home,
            std::slice::from_ref(&project),
            &BTreeSet::new(),
            &mut SkillInvocationIndex::default(),
            &mut service,
            BuildPaths {
                cache_path: &home.join("discovery-cache.json"),
                runs_root: temp.path(),
                update_check_path: &home.join("updates.json"),
            },
            Utc::now(),
        )
        .unwrap();
        assert_eq!(snapshot.skills.len(), 1);
        assert_eq!(snapshot.ledger_only.len(), 1);
        assert_eq!(snapshot.ledger_only[0].name, "orphan");
        assert_eq!(
            snapshot.ledger_only[0].project_path.as_ref(),
            Some(&project)
        );
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            value["ledger_only"][0]["sources"][0]["kind"],
            "project-skills-sh"
        );
        assert_eq!(
            value["ledger_only"][0]["sources"][0]["entry"]["computedHash"],
            "hash"
        );
        assert_eq!(value["diagnosis"]["extent"], "full");
        assert_eq!(value["diagnosis"]["issues"][0]["owner"]["name"], "orphan");
        assert_eq!(
            value["diagnosis"]["issues"][0]["absence"],
            "confirmed-absent"
        );
        let mut legacy = value;
        legacy.as_object_mut().unwrap().remove("ledger_only");
        legacy.as_object_mut().unwrap().remove("diagnosis");
        let legacy: SkillSnapshot = serde_json::from_value(legacy).unwrap();
        assert!(legacy.ledger_only.is_empty());
        assert!(legacy.diagnosis.is_none());
        let names = BTreeSet::from(["alpha".to_string()]);
        let inventory = read_snapshot_inventory(
            &home,
            std::slice::from_ref(&project),
            &mut service,
            Some(&names),
        )
        .unwrap();
        assert_eq!(service.as_ref().unwrap().last_pass_stats(), (1, 1));
        assert!(
            matches!(&inventory.replacement_safety, ReplacementSafety::Safe { names: selected } if selected == &names)
        );
        assert!(!inventory.discovery_issues.is_empty());
        let updated_diagnosis = skill_studio_core::skill_diagnosis::diagnose(&inventory);
        let merged_diagnosis = skill_studio_core::skill_diagnosis::reconcile_named_diagnosis(
            snapshot.diagnosis.as_ref().unwrap(),
            &snapshot.skills,
            &names,
            &updated_diagnosis,
        )
        .unwrap();
        assert!(merged_diagnosis.issues.iter().any(|issue| matches!(issue,
            skill_studio_core::skill_diagnosis::SkillDiagnostic::LedgerOnly { owner, .. } if owner.name == "orphan")));
        let (rows, ledger_only, warnings, _) =
            snapshot_inventory_projection(&home, inventory, Some(&names));
        assert_eq!(rows.len(), 1);
        assert!(ledger_only.is_empty());
        assert!(!warnings.is_empty());
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
            refresh_demand: Arc::new(RefreshDemand::default()),
            invocations_dirty: Arc::new(AtomicBool::new(false)),
            invocation_index: Arc::new(Mutex::new(SkillInvocationIndex::default())),
            last_built_hour: Arc::new(Mutex::new(None)),
            cache_path: PathBuf::from("/dev/null"),
            runs_root: PathBuf::from("/dev/null"),
            update_check_path: PathBuf::from("/dev/null"),
            inventory_service: Arc::new(Mutex::new(None)),
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
    fn stored_snapshots_receive_monotonic_revisions() {
        let state = fixture_state();
        let first = store_skill_snapshot(&state, fixture_snapshot(Path::new("/first"))).unwrap();
        let second = store_skill_snapshot(&state, fixture_snapshot(Path::new("/second"))).unwrap();

        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 2);
    }

    #[test]
    fn rebuild_started_before_patch_cannot_publish_after_patch() {
        let state = fixture_state();
        store_skill_snapshot(&state, fixture_snapshot(Path::new("/initial"))).unwrap();
        let rebuild_state = state.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let rebuild = std::thread::spawn(move || {
            let _guard = rebuild_state.rebuild_lock.lock().unwrap();
            started_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
            store_skill_snapshot(&rebuild_state, fixture_snapshot(Path::new("/rebuild"))).unwrap()
        });
        started_rx.recv().unwrap();

        let patch_state = state.clone();
        let patch = std::thread::spawn(move || {
            let _guard = patch_state.rebuild_lock.lock().unwrap();
            store_skill_snapshot(&patch_state, fixture_snapshot(Path::new("/patch"))).unwrap()
        });
        finish_tx.send(()).unwrap();

        assert_eq!(rebuild.join().unwrap().revision, 2);
        assert_eq!(patch.join().unwrap().revision, 3);
        assert_eq!(state.snapshot.read().unwrap().as_ref().unwrap().revision, 3);
    }

    #[test]
    fn targeted_replacement_adds_updates_removes_and_sorts_without_touching_unrelated_skills() {
        let mut unrelated = fixture_snapshot(Path::new("/unrelated")).skills.remove(0);
        unrelated.name = "middle".to_string();
        unrelated.description = Some("preserve me".to_string());
        let mut update = fixture_snapshot(Path::new("/old")).skills.remove(0);
        update.name = "zulu".to_string();
        let mut remove = fixture_snapshot(Path::new("/remove")).skills.remove(0);
        remove.name = "remove".to_string();
        let mut skills = vec![update, unrelated, remove];

        let mut added = fixture_snapshot(Path::new("/add")).skills.remove(0);
        added.name = "alpha".to_string();
        let mut updated = fixture_snapshot(Path::new("/new")).skills.remove(0);
        updated.name = "zulu".to_string();
        updated.description = Some("updated".to_string());
        updated.deployments.push(Deployment {
            id: "a".to_string(),
            path: "/new/a".to_string(),
            ..Default::default()
        });
        updated.deployments[0].id = "z".to_string();
        let targeted_paths = ["/old", "/remove", "/new", "/new/a", "/add"]
            .into_iter()
            .map(PathBuf::from)
            .collect();

        skill_studio_core::skill_reconciliation::replace_named_skills(
            &mut skills,
            &BTreeSet::from([
                "zulu".to_string(),
                "remove".to_string(),
                "alpha".to_string(),
            ]),
            &targeted_paths,
            vec![updated, added],
        )
        .unwrap();

        assert_eq!(
            skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "middle", "zulu"]
        );
        assert_eq!(skills[1].description.as_deref(), Some("preserve me"));
        assert_eq!(skills[2].description.as_deref(), Some("updated"));
        assert_eq!(
            skills[2]
                .deployments
                .iter()
                .map(|deployment| deployment.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "z"]
        );
    }

    #[test]
    fn targeted_replacement_preserves_unrelated_lock_only_rows() {
        let mut unrelated = fixture_snapshot(Path::new("/unrelated")).skills.remove(0);
        unrelated.name = "unrelated".into();
        unrelated.deployments.clear();
        unrelated.description = Some("keep this ledger entry".into());
        let mut updated = unrelated.clone();
        updated.name = "updated".into();
        let mut removed = unrelated.clone();
        removed.name = "removed".into();
        let mut replacement = updated.clone();
        replacement.description = Some("new ledger entry".into());
        let mut skills = vec![unrelated, updated, removed];
        let names = ["updated".into(), "removed".into()].into_iter().collect();

        skill_studio_core::skill_reconciliation::replace_named_skills(
            &mut skills,
            &names,
            &BTreeSet::new(),
            vec![replacement],
        )
        .unwrap();

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "unrelated");
        assert_eq!(
            skills[0].description.as_deref(),
            Some("keep this ledger entry")
        );
        assert_eq!(skills[1].name, "updated");
        assert_eq!(skills[1].description.as_deref(), Some("new ledger entry"));
    }

    #[test]
    fn targeted_replacement_removes_prior_row_by_lexical_deployment_path() {
        let mut stale = fixture_snapshot(Path::new("/root/lexical-name"))
            .skills
            .remove(0);
        stale.name = "different-name".to_string();
        let unrelated = fixture_snapshot(Path::new("/root/unrelated"))
            .skills
            .remove(0);
        let mut replacement = fixture_snapshot(Path::new("/root/lexical-name"))
            .skills
            .remove(0);
        replacement.name = "lexical-name".to_string();
        replacement.spec_violations = vec![
            "name \"different-name\" does not match its directory name \"lexical-name\""
                .to_string(),
        ];
        let targeted_paths = [PathBuf::from("/root/lexical-name")].into_iter().collect();
        let mut skills = vec![stale, unrelated];

        skill_studio_core::skill_reconciliation::replace_named_skills(
            &mut skills,
            &BTreeSet::new(),
            &targeted_paths,
            vec![replacement],
        )
        .unwrap();

        assert_eq!(skills.len(), 2);
        assert!(skills.iter().any(|skill| skill.name == "foo"));
        let refreshed = skills
            .iter()
            .find(|skill| skill.name == "lexical-name")
            .unwrap();
        assert!(refreshed.spec_violations[0].contains("does not match"));
        assert!(!skills.iter().any(|skill| skill.name == "different-name"));
    }

    #[test]
    fn targeted_overlay_recomputation_clears_stale_native_and_update_state() {
        let temp = tempfile::tempdir().unwrap();
        let mut skill = fixture_snapshot(&temp.path().join("skill"))
            .skills
            .remove(0);
        skill.has_update = true;
        skill.update_owner_ids.push("stale-owner".to_string());
        skill
            .frontmatter_fields
            .insert("disable-model-invocation".to_string(), "true".to_string());
        skill.deployments[0].agent = "Codex".to_string();
        skill.deployments[0].disabled = true;
        skill.deployments[0].disabled_by = Some(super::super::skill_dto::DisabledBy::CodexConfig);
        skill.deployments[0].disabled_readers = vec!["open-code".to_string()];
        skill.deployments[0].codex_implicit_invocation = Some(true);

        apply_skill_snapshot_overlays(
            temp.path(),
            std::slice::from_mut(&mut skill),
            &super::super::skill_fork_registry::ForkRegistry::default(),
            &skill_update_check::UpdateCheckStore::default(),
            &[],
        );

        assert!(!skill.has_update);
        assert!(skill.update_owner_ids.is_empty());
        assert_eq!(
            skill.invocation,
            super::super::frontmatter::InvocationPolicy::UserOnly
        );
        assert!(!skill.deployments[0].disabled);
        assert_eq!(skill.deployments[0].disabled_by, None);
        assert!(skill.deployments[0].disabled_readers.is_empty());
        assert_eq!(skill.deployments[0].codex_implicit_invocation, None);
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
    fn run_isolated_telemetry_test(name: &str) -> bool {
        const ISOLATED: &str = "SKILL_STUDIO_TEST_ISOLATED_TELEMETRY";
        if std::env::var(ISOLATED).ok().as_deref() == Some(name) {
            return false;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env(ISOLATED, name)
            .args(["--exact", name, "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated telemetry fixture failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    #[test]
    fn tauri_refresh_receipts_and_trace_headers_round_trip_through_commands() {
        if run_isolated_telemetry_test("skills::skill_refresh::tests::tauri_refresh_receipts_and_trace_headers_round_trip_through_commands") {
            return;
        }
        use tracing_subscriber::prelude::*;
        let state = fixture_state();
        let app = tauri::test::mock_builder()
            .manage(state.clone())
            .invoke_handler(tauri::generate_handler![
                get_skill_snapshot,
                request_skill_rescan
            ])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let invoke = |command: &str, body: serde_json::Value| {
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            window.as_ref().clone().on_message(
                tauri::webview::InvokeRequest {
                    cmd: command.into(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: if cfg!(any(windows, target_os = "android")) {
                        "http://tauri.localhost"
                    } else {
                        "tauri://localhost"
                    }
                    .parse()
                    .unwrap(),
                    body: body.into(),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.into(),
                },
                Box::new(move |_, _, response, _, _| {
                    sender.send(response).unwrap();
                }),
            );
            match receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("IPC response deadline")
            {
                tauri::ipc::InvokeResponse::Ok(body) => {
                    body.deserialize::<serde_json::Value>().unwrap()
                }
                tauri::ipc::InvokeResponse::Err(error) => panic!("IPC failed: {error:?}"),
            }
        };
        let first = invoke("request_skill_rescan", serde_json::json!({}));
        let second = invoke("request_skill_rescan", serde_json::json!({}));
        assert_eq!(first["instance_id"], second["instance_id"]);
        assert_eq!(first["generation"], "1");
        assert_eq!(second["generation"], "2");
        let batch = state.refresh_demand.begin();
        let third = invoke("request_skill_rescan", serde_json::json!({}));
        assert_eq!(third["generation"], "3");
        let mut full = fixture_snapshot(Path::new("/fixture-only"));
        full.full_refresh = Some(batch.position());
        let stored = store_skill_snapshot(&state, full).unwrap();
        batch.complete();
        assert!(state.is_skills_dirty());
        let mut invocation = stored.clone();
        invocation.scanned_at = "invocation-only".into();
        store_skill_snapshot(&state, invocation).unwrap();
        let envelopes = sentry::test::with_captured_envelopes_options(
            || {
                tracing::subscriber::with_default(
                    tracing_subscriber::registry()
                        .with(skill_studio_telemetry::read_sentry_layer()),
                    || {
                        let cached = invoke(
                            "get_skill_snapshot",
                            serde_json::json!({
                                "telemetryTrace": "0123456789abcdef0123456789abcdef-0123456789abcdef-1",
                            }),
                        );
                        assert_eq!(cached["full_refresh"], second);
                        assert_eq!(cached["revision"], 2);
                        assert_eq!(cached["scanned_at"], "invocation-only");
                        assert_eq!(
                            invoke(
                                "get_skill_snapshot",
                                serde_json::json!({"telemetryTrace": "PRIVATE_SENTINEL"})
                            )["full_refresh"],
                            second
                        );
                        assert_eq!(
                            invoke("get_skill_snapshot", serde_json::json!({}))["full_refresh"],
                            second
                        );
                    },
                );
            },
            sentry::ClientOptions::new()
                .default_integrations(false)
                .traces_sample_rate(1.0),
        );
        let transactions: Vec<_> = envelopes
            .into_iter()
            .flat_map(sentry::Envelope::into_items)
            .filter_map(|item| {
                if let sentry::protocol::EnvelopeItem::Transaction(tx) = item {
                    Some(serde_json::to_value(tx).unwrap())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(transactions.len(), 3);
        let remote = transactions
            .iter()
            .find(|tx| tx["contexts"]["trace"]["trace_id"] == "0123456789abcdef0123456789abcdef")
            .unwrap();
        assert_eq!(
            remote["contexts"]["trace"]["parent_span_id"],
            "0123456789abcdef"
        );
        assert!(remote["spans"].as_array().unwrap().is_empty());
        assert!(!serde_json::to_string(&transactions)
            .unwrap()
            .contains("PRIVATE_SENTINEL"));
        let final_batch = state.refresh_demand.begin();
        let mut next = stored;
        next.full_refresh = Some(final_batch.position());
        store_skill_snapshot(&state, next).unwrap();
        final_batch.complete();
        assert!(!state.is_skills_dirty());
        assert_eq!(
            invoke("get_skill_snapshot", serde_json::json!({}))["full_refresh"],
            third
        );
    }

    #[test]
    fn inventory_reads_emit_desktop_read_contexts_for_full_and_named_scans() {
        if run_isolated_telemetry_test("skills::skill_refresh::tests::inventory_reads_emit_desktop_read_contexts_for_full_and_named_scans") {
            return;
        }
        use tracing_subscriber::prelude::*;
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".git")).unwrap();
        let mut service = None;
        let names = BTreeSet::from(["absent".to_string()]);
        let envelopes = sentry::test::with_captured_envelopes_options(
            || {
                let subscriber = tracing_subscriber::registry()
                    .with(skill_studio_telemetry::read_sentry_layer());
                tracing::subscriber::with_default(subscriber, || {
                    assert!(
                        read_snapshot_inventory(home.path(), &[], &mut service, None)
                            .unwrap()
                            .skills
                            .is_empty()
                    );
                    assert!(
                        read_snapshot_inventory(home.path(), &[], &mut service, Some(&names))
                            .unwrap()
                            .skills
                            .is_empty()
                    );
                });
            },
            sentry::ClientOptions::new()
                .default_integrations(false)
                .traces_sample_rate(1.0),
        );
        let transactions: Vec<_> = envelopes
            .into_iter()
            .flat_map(sentry::Envelope::into_items)
            .filter_map(|item| {
                if let sentry::protocol::EnvelopeItem::Transaction(transaction) = item {
                    Some(serde_json::to_value(transaction).unwrap())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(transactions.len(), 2);
        for transaction in &transactions {
            assert_eq!(transaction["transaction"], "skill.read");
            assert_eq!(
                transaction["contexts"]["trace"]["data"]["adapter"],
                "desktop"
            );
            let scans: Vec<_> = transaction["spans"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|span| span["description"] == "skill.scan")
                .collect();
            assert_eq!(scans.len(), 1);
            assert_eq!(
                scans[0]["parent_span_id"],
                transaction["contexts"]["trace"]["span_id"]
            );
        }
        assert_ne!(
            transactions[0]["contexts"]["trace"]["trace_id"],
            transactions[1]["contexts"]["trace"]["trace_id"]
        );
    }
}
