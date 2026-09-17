// ============================================================================
// Skills Module - event_commands
// Tauri IPC surface for the event store (docs/spec-event-store.md): listing
// History rows, restoring an event, and the Locations card's per-harness
// disable entry point for shared-folder skills (`skill_materialize`).
// `EventStoreState` wraps an `Option` rather than the bare `EventStore`
// because opening the database can fail (e.g. a locked or corrupt file) and
// the app should still start - every command surfaces that as an ordinary
// `Err` instead of panicking at startup.
// ============================================================================

use super::skill_document_operation::{check_document_cancellation, DocumentOperation};
use skill_studio_core::skill_service::{CancellationToken, ScopedSkillService};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::Manager;

use super::agents::AgentId;
use super::event_store::{EventRow, EventStore};
use super::skill_agent_runner::validate_skill_dir_name;
use super::skill_dto::{Deployment, LifecycleTarget, SkillEventDto};
use super::skill_fork::ForkMutationLock;
use super::skill_materialize;
use super::skill_refresh::{self, SkillRefreshState, SkillSnapshot};
use skill_studio_core::skill_history::{
    read_history_page, read_history_summaries, HistoryQuery, HistoryScope, HistorySummary,
};

pub struct EventStoreState(pub Arc<Mutex<Option<EventStore>>>);

static HISTORY_READ_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

fn locked_store(
    state: &EventStoreState,
) -> Result<std::sync::MutexGuard<'_, Option<EventStore>>, String> {
    state
        .0
        .lock()
        .map_err(|e| format!("event store lock poisoned: {e}"))
}

fn legacy_copy_move_guard(store: &EventStore, home: &Path, row: &EventRow) -> Result<(), String> {
    let mut endpoints = Vec::new();
    let mut current = row.clone();
    let mut visited = std::collections::BTreeSet::new();
    loop {
        if visited.len() >= 64 || !visited.insert(current.id.clone()) {
            return Err("Cannot verify legacy restore ancestry".into());
        }
        let Some(value) = &current.inverse else {
            break;
        };
        let inverse: super::event_store::InverseOp = serde_json::from_value(value.clone())
            .map_err(|error| format!("Cannot verify legacy restore: {error}"))?;
        match inverse {
            super::event_store::InverseOp::MoveBack { from, to, .. } => {
                endpoints.extend([from, to]);
                break;
            }
            super::event_store::InverseOp::RestoreBackup { path, .. } => {
                endpoints.push(path);
                if current.kind != "restore" {
                    break;
                }
                let parent = current
                    .payload
                    .get("target_event")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("Cannot verify legacy restore origin")?;
                current = store
                    .get(parent)?
                    .ok_or("Legacy restore origin is missing")?;
            }
            _ => break,
        }
    }
    if endpoints.is_empty() {
        return Ok(());
    }
    let registry = super::skill_fork_registry::read_fork_registry(home)?;
    let aliases = |path: &Path| {
        let mut paths = vec![path.to_path_buf()];
        paths.extend(normalize_link_path(path));
        paths.extend(std::fs::canonicalize(path).ok());
        paths
    };
    for endpoint in &endpoints {
        if !endpoint.is_absolute()
            || endpoint
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err("Cannot verify legacy restore path".into());
        }
        for record in registry.copies.values() {
            if !record.path.is_absolute() {
                return Err("Cannot verify Copy ownership for legacy restore".into());
            }
            if aliases(endpoint).iter().any(|endpoint| {
                aliases(&record.path)
                    .iter()
                    .any(|owned| endpoint.starts_with(owned) || owned.starts_with(endpoint))
            }) {
                return Err("This older visibility event cannot safely restore Copy ownership. Use the Copy's current Enable or Disable control instead.".into());
            }
        }
    }
    Ok(())
}

fn restore_legacy_event(
    store: &EventStore,
    home: &Path,
    row: &EventRow,
    force: bool,
) -> Result<(), String> {
    legacy_copy_move_guard(store, home, row)?;
    store.restore(&row.id, force).map(|_| ())
}

fn history_detail(store: &EventStore, event_id: &str) -> Option<EventRow> {
    read_history_page(
        &store.conn,
        &HistoryQuery {
            event_id: Some(event_id.to_string()),
            limit: 1,
            before_rowid: None,
            skill: None,
            scope: HistoryScope::All,
        },
    )
    .ok()
    .and_then(|mut page| page.events.pop())
}

fn dto_from_summary(store: &EventStore, home: &Path, row: HistorySummary) -> SkillEventDto {
    let needs_detail = matches!(
        row.kind.as_str(),
        "move_copy_deployment"
            | "move_aside_disable"
            | "move_aside_restore"
            | "restore"
            | "explode_shared_dir"
    );
    let detail = if needs_detail {
        history_detail(store, &row.id)
    } else {
        None
    };
    let copy_visibility = if row.kind == "move_copy_deployment" {
        row.copy_visibility_disabled
    } else {
        None
    };
    let restorable = (row.restorable
        && row.has_inverse
        && row.reverted_by.is_none()
        && matches!(row.status.as_str(), "done" | "failed" | "interrupted"))
        || (row.kind == "move_copy_deployment"
            && row.status == "done"
            && row.reverted_by.is_none()
            && detail.as_ref().is_some_and(|event| {
                skill_studio_core::skill_copy_move_intent::CopyMoveIntent::from_event(event).is_ok()
            }))
        || (super::skill_copy_repair::is_copy_event(&row.kind)
            && row.status == "done"
            && row.reverted_by.is_none())
        || cfg!(target_os = "macos")
            && matches!(
                row.kind.as_str(),
                "repair_dotagents_fork" | "restore_fork_document"
            )
            && row.status == "done"
            && row.reverted_by.is_none();
    let restorable = restorable
        && (!needs_detail || detail.is_some())
        && detail
            .as_ref()
            .is_none_or(|event| legacy_copy_move_guard(store, home, event).is_ok());
    let recovery_action = (row.kind == skill_studio_core::skill_copy_trial_expiry::EVENT_KIND
        && row.status == "done"
        && row.reverted_by.is_none())
    .then_some("restore_trial_backup".to_string());
    let backup_path = row
        .backup_dir
        .as_ref()
        .map(|dir| store.app_data.join(dir).to_string_lossy().into_owned());
    let force_restorable = restorable
        && !super::skill_copy_repair::is_copy_event(&row.kind)
        && row.kind != "move_copy_deployment"
        && row.kind != "make_independent_copy"
        && (row.kind != "explode_shared_dir"
            || detail.as_ref().is_some_and(|event| {
                skill_materialize::restore_guard_for_explode(store, event, home).is_ok()
            }));
    SkillEventDto {
        id: row.id,
        ts: row.ts,
        kind: row.kind,
        skill: row.skill,
        harness: row.harness,
        scope: row.scope,
        project_path: row.project_path,
        status: row.status,
        restorable,
        force_restorable,
        history_label: copy_visibility.map(|disabled| {
            if disabled {
                "Disabled Copy deployment"
            } else {
                "Enabled Copy deployment"
            }
            .to_string()
        }),
        reversal_label: (restorable && copy_visibility.is_some()).then(|| {
            if copy_visibility == Some(true) {
                "Enable"
            } else {
                "Disable"
            }
            .to_string()
        }),
        recovery_action,
        reverted_by: row.reverted_by,
        backup_path,
    }
}

/// Lists events newest-first, for the Activity view's History section.
#[tauri::command]
pub async fn list_skill_events(
    limit: Option<usize>,
    skill: Option<String>,
    event_store: tauri::State<'_, EventStoreState>,
) -> Result<Vec<SkillEventDto>, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    load_history_on_worker(
        Arc::clone(&event_store.0),
        home,
        limit.unwrap_or(200),
        skill,
    )
    .await
}

async fn load_history_on_worker(
    state: Arc<Mutex<Option<EventStore>>>,
    home: PathBuf,
    limit: usize,
    skill: Option<String>,
) -> Result<Vec<SkillEventDto>, String> {
    let permit = HISTORY_READ_SLOTS
        .try_acquire()
        .map_err(|_| "history_busy".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        let state = EventStoreState(state);
        let guard = locked_store(&state)?;
        let store = guard.as_ref().ok_or("Event store is unavailable")?;
        let rows = read_history_summaries(&store.conn, limit, skill.as_deref())
            .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| dto_from_summary(store, &home, row))
            .collect())
    })
    .await
    .map_err(|_| "history_worker_failed".to_string())?
}

#[cfg(test)]
fn dto_from_row(store: &EventStore, home: &Path, row: EventRow) -> SkillEventDto {
    let summary = HistorySummary {
        has_inverse: row.inverse.is_some(),
        copy_visibility_disabled: row
            .payload
            .pointer("/transition/after/disabled")
            .and_then(serde_json::Value::as_bool),
        id: row.id,
        ts: row.ts,
        kind: row.kind,
        skill: row.skill,
        harness: row.harness,
        scope: row.scope,
        project_path: row.project_path,
        status: row.status,
        reverted_by: row.reverted_by,
        backup_dir: row.backup_dir,
        restorable: row.restorable,
    };
    dto_from_summary(store, home, summary)
}

/// Checks for unfinished recovery events without loading the Activity history.
#[tauri::command]
pub async fn has_interrupted_skill_events(app: tauri::AppHandle) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let event_store = app.state::<EventStoreState>();
        let guard = locked_store(&event_store)?;
        let store = guard.as_ref().ok_or("Event store is unavailable")?;
        store.has_interrupted_events()
    })
    .await
    .map_err(|error| format!("Interrupted event check failed: {error}"))?
}

/// Undoes one event. Refuses an `explode_shared_dir` restore while any of
/// its skills are individually disabled (`restore_guard_for_explode`), and
/// unregisters the materialized root once such a restore succeeds.
#[tauri::command]
pub async fn restore_skill_event(
    event_id: String,
    force: bool,
    app: tauri::AppHandle,
    operation_id: Option<String>,
) -> Result<(), String> {
    let operation = DocumentOperation::start(&app, operation_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        restore_skill_event_blocking(
            event_id,
            force,
            app.clone(),
            app.state::<ForkMutationLock>(),
            app.state::<EventStoreState>(),
            _operation.cancellation.clone(),
        )
    })
    .await
    .map_err(|error| format!("Restore task failed: {error}"))?
}

#[tauri::command]
pub async fn restore_expired_trial_backup(
    event_id: String,
    app: tauri::AppHandle,
    operation_id: Option<String>,
) -> Result<(), String> {
    let operation = DocumentOperation::start(&app, operation_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        check_document_cancellation(&_operation.cancellation)?;
        let fork_lock = app.state::<ForkMutationLock>();
        let _guard = fork_lock.try_acquire()?;
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let event_store = app.state::<EventStoreState>();
        let guard = locked_store(&event_store)?;
        let store = guard.as_ref().ok_or("Event store is unavailable")?;
        let transaction = super::skill_md_write::begin_skill_md_write_transaction()?;
        let result =
            skill_studio_core::skill_trial_restore_event::restore_expired_copy_trial_backup(
                &home,
                store,
                &event_id,
                super::skill_harness_disable::copy_visibility_limits(),
                Some(std::time::Duration::from_secs(30)),
                _operation.cancellation.clone(),
            );
        drop(transaction);
        drop(guard);
        skill_refresh::request_snapshot_rebuild(&app);
        result.map(|_| ()).map_err(|error| {
            if error.recovery_required {
                format!(
                    "Restore is incomplete and requires recovery. {}",
                    error.message
                )
            } else {
                error.message
            }
        })
    })
    .await
    .map_err(|error| format!("Restore task failed: {error}"))?
}

fn restore_skill_event_blocking(
    event_id: String,
    force: bool,
    app: tauri::AppHandle,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
    cancellation: CancellationToken,
) -> Result<(), String> {
    check_document_cancellation(&cancellation)?;
    let _guard = fork_lock.try_acquire()?;
    let guard = locked_store(&event_store)?;
    let store = guard.as_ref().ok_or("Event store is unavailable")?;

    let target = store
        .get(&event_id)?
        .ok_or_else(|| format!("Event {event_id} not found"))?;
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    skill_materialize::restore_guard_for_explode(store, &target, &home)?;
    if target.kind == "make_independent_copy" {
        if force {
            return Err(
                "An independent copy cannot be force-restored because that could delete local edits"
                    .to_string(),
            );
        }
        super::skill_independent_copy::restore_independent_copy(store, &home, &target)?;
        drop(guard);
        skill_refresh::request_snapshot_rebuild(&app);
        return Ok(());
    }
    if target.kind == "move_copy_deployment" {
        if force {
            return Err(
                "Copy visibility reversal cannot force overwrite changed content or ownership"
                    .into(),
            );
        }
        let projects = super::skill_project_authority::scoped_projects(&home, [])?;
        let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
        let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
        let result = skill_studio_core::skill_copy_visibility::reverse_copy_visibility(
            &mut service,
            store,
            &event_id,
            super::skill_harness_disable::copy_visibility_limits(),
            Some(std::time::Duration::from_secs(30)),
            cancellation,
        )
        .map(|_| ())
        .map_err(super::skill_harness_disable::copy_visibility_error);
        drop(guard);
        skill_refresh::request_snapshot_rebuild(&app);
        return result;
    }
    #[cfg(target_os = "macos")]
    if super::skill_fork_document_history::is_fork_event(&target.kind) {
        let projects = super::skill_project_authority::scoped_projects(&home, [])?;
        let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
        let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
        let transaction = super::skill_md_write::begin_skill_md_write_transaction()?;
        let result = super::skill_fork_document_history::restore(
            &mut service,
            store,
            &target,
            force,
            &super::event_store::allocate_id(),
            cancellation,
        );
        drop(transaction);
        drop(guard);
        skill_refresh::request_snapshot_rebuild(&app);
        return result;
    }
    if super::skill_copy_repair::is_copy_event(&target.kind) {
        let projects = super::skill_project_authority::scoped_projects(&home, [])?;
        let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
        let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
        let transaction = super::skill_md_write::begin_skill_md_write_transaction()?;
        let result = super::skill_copy_repair::restore(
            &mut service,
            store,
            &target,
            force,
            &super::event_store::allocate_id(),
            cancellation,
        );
        drop(transaction);
        drop(guard);
        skill_refresh::request_snapshot_rebuild(&app);
        return result;
    }
    #[cfg(all(target_os = "macos", feature = "worker-repair"))]
    {
        let projects = super::skill_project_authority::scoped_projects(&home, [])?;
        let scope = super::skill_scope_config::desktop_skill_scope(&home, &projects)?;
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let transaction = super::skill_md_write::begin_skill_md_write_transaction()?;
        let restore_id = super::event_store::allocate_id();
        let command = || {
            let mut command = std::process::Command::new(&executable);
            command.arg("__event-worker");
            command
        };
        let restored = super::skill_frontmatter_repair::restore_scoped_desktop_repair(
            scope.clone(),
            &store.app_data,
            &target,
            force,
            &restore_id,
            cancellation.clone(),
            command,
        );
        drop(transaction);
        let restored = super::skill_frontmatter_repair::settle_desktop_document_operation(
            scope,
            store,
            &restore_id,
            restored,
            true,
            command,
        );
        match restored {
            Ok(false) => {}
            result => {
                drop(guard);
                skill_refresh::request_snapshot_rebuild(&app);
                return result.map(|_| ());
            }
        }
    }
    restore_legacy_event(store, &home, &target, force)?;
    if target.kind == "explode_shared_dir" {
        if let Some(root) = target.payload.get("root").and_then(|v| v.as_str()) {
            store.unregister_materialized_root(Path::new(root))?;
        }
    }
    drop(guard);

    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

/// The Locations card's entry point for disabling/enabling one skill under
/// one harness that reads from the shared root. Converts a whole-dir link to
/// per-skill links on first disable (`explode_shared_dir`), then delegates
/// to `unlink_harness`/`relink_harness`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn set_shared_harness_skill_enabled(
    root_path: String,
    target: LifecycleTarget,
    harness: String,
    enabled: bool,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let deployment_id = target
        .deployment_id
        .as_deref()
        .ok_or("Shared harness disable needs one deployment_id")?;
    if target.owner_id.is_some() {
        return Err(
            "Shared harness disable targets one deployment, not an owner group".to_string(),
        );
    }
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let (installed_skill, deployment) =
        super::skill_lifecycle::find_deployment(&snapshot, deployment_id)?;
    super::skill_lifecycle::revalidate_deployment(deployment, deployment_id)?;
    let display = AgentId::all()
        .into_iter()
        .find(|agent| {
            agent.cli_name() == harness || (*agent == AgentId::OpenCode && harness == "open-code")
        })
        .map(|agent| agent.display_name())
        .ok_or_else(|| format!("Unknown harness: {harness}"))?;
    if deployment.agent != display
        || Path::new(&deployment.path).parent() != Some(Path::new(&root_path))
        || !matches!(
            deployment.backing,
            super::skill_deployment::BackingRelationship::LinkedTo { .. }
        )
    {
        return Err(format!(
            "Deployment {deployment_id} is not the selected {harness} deployment under {root_path}"
        ));
    }
    let skill = installed_skill.name.clone();
    validate_skill_dir_name(&skill)?;
    let root = PathBuf::from(&root_path);

    let guard = locked_store(&event_store)?;
    let store = guard.as_ref().ok_or("Event store is unavailable")?;

    if enabled {
        skill_materialize::relink_harness(store, &root, &skill, &harness)?;
    } else {
        // No longer converts a whole-dir link implicitly (that used to run
        // silently on first disable) - the frontend routes that case through
        // the explicit `materialize_harness_root` dialog first (Locations
        // card / Home repair card) and only calls this once the root is
        // already per-skill links.
        let is_whole_dir_link = std::fs::symlink_metadata(&root)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_whole_dir_link {
            return Err(format!(
                "{} is a link to the Universal folder; convert it to per-skill links first",
                root.display()
            ));
        }
        skill_materialize::unlink_harness(store, &root, &skill, &harness)?;
    }
    drop(guard);

    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

/// `materialize_harness_root`'s guard against a renderer-supplied
/// `(harness, root)` pair that doesn't match a real whole-directory link:
/// the snapshot must record a global deployment for `harness` at exactly
/// `root` with `shared_via_whole_dir_link` set.
fn validate_materialize_request(
    snapshot: &SkillSnapshot,
    target: &LifecycleTarget,
    harness: &str,
    root: &str,
) -> Result<PathBuf, String> {
    let deployment_id = target
        .deployment_id
        .as_deref()
        .ok_or("Materialization needs one deployment_id")?;
    if target.owner_id.is_some() {
        return Err("Materialization targets one deployment, not an owner group".to_string());
    }
    let (_, deployment) = super::skill_lifecycle::find_deployment(snapshot, deployment_id)?;
    super::skill_lifecycle::revalidate_deployment(deployment, deployment_id)?;
    let display = AgentId::all()
        .into_iter()
        .find(|agent| {
            agent.cli_name() == harness || (*agent == AgentId::OpenCode && harness == "open-code")
        })
        .map(|agent| agent.display_name().to_string())
        .unwrap_or_else(|| harness.to_string());
    if deployment.agent != display || !deployment.shared_via_whole_dir_link {
        return Err(format!(
            "Deployment {deployment_id} is not a recorded whole-directory link for {harness}"
        ));
    }
    let deployment_root = Path::new(&deployment.path)
        .parent()
        .ok_or_else(|| format!("{} has no skills root", deployment.path))?;
    if deployment_root != Path::new(root) {
        return Err(format!(
            "{root} is not the harness root of deployment {deployment_id}"
        ));
    }
    let universal_id = match &deployment.backing {
        super::skill_deployment::BackingRelationship::LinkedTo { deployment_id } => deployment_id,
        _ => return Err("Materialization requires a deployment linked to Universal".to_string()),
    };
    let (_, universal) = super::skill_lifecycle::find_deployment(snapshot, universal_id)?;
    if universal.scope != deployment.scope
        || universal.project_path != deployment.project_path
        || !matches!(
            universal.backing,
            super::skill_deployment::BackingRelationship::Canonical
        )
    {
        return Err(
            "The harness deployment does not match its exact scoped Universal deployment"
                .to_string(),
        );
    }
    Path::new(&universal.path)
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{} has no Universal root", universal.path))
}

/// Converts a harness's whole-dir link to the shared skills root into a real
/// directory of per-skill links, as an explicit, named action - the
/// Locations card's Convert dialog and Home's linked-root repair card, both
/// of which must ask before doing this (see the module doc). Refuses when
/// `root` isn't a symlink whose canonical target ends in `.agents/skills`, or
/// when the snapshot has no matching whole-dir-link deployment for `harness`.
#[tauri::command]
pub fn materialize_harness_root(
    app: tauri::AppHandle,
    target: LifecycleTarget,
    harness: String,
    root: String,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let root_path = PathBuf::from(&root);
    skill_materialize::validate_materialize_root(&root_path)?;

    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let universal_root = validate_materialize_request(&snapshot, &target, &harness, &root)?;
    let resolved_harness_root = std::fs::canonicalize(&root_path)
        .map_err(|error| format!("Failed to resolve {root}: {error}"))?;
    let resolved_universal_root = std::fs::canonicalize(&universal_root).map_err(|error| {
        format!(
            "Failed to resolve selected Universal root {}: {error}",
            universal_root.display()
        )
    })?;
    if resolved_harness_root != resolved_universal_root {
        return Err(format!(
            "{root} does not point to the selected deployment's exact scoped Universal root {}",
            universal_root.display()
        ));
    }

    let guard = locked_store(&event_store)?;
    let store = guard.as_ref().ok_or("Event store is unavailable")?;
    skill_materialize::explode_shared_dir(store, &root_path, &harness)?;
    drop(guard);

    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

/// Converts a whole harness root and turns off the exact selected deployment as one durable operation.
#[tauri::command]
pub fn materialize_harness_root_then_disable(
    app: tauri::AppHandle,
    target: LifecycleTarget,
    harness: String,
    root: String,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let deployment_id = target
        .deployment_id
        .as_deref()
        .ok_or("Convert and turn off needs one deployment_id")?;
    if target.owner_id.is_some() {
        return Err("Convert and turn off targets one deployment, not an owner group".to_string());
    }
    let root_path = PathBuf::from(&root);
    skill_materialize::validate_materialize_root(&root_path)?;
    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let universal_root = validate_materialize_request(&snapshot, &target, &harness, &root)?;
    let (installed_skill, deployment) =
        super::skill_lifecycle::find_deployment(&snapshot, deployment_id)?;
    let parsed = super::skill_deployment::parse_deployment_id(deployment_id)
        .ok_or_else(|| format!("Not a deployment id: {deployment_id}"))?;
    let deployment_path = PathBuf::from(&deployment.path);
    if parsed.name != installed_skill.name
        || parsed.scope != deployment.scope
        || parsed.project_path != deployment.project_path
        || parsed.lexical_path != deployment_path
        || deployment_path.parent() != Some(root_path.as_path())
    {
        return Err(
            "The selected deployment identity no longer matches its exact path".to_string(),
        );
    }
    validate_skill_dir_name(&installed_skill.name)?;
    let resolved_harness_root = std::fs::canonicalize(&root_path)
        .map_err(|error| format!("Failed to resolve {root}: {error}"))?;
    let resolved_universal_root = std::fs::canonicalize(&universal_root).map_err(|error| {
        format!(
            "Failed to resolve selected Universal root {}: {error}",
            universal_root.display()
        )
    })?;
    if resolved_harness_root != resolved_universal_root {
        return Err(format!(
            "{root} does not point to the selected deployment's exact scoped Universal root {}",
            universal_root.display()
        ));
    }

    let guard = locked_store(&event_store)?;
    let store = guard.as_ref().ok_or("Event store is unavailable")?;
    skill_materialize::convert_root_then_disable(
        store,
        skill_materialize::ConvertThenDisableRequest {
            root: &root_path,
            shared_root: &universal_root,
            skill: &installed_skill.name,
            harness: &harness,
            deployment_id,
            deployment_path: &deployment_path,
            scope: &deployment.scope,
            project_path: deployment.project_path.as_deref(),
        },
    )?;
    drop(guard);
    skill_refresh::request_snapshot_rebuild(&app);
    Ok(())
}

/// Replaces one healthy Universal-backed deployment link with a local Copy
/// deployment at that exact path. A whole-root link is first converted to
/// per-skill links and restored if the selected copy cannot be completed.
#[tauri::command]
pub fn make_skill_independent_copy(
    target: LifecycleTarget,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let deployment_id = target
        .deployment_id
        .as_deref()
        .ok_or("Make independent copy needs one deployment_id")?;
    if target.owner_id.is_some() {
        return Err("Make independent copy targets one deployment, not an owner group".to_string());
    }

    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;
    let (installed_skill, deployment) =
        super::skill_lifecycle::find_deployment(&snapshot, deployment_id)?;
    super::skill_lifecycle::revalidate_deployment(deployment, deployment_id)?;
    if deployment.disabled || deployment.symlink_is_broken || deployment.symlink_error.is_some() {
        return Err("Make independent copy requires a healthy enabled link".to_string());
    }
    let universal_id = match &deployment.backing {
        super::skill_deployment::BackingRelationship::LinkedTo { deployment_id } => deployment_id,
        _ => return Err("Make independent copy requires a Universal-backed link".to_string()),
    };
    if !deployment.is_symlink && !deployment.shared_via_whole_dir_link {
        return Err("Make independent copy requires a symlink-backed deployment".to_string());
    }
    let (_, universal) = super::skill_lifecycle::find_deployment(&snapshot, universal_id)?;
    if universal.scope != deployment.scope
        || universal.project_path != deployment.project_path
        || !matches!(
            universal.backing,
            super::skill_deployment::BackingRelationship::Canonical
        )
    {
        return Err("The link does not match its exact scoped Universal deployment".to_string());
    }
    let parsed = super::skill_deployment::parse_deployment_id(deployment_id)
        .ok_or_else(|| format!("Not a deployment id: {deployment_id}"))?;
    if parsed.name != installed_skill.name
        || parsed.project_path != deployment.project_path
        || parsed.lexical_path != Path::new(&deployment.path)
    {
        return Err("The selected deployment identity no longer matches its path".to_string());
    }
    let scope = match deployment.scope.as_str() {
        "global" => super::skill_dto::InstallScope::Global,
        "project" => super::skill_dto::InstallScope::Project,
        _ => {
            return Err("Make independent copy supports global or project deployments".to_string())
        }
    };
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let link = PathBuf::from(&deployment.path);
    let expected_source = PathBuf::from(&universal.path);
    let harness = deployment.agent.clone();
    let skill = installed_skill.name.clone();
    let project_path = deployment.project_path.clone();
    let whole_root = deployment.shared_via_whole_dir_link;
    if link.parent().is_none() {
        return Err(format!("{} has no skills root", link.display()));
    }

    let guard = locked_store(&event_store)?;
    let store = guard.as_ref().ok_or("Event store is unavailable")?;
    super::skill_independent_copy::make_skill_independent_copy(
        store,
        super::skill_independent_copy::IndependentCopyRequest {
            home: &home,
            skill: &skill,
            link: &link,
            expected_source: &expected_source,
            harness: &harness,
            scope,
            project_path: project_path.as_deref(),
            slot: &parsed.slot,
            convert_whole_root: whole_root,
        },
    )?;
    drop(guard);
    let affected_projects: Vec<PathBuf> = project_path.iter().map(PathBuf::from).collect();
    if let Err(error) = skill_refresh::reconcile_skill_names_and_emit(
        &app,
        &refresh_state,
        [skill],
        &affected_projects,
    ) {
        eprintln!("[make_skill_independent_copy] targeted snapshot reconciliation failed: {error}");
        refresh_state.mark_skills_dirty();
    }
    Ok(())
}

/// Normalizes a deployment path for comparison against the snapshot without
/// requiring it to resolve: `fs::canonicalize` fails on a broken symlink's
/// final component, so this canonicalizes the *parent* directory instead and
/// rejoins the file name. Works whether or not `path` itself resolves.
fn normalize_link_path(path: &Path) -> Option<PathBuf> {
    let file_name = path.file_name()?;
    let parent = path.parent()?;
    let canonical_parent = std::fs::canonicalize(parent).ok()?;
    Some(canonical_parent.join(file_name))
}

/// Finds the `(skill name, deployment)` in `snapshot` whose path is `path`,
/// matched via `normalize_link_path` so a broken symlink still resolves to
/// its snapshot entry. Used to keep `repair_skill_link` from becoming an
/// arbitrary rm/ln - it can only touch a path the snapshot already knows as
/// a deployment.
fn find_deployment_at<'a>(
    snapshot: &'a SkillSnapshot,
    path: &Path,
) -> Option<(&'a str, &'a Deployment)> {
    let normalized = normalize_link_path(path)?;
    snapshot.skills.iter().find_map(|skill| {
        skill
            .deployments
            .iter()
            .find(|d| {
                normalize_link_path(Path::new(&d.path)).as_deref() == Some(normalized.as_path())
            })
            .map(|d| (skill.name.as_str(), d))
    })
}

fn is_unresolved(deployment: &Deployment) -> bool {
    deployment.symlink_is_broken || deployment.symlink_error.is_some()
}

/// SkillPage's "Repair this location" entry point for a broken deployment
/// symlink that `unlink_harness`/`relink_harness` don't cover (those only
/// handle the shared-root materialize pattern). Validates `path` against the
/// current snapshot as an unresolved deployment, and - for `"relink"` -
/// `target` as a healthy deployment of the *same* skill, so this can't be
/// used to rm/ln an arbitrary path.
#[tauri::command]
pub fn repair_skill_link(
    path: String,
    action: String,
    target: Option<String>,
    app: tauri::AppHandle,
    refresh_state: tauri::State<SkillRefreshState>,
    fork_lock: tauri::State<ForkMutationLock>,
    event_store: tauri::State<EventStoreState>,
) -> Result<(), String> {
    let _guard = fork_lock.try_acquire()?;
    let link = PathBuf::from(&path);

    let snapshot = super::skill_lifecycle::rebuild_fresh_lifecycle_snapshot(&app, &refresh_state)?;

    let (skill_name, deployment) = find_deployment_at(&snapshot, &link)
        .ok_or_else(|| format!("Path is not an installed skill: {path}"))?;
    if !is_unresolved(deployment) {
        return Err(format!("{path} is not a broken link"));
    }
    let skill_name = skill_name.to_string();
    let harness = deployment.agent.clone();

    let guard = locked_store(&event_store)?;
    let store = guard.as_ref().ok_or("Event store is unavailable")?;

    let mut relinked_target: Option<String> = None;
    match action.as_str() {
        "remove" => skill_materialize::repair_remove_link(store, &link, &skill_name, &harness)?,
        "relink" => {
            let target = target.ok_or("relink requires a target")?;
            let target_path = PathBuf::from(&target);
            let (target_skill, target_deployment) = find_deployment_at(&snapshot, &target_path)
                .ok_or_else(|| format!("Target is not an installed skill: {target}"))?;
            if target_skill != skill_name {
                return Err("Target must be a deployment of the same skill".to_string());
            }
            if is_unresolved(target_deployment) {
                return Err("Target location is not healthy".to_string());
            }
            let resolved_target = std::fs::canonicalize(&target_path)
                .map_err(|e| format!("Failed to resolve {target}: {e}"))?;
            skill_materialize::repair_relink_link(
                store,
                &link,
                &resolved_target,
                &skill_name,
                &harness,
            )?;
            relinked_target = Some(resolved_target.to_string_lossy().into_owned());
        }
        other => return Err(format!("Unknown repair action: {other}")),
    }
    drop(guard);

    let normalized_link = normalize_link_path(&link);
    match action.as_str() {
        "remove" => {
            if let Err(e) =
                skill_refresh::patch_snapshot_and_emit(&app, &refresh_state, |snapshot| {
                    let Some(skill) = snapshot.skills.iter_mut().find(|s| s.name == skill_name)
                    else {
                        return;
                    };
                    skill.deployments.retain(|d| {
                        normalize_link_path(Path::new(&d.path)).as_deref()
                            != normalized_link.as_deref()
                    });
                    if skill.deployments.is_empty() {
                        snapshot.skills.retain(|s| s.name != skill_name);
                    }
                })
            {
                eprintln!("[repair_skill_link] snapshot patch failed: {e}");
            }
        }
        "relink" => {
            let new_target = relinked_target;
            if let Err(e) =
                skill_refresh::patch_snapshot_and_emit(&app, &refresh_state, |snapshot| {
                    let Some(skill) = snapshot.skills.iter_mut().find(|s| s.name == skill_name)
                    else {
                        return;
                    };
                    let Some(deployment) = skill.deployments.iter_mut().find(|d| {
                        normalize_link_path(Path::new(&d.path)).as_deref()
                            == normalized_link.as_deref()
                    }) else {
                        return;
                    };
                    deployment.symlink_target = new_target;
                    deployment.symlink_is_broken = false;
                    deployment.symlink_error = None;
                })
            {
                eprintln!("[repair_skill_link] snapshot patch failed: {e}");
            }
        }
        _ => unreachable!("validated above"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn copy_repair_history_routes_only_completed_unclaimed_rows_without_force() {
        use super::super::event_store::{EventDraft, EventStatus};
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(temp.path()).unwrap();
        for kind in [
            "edit_copy_document",
            "undo_copy_document",
            "redo_copy_document",
            "repair_copy_frontmatter",
            "undo_copy_frontmatter",
            "redo_copy_frontmatter",
        ] {
            store
                .record(
                    kind,
                    EventDraft {
                        kind: kind.into(),
                        skill: "sample".into(),
                        harness: None,
                        scope: Some("global".into()),
                        project_path: None,
                        payload: serde_json::Value::Null,
                        inverse: None,
                        backup_dir: None,
                        restorable: false,
                    },
                )
                .unwrap();
            let pending = store.get(kind).unwrap().unwrap();
            assert!(!dto_from_row(&store, temp.path(), pending).restorable);
            store.finish(kind, EventStatus::Done).unwrap();
            let row = store.get(kind).unwrap().unwrap();
            assert!(!row.restorable);
            assert!(row.inverse.is_none());
            let dto = dto_from_row(&store, temp.path(), row.clone());
            assert!(dto.restorable);
            assert!(!dto.force_restorable);
            let mut claimed = row;
            claimed.reverted_by = Some("next".into());
            assert!(!dto_from_row(&store, temp.path(), claimed).restorable);
        }
    }

    use super::super::event_store::{allocate_id, EventDraft, EventStatus, InverseOp};
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    fn history_refuses_reversal_when_required_legacy_detail_exceeds_its_bound() {
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        store.conn.execute(
            "INSERT INTO events(id,ts,kind,skill,payload,inverse,status,restorable) VALUES('oversized','now','restore','sample',?1,'{}','done',1)",
            [serde_json::json!({"body": "x".repeat(skill_studio_core::skill_history::MAX_HISTORY_RECORD_BYTES)}).to_string()],
        ).unwrap();
        let row = read_history_summaries(&store.conn, 1, None)
            .unwrap()
            .pop()
            .unwrap();
        let dto = dto_from_summary(&store, temp.path(), row);
        assert!(!dto.restorable);
        assert!(!dto.force_restorable);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_fork_history_requires_completed_unclaimed_events() {
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        for kind in ["repair_dotagents_fork", "restore_fork_document"] {
            let id = allocate_id();
            store
                .record(
                    &id,
                    EventDraft {
                        kind: kind.into(),
                        skill: "sample".into(),
                        harness: None,
                        scope: Some("global".into()),
                        project_path: None,
                        payload: serde_json::json!({}),
                        inverse: None,
                        backup_dir: None,
                        restorable: false,
                    },
                )
                .unwrap();
            for (status, claim, eligible) in [
                ("pending", None, false),
                ("interrupted", None, false),
                ("failed", None, false),
                ("done", None, true),
                ("done", Some("later"), false),
            ] {
                store
                    .conn
                    .execute(
                        "UPDATE events SET status = ?1, reverted_by = ?2 WHERE id = ?3",
                        rusqlite::params![status, claim, id],
                    )
                    .unwrap();
                let summary = read_history_summaries(&store.conn, 1, None)
                    .unwrap()
                    .pop()
                    .unwrap();
                let dto = dto_from_summary(&store, temp.path(), summary);
                assert_eq!(dto.restorable, eligible, "{kind}/{status}/{claim:?}");
                assert_eq!(dto.force_restorable, eligible);
            }
        }
    }

    #[test]
    fn history_workers_refuse_excess_reads_until_database_work_finishes() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        use std::time::Duration;

        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        let state = Arc::new(Mutex::new(Some(store)));
        let guard = state.lock().unwrap();
        let mut context = Context::from_waker(Waker::noop());
        let mut first = Box::pin(load_history_on_worker(
            Arc::clone(&state),
            temp.path().to_owned(),
            200,
            None,
        ));
        let mut second = Box::pin(load_history_on_worker(
            Arc::clone(&state),
            temp.path().to_owned(),
            200,
            None,
        ));
        assert!(first.as_mut().poll(&mut context).is_pending());
        assert!(second.as_mut().poll(&mut context).is_pending());
        let mut excess = Box::pin(load_history_on_worker(
            Arc::clone(&state),
            temp.path().to_owned(),
            200,
            None,
        ));
        assert!(matches!(
            excess.as_mut().poll(&mut context),
            Poll::Ready(Err(code)) if code == "history_busy"
        ));
        drop(first);
        drop(second);
        drop(guard);

        tauri::async_runtime::block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while HISTORY_READ_SLOTS.available_permits() != 2 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(
                load_history_on_worker(state, temp.path().to_owned(), 200, None)
                    .await
                    .unwrap()
                    .is_empty()
            );
        });
    }

    #[test]
    fn legacy_copy_move_history_refuses_both_endpoints_and_force_without_effects() {
        for kind in ["move_aside_disable", "move_aside_restore", "restore"] {
            for owns_source in [true, false] {
                let temp = tempfile::tempdir().unwrap();
                let home = temp.path();
                fs::create_dir_all(home.join(".agents")).unwrap();
                let store = EventStore::open(&home.join("app-data")).unwrap();
                let from = home.join(".cursor/skills/.skill-studio-disabled/sample");
                let to = home.join(".cursor/skills/sample");
                fs::create_dir_all(&from).unwrap();
                fs::write(from.join("SKILL.md"), "preserved").unwrap();
                let owned = if owns_source { &from } else { &to };
                let registry = serde_json::json!({"version":4,"copies":{"legacy-copy":{
                    "deployment_id":"legacy-copy","name":"sample","path":owned,
                    "scope":"global","destination":"per-harness","slot":"cursor",
                    "disabled":owns_source,"content_hash":"old-hash"
                }}});
                let registry_path = home.join(".agents/skill-studio.json");
                let registry_bytes = serde_json::to_vec(&registry).unwrap();
                fs::write(&registry_path, &registry_bytes).unwrap();
                let id = allocate_id();
                store
                    .record(
                        &id,
                        EventDraft {
                            kind: kind.into(),
                            skill: "sample".into(),
                            harness: None,
                            scope: None,
                            project_path: None,
                            payload: serde_json::json!({"from":to,"to":from}),
                            inverse: Some(
                                serde_json::to_value(InverseOp::MoveBack {
                                    from: from.clone(),
                                    to: to.clone(),
                                    pre_fingerprint: "before".into(),
                                    post_fingerprint: Some("absent".into()),
                                })
                                .unwrap(),
                            ),
                            backup_dir: None,
                            restorable: true,
                        },
                    )
                    .unwrap();
                store.finish(&id, EventStatus::Done).unwrap();
                let row = store.get(&id).unwrap().unwrap();
                let dto = dto_from_row(&store, home, row.clone());
                assert!(!dto.restorable && !dto.force_restorable);
                for force in [false, true] {
                    assert!(restore_legacy_event(&store, home, &row, force)
                        .unwrap_err()
                        .contains("current Enable or Disable"));
                }
                assert_eq!(
                    fs::read_to_string(from.join("SKILL.md")).unwrap(),
                    "preserved"
                );
                assert!(!to.exists());
                assert_eq!(fs::read(&registry_path).unwrap(), registry_bytes);
                assert!(store.get(&id).unwrap().unwrap().reverted_by.is_none());
                fs::write(&registry_path, "invalid registry").unwrap();
                assert!(restore_legacy_event(&store, home, &row, true).is_err());
                assert!(!dto_from_row(&store, home, row.clone()).restorable);
                fs::write(&registry_path, r#"{"version":4,"copies":{}}"#).unwrap();
                assert!(dto_from_row(&store, home, row.clone()).restorable);
                restore_legacy_event(&store, home, &row, false).unwrap();
                assert_eq!(
                    fs::read_to_string(to.join("SKILL.md")).unwrap(),
                    "preserved"
                );
                assert!(!from.exists());
                let restore_id = store.get(&id).unwrap().unwrap().reverted_by.unwrap();
                let descendant = store.get(&restore_id).unwrap().unwrap();
                assert_eq!(descendant.inverse.as_ref().unwrap()["op"], "restore_backup");
                assert!(dto_from_row(&store, home, descendant.clone()).restorable);
                fs::write(&registry_path, &registry_bytes).unwrap();
                assert!(!dto_from_row(&store, home, descendant.clone()).restorable);
                for force in [false, true] {
                    assert!(restore_legacy_event(&store, home, &descendant, force).is_err());
                }
                assert_eq!(
                    fs::read_to_string(to.join("SKILL.md")).unwrap(),
                    "preserved"
                );
                assert_eq!(fs::read(&registry_path).unwrap(), registry_bytes);
                assert!(store
                    .get(&restore_id)
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .is_none());
            }
        }
    }

    #[test]
    fn copy_visibility_history_requires_valid_completed_unclaimed_intent() {
        use skill_studio_core::{
            skill_copy_move::CopyMoveTransition,
            skill_copy_move_intent::CopyMoveIntent,
            skill_deployment::{deployment_id, InstallScope, SkillDestination},
            skill_fork_registry::CopyDeploymentRecord,
        };
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        let path = temp.path().join(".cursor/skills/sample");
        let before = CopyDeploymentRecord {
            deployment_id: deployment_id(
                "sample",
                "global",
                SkillDestination::PerHarness,
                "cursor",
                None,
                &path,
            ),
            name: "sample".into(),
            path,
            scope: InstallScope::Global,
            destination: SkillDestination::PerHarness,
            slot: "cursor".into(),
            project_path: None,
            content_hash: "a".repeat(64),
            disabled: false,
        };
        let intent = CopyMoveIntent::new(
            CopyMoveTransition::new(before, false).unwrap(),
            format!("tree-v1:{}", "b".repeat(64)),
            temp.path().join(".agents/skill-studio.json"),
        )
        .unwrap();
        let id = allocate_id();
        store.record(&id, intent.event_draft().unwrap()).unwrap();
        let pending = dto_from_row(&store, temp.path(), store.get(&id).unwrap().unwrap());
        assert!(!pending.restorable);
        assert!(pending.reversal_label.is_none());
        store.finish(&id, EventStatus::Done).unwrap();
        let row = store.get(&id).unwrap().unwrap();
        let completed = dto_from_row(&store, temp.path(), row.clone());
        assert!(completed.restorable);
        assert!(!completed.force_restorable);
        assert_eq!(
            completed.history_label.as_deref(),
            Some("Disabled Copy deployment")
        );
        assert_eq!(completed.reversal_label.as_deref(), Some("Enable"));
        let mut claimed = row.clone();
        claimed.reverted_by = Some(allocate_id());
        let claimed = dto_from_row(&store, temp.path(), claimed);
        assert!(!claimed.restorable);
        assert!(claimed.reversal_label.is_none());
        let mut invalid = row;
        invalid.payload["transition"]["after"]["path"] = serde_json::json!("/unrelated");
        store
            .conn
            .execute(
                "UPDATE events SET payload = ?1 WHERE id = ?2",
                rusqlite::params![invalid.payload.to_string(), invalid.id],
            )
            .unwrap();
        let invalid = dto_from_row(&store, temp.path(), invalid);
        assert!(!invalid.restorable);
        assert!(!invalid.force_restorable);
        assert!(invalid.reversal_label.is_none());
    }

    #[test]
    fn event_dto_keeps_non_restorable_backend_policy() {
        let temp = tempfile::tempdir().unwrap();
        let store = EventStore::open(&temp.path().join("app-data")).unwrap();
        let id = allocate_id();
        let path = temp.path().join("copy");
        let inverse = InverseOp::RestoreBackup {
            path,
            pre_fingerprint: "before".to_string(),
            post_fingerprint: Some("after".to_string()),
        };
        store
            .record(
                &id,
                EventDraft {
                    kind: "restore".to_string(),
                    skill: "find-bugs".to_string(),
                    harness: Some("claude-code".to_string()),
                    scope: Some("global".to_string()),
                    project_path: None,
                    payload: serde_json::json!({"target_event": "make-event"}),
                    inverse: Some(serde_json::to_value(inverse).unwrap()),
                    backup_dir: Some(format!("backups/{id}")),
                    restorable: false,
                },
            )
            .unwrap();
        store.finish(&id, EventStatus::Done).unwrap();

        let row = store.get(&id).unwrap().unwrap();
        let dto = dto_from_row(&store, temp.path(), row);
        assert!(!dto.restorable);
    }

    /// Builds a two-deployment snapshot for one skill: `broken_path` as an
    /// unresolved (broken symlink) deployment, `healthy_path` as a resolved
    /// one - for `find_deployment_at`/`repair_skill_link` validation tests,
    /// without needing a running Tauri app.
    fn fixture_snapshot(broken_path: &Path, healthy_path: &Path) -> SkillSnapshot {
        use super::super::provenance::SourceKind;
        use super::super::skill_dto::InstalledSkill;

        fn deployment(path: &Path, broken: bool) -> Deployment {
            Deployment {
                agent: "Claude Code".to_string(),
                scope: "project".to_string(),
                path: path.to_string_lossy().to_string(),
                is_symlink: true,
                plugin: None,
                symlink_is_broken: broken,
                ..Default::default()
            }
        }

        SkillSnapshot {
            ledger_only: Vec::new(),
            diagnosis: None,
            read_warnings: Vec::new(),
            revision: 0,
            full_refresh: None,
            skills: vec![InstalledSkill {
                update_sources: Vec::new(),
                name: "find-bugs".to_string(),
                source: "manual".to_string(),
                source_type: "manual".to_string(),
                source_url: None,
                skill_path: None,
                installed_at: chrono::Utc::now().to_rfc3339(),
                updated_at: None,
                has_update: false,
                update_owner_ids: Vec::new(),
                update_owners: Vec::new(),
                update_commit: None,
                update_commit_at: None,
                source_kind: SourceKind::Manual,
                deployments: vec![
                    deployment(broken_path, true),
                    deployment(healthy_path, false),
                ],
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
            heatmap: super::super::skill_invocations::InvocationHeatmap::default(),
            scanned_at: chrono::Utc::now().to_rfc3339(),
            last_test_by_skill: Default::default(),
            update_check: Default::default(),
            opencode_config_kind: None,
        }
    }

    /// A snapshot with one global, whole-dir-linked Claude Code deployment,
    /// for `validate_materialize_request` tests.
    fn fixture_materialize_snapshot(root: &Path) -> SkillSnapshot {
        use super::super::skill_deployment::{
            deployment_id, BackingRelationship, SkillDestination,
        };

        let canonical_path = PathBuf::from("/home/.agents/skills/find-bugs");
        let canonical_id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &canonical_path,
        );
        let linked_path = root.join("find-bugs");
        let linked_id = deployment_id(
            "find-bugs",
            "global",
            SkillDestination::Universal,
            "claude-code",
            None,
            &linked_path,
        );
        let mut snapshot = fixture_snapshot(&linked_path, &canonical_path);
        let linked = &mut snapshot.skills[0].deployments[0];
        linked.id = linked_id;
        linked.destination = SkillDestination::Universal;
        linked.scope = "global".to_string();
        linked.symlink_is_broken = false;
        linked.shared_via_whole_dir_link = true;
        linked.backing = BackingRelationship::LinkedTo {
            deployment_id: canonical_id.clone(),
        };
        let canonical = &mut snapshot.skills[0].deployments[1];
        canonical.id = canonical_id;
        canonical.agent = "shared".to_string();
        canonical.destination = SkillDestination::Universal;
        canonical.scope = "global".to_string();
        canonical.is_symlink = false;
        canonical.backing = BackingRelationship::Canonical;
        snapshot
    }

    fn materialize_target(snapshot: &SkillSnapshot) -> LifecycleTarget {
        LifecycleTarget {
            deployment_id: Some(snapshot.skills[0].deployments[0].id.clone()),
            owner_id: None,
        }
    }

    #[test]
    fn validate_materialize_request_accepts_a_recorded_whole_dir_link() {
        let root = PathBuf::from("/home/.claude/skills");
        let snapshot = fixture_materialize_snapshot(&root);
        let target = materialize_target(&snapshot);
        assert!(validate_materialize_request(
            &snapshot,
            &target,
            "claude-code",
            "/home/.claude/skills"
        )
        .is_ok());
    }

    #[test]
    fn materialize_refuses_when_fresh_snapshot_no_longer_has_cached_target() {
        let root = PathBuf::from("/home/.claude/skills");
        let cached = fixture_materialize_snapshot(&root);
        let target = materialize_target(&cached);
        assert!(validate_materialize_request(
            &cached,
            &target,
            "claude-code",
            "/home/.claude/skills"
        )
        .is_ok());

        let mut fresh = cached;
        fresh.skills[0]
            .deployments
            .retain(|deployment| target.deployment_id.as_deref() != Some(&deployment.id));
        assert!(validate_materialize_request(
            &fresh,
            &target,
            "claude-code",
            "/home/.claude/skills"
        )
        .is_err());
    }

    #[test]
    fn materialize_resolves_whole_root_children_to_the_exact_universal_deployment() {
        use super::super::skill_deployment::{id_for_candidate, DeploymentCandidate};

        for (label, harness, root) in [
            ("Claude Code", "claude-code", "/home/.claude/skills"),
            ("OpenCode", "open-code", "/home/.config/opencode/skills"),
        ] {
            let root = PathBuf::from(root);
            let linked_path = root.join("find-bugs");
            let resolved_path = PathBuf::from("/home/.agents/skills/find-bugs");
            let (linked_id, _, backing) = id_for_candidate(DeploymentCandidate {
                name: "find-bugs",
                root_label: label,
                scope: "global",
                path: &linked_path,
                project_path: None,
                is_symlink: false,
                symlink_target: None,
                resolved_path: Some(&resolved_path),
                shared_via_whole_dir_link: true,
            });
            let mut snapshot = fixture_materialize_snapshot(&root);
            let linked = &mut snapshot.skills[0].deployments[0];
            linked.id = linked_id.clone();
            linked.agent = label.to_string();
            linked.path = linked_path.to_string_lossy().into_owned();
            linked.resolved_path = Some(resolved_path.to_string_lossy().into_owned());
            linked.backing = backing;
            let target = LifecycleTarget {
                deployment_id: Some(linked_id),
                owner_id: None,
            };

            let selected =
                validate_materialize_request(&snapshot, &target, harness, &root.to_string_lossy())
                    .unwrap();
            assert_eq!(selected, PathBuf::from("/home/.agents/skills"), "{label}");
        }
    }

    #[test]
    fn validate_materialize_request_rejects_a_root_not_in_the_snapshot() {
        let root = PathBuf::from("/home/.claude/skills");
        let snapshot = fixture_materialize_snapshot(&root);
        let target = materialize_target(&snapshot);
        let err =
            validate_materialize_request(&snapshot, &target, "claude-code", "/home/.codex/skills")
                .unwrap_err();
        assert!(err.contains("/home/.codex/skills"), "{err}");
    }

    #[test]
    fn validate_materialize_request_rejects_a_harness_root_mismatch() {
        let root = PathBuf::from("/home/.claude/skills");
        let snapshot = fixture_materialize_snapshot(&root);
        let target = materialize_target(&snapshot);
        // The root is recorded for Claude Code, not Codex - the two must
        // agree, not just each independently point at something real.
        let err = validate_materialize_request(&snapshot, &target, "codex", "/home/.claude/skills")
            .unwrap_err();
        assert!(err.contains("codex"), "{err}");
    }

    #[test]
    fn materialize_same_name_project_target_resolves_only_project_universal_root() {
        use super::super::skill_deployment::{
            deployment_id, BackingRelationship, SkillDestination,
        };

        let mut snapshot = fixture_materialize_snapshot(Path::new("/home/.claude/skills"));
        let project_root = PathBuf::from("/work/app/.agents/skills");
        let project_skill = project_root.join("find-bugs");
        let project_id = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::Universal,
            "universal",
            Some("/work/app"),
            &project_skill,
        );
        let linked_path = PathBuf::from("/work/app/.claude/skills/find-bugs");
        let linked_id = deployment_id(
            "find-bugs",
            "project",
            SkillDestination::Universal,
            "claude-code",
            Some("/work/app"),
            &linked_path,
        );
        let mut project_link = snapshot.skills[0].deployments[0].clone();
        project_link.id = linked_id.clone();
        project_link.scope = "project".to_string();
        project_link.project_path = Some("/work/app".to_string());
        project_link.path = linked_path.to_string_lossy().into_owned();
        project_link.backing = BackingRelationship::LinkedTo {
            deployment_id: project_id.clone(),
        };
        let mut project_universal = snapshot.skills[0].deployments[1].clone();
        project_universal.id = project_id;
        project_universal.scope = "project".to_string();
        project_universal.project_path = Some("/work/app".to_string());
        project_universal.path = project_skill.to_string_lossy().into_owned();
        snapshot.skills[0].deployments.push(project_link);
        snapshot.skills[0].deployments.push(project_universal);
        let target = LifecycleTarget {
            deployment_id: Some(linked_id),
            owner_id: None,
        };

        let selected = validate_materialize_request(
            &snapshot,
            &target,
            "claude-code",
            "/work/app/.claude/skills",
        )
        .unwrap();
        assert_eq!(selected, project_root);
    }

    #[test]
    fn find_deployment_at_matches_a_broken_symlink_by_its_own_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let broken = root.join("find-bugs-claude");
        symlink("/does/not/exist", &broken).unwrap();
        let healthy = root.join("find-bugs-codex");
        fs::create_dir_all(&healthy).unwrap();

        let snapshot = fixture_snapshot(&broken, &healthy);
        let (skill, deployment) = find_deployment_at(&snapshot, &broken).unwrap();
        assert_eq!(skill, "find-bugs");
        assert!(is_unresolved(deployment));
    }

    #[test]
    fn find_deployment_at_rejects_a_path_outside_the_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let broken = root.join("find-bugs-claude");
        symlink("/does/not/exist", &broken).unwrap();
        let healthy = root.join("find-bugs-codex");
        fs::create_dir_all(&healthy).unwrap();
        let outside = root.join("some-other-skill");
        fs::create_dir_all(&outside).unwrap();

        let snapshot = fixture_snapshot(&broken, &healthy);
        assert!(find_deployment_at(&snapshot, &outside).is_none());
    }

    #[test]
    fn repair_refuses_when_fresh_snapshot_no_longer_has_cached_link() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let broken = root.join("find-bugs-claude");
        symlink("/does/not/exist", &broken).unwrap();
        let healthy = root.join("find-bugs-codex");
        fs::create_dir_all(&healthy).unwrap();
        let cached = fixture_snapshot(&broken, &healthy);
        assert!(find_deployment_at(&cached, &broken).is_some());

        let mut fresh = cached;
        fresh.skills[0]
            .deployments
            .retain(|deployment| Path::new(&deployment.path) != broken);
        assert!(find_deployment_at(&fresh, &broken).is_none());
    }

    #[test]
    fn find_deployment_at_finds_the_healthy_deployment_as_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let broken = root.join("find-bugs-claude");
        symlink("/does/not/exist", &broken).unwrap();
        let healthy = root.join("find-bugs-codex");
        fs::create_dir_all(&healthy).unwrap();

        let snapshot = fixture_snapshot(&broken, &healthy);
        let (_, deployment) = find_deployment_at(&snapshot, &healthy).unwrap();
        assert!(!is_unresolved(deployment));
    }
}
