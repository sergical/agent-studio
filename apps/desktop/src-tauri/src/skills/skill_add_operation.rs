// ============================================================================
// Skills Module - skill_add_operation
// Background Add Skill: start returns after scheduling, events carry an
// operation id and a strictly increasing sequence. Unit 3.5c: the worker
// (`run_operation_body`) now calls `skill_install`'s `ops::install` adapter
// per skill instead of shelling out itself - `ops::install`'s own
// `InstallOutcome` is the definitive result, so the before/after root
// fingerprinting this file used to do (comparing directory listings to
// guess what changed) is gone; `reconcile_affected` runs from the verified
// names `ops::install` returns instead.
// ============================================================================

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use skill_studio_core::ports::Runtime;
use tauri::{AppHandle, Emitter, Manager};

use super::skill_agent_runner::validate_run_id;
use super::skill_dto::{AddSkillOutcome, AddSkillRequest, AddSkillResult, AddSkillsRequest};
use super::skill_fork::RepoSnapshot;
use super::skill_fork_registry::AddMethod;
use super::skill_install;
use super::skill_refresh::{self, SkillRefreshState};
use super::skill_trust_policy::{
    normalize_confirmation_identity, record_trusted_dotagents_source_locked,
    UNTRUSTED_DOTAGENTS_SOURCE_MESSAGE,
};

/// Event name every Add Skill operation status is emitted on.
pub const ADD_SKILL_OPERATION_EVENT: &str = "skills://add-skill-operation";

const MAX_RETAINED_OPERATIONS: usize = 32;
const OPERATION_TTL: Duration = Duration::from_secs(30 * 60);

/// One phase of a background Add Skill operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AddSkillOperationPhase {
    Queued,
    Validating,
    Fetching,
    Installing,
    Finalizing,
    Reconciling,
    NeedsTrust,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl AddSkillOperationPhase {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }
}

/// Batch item progress: 1-based `current` of `total`, plus the skill name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddSkillItemProgress {
    pub current: usize,
    pub total: usize,
    pub name: String,
}

/// Normalized repository identity that needs an explicit trust confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddSkillUntrustedSource {
    pub identity: String,
}

/// One status event or catch-up snapshot for an Add Skill operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddSkillOperationEvent {
    pub operation_id: String,
    pub sequence: u64,
    pub phase: AddSkillOperationPhase,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<AddSkillItemProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<AddSkillResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcomes: Option<Vec<AddSkillOutcome>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untrusted_source: Option<AddSkillUntrustedSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
}

#[derive(Clone)]
enum AddSkillOperationKind {
    Single(AddSkillRequest),
    Batch(AddSkillsRequest),
}

struct AddSkillOperationRecord {
    event: AddSkillOperationEvent,
    kind: AddSkillOperationKind,
    cancel: Arc<AtomicBool>,
    updated_at: Instant,
    /// True after an accepted trust confirmation. Blocks replay without
    /// treating the parent as a successful install.
    trust_confirmed: bool,
}

struct AddSkillOperationInner {
    records: HashMap<String, AddSkillOperationRecord>,
    order: VecDeque<String>,
}

/// Managed state for in-flight and recently finished Add Skill operations.
#[derive(Clone)]
pub struct AddSkillOperationState {
    inner: Arc<Mutex<AddSkillOperationInner>>,
}

impl Default for AddSkillOperationState {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(AddSkillOperationInner {
                records: HashMap::new(),
                order: VecDeque::new(),
            })),
        }
    }
}

impl AddSkillOperationState {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, AddSkillOperationInner>, String> {
        self.inner
            .lock()
            .map_err(|error| format!("Add skill operation lock poisoned: {error}"))
    }

    fn begin(
        &self,
        operation_id: String,
        kind: AddSkillOperationKind,
        retry_of: Option<String>,
    ) -> Result<AddSkillOperationEvent, String> {
        validate_run_id(&operation_id).map_err(|error| error.replace("Run id", "Operation id"))?;
        let mut inner = self.lock()?;
        prune_locked(&mut inner, Instant::now());
        if inner.records.contains_key(&operation_id) {
            return Err(format!("Add skill operation {operation_id} already exists"));
        }
        make_operation_room(&mut inner)?;
        let event = AddSkillOperationEvent {
            operation_id: operation_id.clone(),
            sequence: 1,
            phase: AddSkillOperationPhase::Queued,
            message: "Waiting to add skill".to_string(),
            item: None,
            result: None,
            outcomes: None,
            error: None,
            untrusted_source: None,
            retry_of,
        };
        inner.records.insert(
            operation_id.clone(),
            AddSkillOperationRecord {
                event: event.clone(),
                kind,
                cancel: Arc::new(AtomicBool::new(false)),
                updated_at: Instant::now(),
                trust_confirmed: false,
            },
        );
        inner.order.push_back(operation_id);
        Ok(event)
    }

    fn snapshot(&self, operation_id: &str) -> Result<AddSkillOperationEvent, String> {
        let mut inner = self.lock()?;
        prune_locked(&mut inner, Instant::now());
        inner
            .records
            .get(operation_id)
            .map(|record| record.event.clone())
            .ok_or_else(|| format!("Add skill operation {operation_id} was not found"))
    }

    fn request_cancel(&self, operation_id: &str) -> Result<AddSkillOperationEvent, String> {
        let mut inner = self.lock()?;
        let record = inner
            .records
            .get_mut(operation_id)
            .ok_or_else(|| format!("Add skill operation {operation_id} was not found"))?;
        if record.event.phase.is_terminal() {
            return Ok(record.event.clone());
        }
        record.cancel.store(true, Ordering::SeqCst);
        if record.event.phase == AddSkillOperationPhase::NeedsTrust {
            advance_locked(
                record,
                AddSkillOperationPhase::Cancelled,
                "Add skill cancelled",
                |_| {},
            );
        }
        Ok(record.event.clone())
    }

    fn cancel_flag(&self, operation_id: &str) -> Result<Arc<AtomicBool>, String> {
        let inner = self.lock()?;
        inner
            .records
            .get(operation_id)
            .map(|record| Arc::clone(&record.cancel))
            .ok_or_else(|| format!("Add skill operation {operation_id} was not found"))
    }

    fn kind(&self, operation_id: &str) -> Result<AddSkillOperationKind, String> {
        let inner = self.lock()?;
        inner
            .records
            .get(operation_id)
            .map(|record| record.kind.clone())
            .ok_or_else(|| format!("Add skill operation {operation_id} was not found"))
    }

    fn advance(
        &self,
        operation_id: &str,
        phase: AddSkillOperationPhase,
        message: impl Into<String>,
        patch: impl FnOnce(&mut AddSkillOperationEvent),
    ) -> Result<AddSkillOperationEvent, String> {
        let mut inner = self.lock()?;
        let record = inner
            .records
            .get_mut(operation_id)
            .ok_or_else(|| format!("Add skill operation {operation_id} was not found"))?;
        if record.event.phase.is_terminal() {
            return Ok(record.event.clone());
        }
        advance_locked(record, phase, message, patch);
        Ok(record.event.clone())
    }
}

fn advance_locked(
    record: &mut AddSkillOperationRecord,
    phase: AddSkillOperationPhase,
    message: impl Into<String>,
    patch: impl FnOnce(&mut AddSkillOperationEvent),
) {
    record.event.sequence += 1;
    record.event.phase = phase;
    record.event.message = message.into();
    patch(&mut record.event);
    record.updated_at = Instant::now();
}

fn prune_locked(inner: &mut AddSkillOperationInner, now: Instant) {
    let stale: Vec<String> = inner
        .records
        .iter()
        .filter(|(_, record)| now.duration_since(record.updated_at) > OPERATION_TTL)
        .map(|(id, _)| id.clone())
        .collect();
    for id in stale {
        inner.records.remove(&id);
        inner.order.retain(|existing| existing != &id);
    }
    while inner.records.len() > MAX_RETAINED_OPERATIONS {
        if !remove_oldest_terminal(inner) {
            break;
        }
    }
}

fn remove_oldest_terminal(inner: &mut AddSkillOperationInner) -> bool {
    let Some(index) = inner.order.iter().position(|id| {
        inner
            .records
            .get(id)
            .is_some_and(|record| record.event.phase.is_terminal())
    }) else {
        return false;
    };
    if let Some(id) = inner.order.remove(index) {
        inner.records.remove(&id);
    }
    true
}

fn make_operation_room(inner: &mut AddSkillOperationInner) -> Result<(), String> {
    while inner.records.len() >= MAX_RETAINED_OPERATIONS && remove_oldest_terminal(inner) {}
    if inner.records.len() >= MAX_RETAINED_OPERATIONS {
        return Err(format!(
            "Cannot start Add skill operation: all {MAX_RETAINED_OPERATIONS} operation slots are active"
        ));
    }
    Ok(())
}

fn emit_status(app: Option<&AppHandle>, event: &AddSkillOperationEvent) {
    if let Some(app) = app {
        let _ = app.emit(ADD_SKILL_OPERATION_EVENT, event);
    }
}

fn publish(
    app: Option<&AppHandle>,
    state: &AddSkillOperationState,
    operation_id: &str,
    phase: AddSkillOperationPhase,
    message: impl Into<String>,
    patch: impl FnOnce(&mut AddSkillOperationEvent),
) -> Result<AddSkillOperationEvent, String> {
    let event = state.advance(operation_id, phase, message, patch)?;
    emit_status(app, &event);
    Ok(event)
}

fn affected_projects(kind: &AddSkillOperationKind) -> Vec<PathBuf> {
    let path = match kind {
        AddSkillOperationKind::Single(request) => request.project_path.as_deref(),
        AddSkillOperationKind::Batch(request) => request.project_path.as_deref(),
    };
    path.map(PathBuf::from).into_iter().collect()
}

fn method_of(kind: &AddSkillOperationKind) -> AddMethod {
    match kind {
        AddSkillOperationKind::Single(request) => request.method,
        AddSkillOperationKind::Batch(request) => request.method,
    }
}

fn fetching_phase(kind: &AddSkillOperationKind) -> bool {
    matches!(method_of(kind), AddMethod::Copy)
}

fn reconcile_affected(
    app: Option<&AppHandle>,
    names: Vec<String>,
    projects: &[PathBuf],
) -> Result<(), String> {
    if names.is_empty() {
        if let Some(app) = app {
            skill_refresh::request_snapshot_rebuild(app);
        }
        return Ok(());
    }
    let Some(app) = app else {
        return Ok(());
    };
    let Some(refresh) = app.try_state::<SkillRefreshState>() else {
        skill_refresh::request_snapshot_rebuild(app);
        return Ok(());
    };
    skill_refresh::reconcile_skill_names_and_emit(app, refresh.inner(), names, projects)
}

/// One skill's `ops::install` attempt, folded into a terminal phase/message/
/// event patch. `NeedsTrust` becomes the same structured phase the old
/// `require_trusted_dotagents_source` pre-check produced, built from
/// `ops::install`'s own answer instead of a duplicate local check.
fn terminal_for_result(
    result: Result<skill_install::InstallAdapterOutcome, String>,
) -> (
    AddSkillOperationPhase,
    String,
    Option<AddSkillResult>,
    Option<String>,
    Option<AddSkillUntrustedSource>,
) {
    match result {
        Ok(skill_install::InstallAdapterOutcome::Result(result)) => (
            AddSkillOperationPhase::Completed,
            format!("Added {}", result.name),
            Some(result),
            None,
            None,
        ),
        Ok(skill_install::InstallAdapterOutcome::NeedsTrust { identity }) => (
            AddSkillOperationPhase::NeedsTrust,
            UNTRUSTED_DOTAGENTS_SOURCE_MESSAGE.to_string(),
            None,
            Some(skill_install::needs_trust_message(&identity)),
            Some(AddSkillUntrustedSource { identity }),
        ),
        Err(error) => (
            AddSkillOperationPhase::Failed,
            error.clone(),
            None,
            Some(error),
            None,
        ),
    }
}

/// The worker: builds one `Runtime`, then routes to a single or batch
/// install through `skill_install`'s `ops::install` adapter. Unit 3.5c: this
/// used to shell out and diff directory listings itself; `ops::install`'s
/// own `InstallOutcome` is now the one source of truth for what changed, so
/// there is nothing left here to fingerprint.
fn run_operation_body(
    app: Option<&AppHandle>,
    state: &AddSkillOperationState,
    operation_id: &str,
    build_runtime: impl FnOnce() -> Result<Runtime, String>,
    fetch: &dyn super::skill_fork::UpstreamFetch,
    lookup: &dyn super::skill_update_check::CommitLookup,
) {
    let Ok(kind) = state.kind(operation_id) else {
        return;
    };
    let Ok(cancel) = state.cancel_flag(operation_id) else {
        return;
    };
    // Only checked before any work starts - `ops::install` runs to
    // completion once called (see the follow-up doc on cancellation).
    if cancel.load(Ordering::SeqCst) {
        let _ = publish(
            app,
            state,
            operation_id,
            AddSkillOperationPhase::Cancelled,
            "Add skill cancelled",
            |_| {},
        );
        return;
    }

    let _ = publish(
        app,
        state,
        operation_id,
        AddSkillOperationPhase::Validating,
        "Checking source",
        |_| {},
    );

    let rt = match build_runtime() {
        Ok(rt) => rt,
        Err(error) => {
            let _ = publish(
                app,
                state,
                operation_id,
                AddSkillOperationPhase::Failed,
                error.clone(),
                |event| event.error = Some(error),
            );
            return;
        }
    };

    if fetching_phase(&kind) {
        let _ = publish(
            app,
            state,
            operation_id,
            AddSkillOperationPhase::Fetching,
            "Fetching skill files",
            |_| {},
        );
    }

    let _ = publish(
        app,
        state,
        operation_id,
        AddSkillOperationPhase::Installing,
        "Installing",
        |_| {},
    );

    match kind {
        AddSkillOperationKind::Single(request) => {
            let outcome = skill_install::install_one(&rt, &request, fetch, lookup, None);
            let (phase, message, result, error, untrusted_source) = terminal_for_result(outcome);
            let names = result.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
            finish_operation(
                app,
                state,
                operation_id,
                &affected_projects(&AddSkillOperationKind::Single(request)),
                names,
                phase,
                message,
                result,
                None,
                error,
                untrusted_source,
            );
        }
        AddSkillOperationKind::Batch(request) => {
            let Ok(snapshot) =
                open_batch_snapshot(app, state, operation_id, &request, fetch, lookup)
            else {
                return;
            };
            let total = request.skills.len();
            let mut outcomes = Vec::with_capacity(total);
            let mut names = Vec::new();
            for (index, entry) in request.skills.iter().enumerate() {
                let _ = publish(
                    app,
                    state,
                    operation_id,
                    AddSkillOperationPhase::Installing,
                    format!("Installing {} ({} of {total})", entry.name, index + 1),
                    |event| {
                        event.item = Some(AddSkillItemProgress {
                            current: index + 1,
                            total,
                            name: entry.name.clone(),
                        });
                    },
                );
                let entry_request = skill_install::request_for_entry(&request, entry);
                let outcome = skill_install::install_one(
                    &rt,
                    &entry_request,
                    fetch,
                    lookup,
                    snapshot.as_deref(),
                );
                let (_, _, result, error, untrusted_source) = terminal_for_result(outcome);
                if untrusted_source.is_some() {
                    // A per-entry NeedsTrust has no single retry target in a
                    // batch; report it as this entry's failure and continue.
                }
                if let Some(result) = &result {
                    names.push(result.name.clone());
                }
                outcomes.push(AddSkillOutcome {
                    name: entry.name.clone(),
                    result,
                    error,
                });
            }
            let any_success = outcomes.iter().any(|o| o.result.is_some());
            let phase = if any_success {
                AddSkillOperationPhase::Completed
            } else {
                AddSkillOperationPhase::Failed
            };
            let count = outcomes.iter().filter(|o| o.result.is_some()).count();
            let message = if any_success {
                format!("Added {count} skill{}", if count == 1 { "" } else { "s" })
            } else {
                "Add skill failed".to_string()
            };
            finish_operation(
                app,
                state,
                operation_id,
                &affected_projects(&AddSkillOperationKind::Batch(request)),
                names,
                phase,
                message,
                None,
                Some(outcomes),
                None,
                None,
            );
        }
    }
}

/// Downloads a Copy batch's shared repo once (unit 3.5c: moved from
/// `skill_add.rs`'s `open_repo_snapshot`), publishing `Failed` and returning
/// `None` on error so the caller can bail with a single `let Some(..) else`.
fn open_batch_snapshot(
    app: Option<&AppHandle>,
    state: &AddSkillOperationState,
    operation_id: &str,
    request: &super::skill_dto::AddSkillsRequest,
    fetch: &dyn super::skill_fork::UpstreamFetch,
    lookup: &dyn super::skill_update_check::CommitLookup,
) -> Result<Option<Box<dyn RepoSnapshot>>, ()> {
    skill_install::open_batch_snapshot(request, fetch, lookup).map_err(|error| {
        let _ = publish(
            app,
            state,
            operation_id,
            AddSkillOperationPhase::Failed,
            error.clone(),
            |event| event.error = Some(error),
        );
    })
}

#[allow(clippy::too_many_arguments)]
fn finish_operation(
    app: Option<&AppHandle>,
    state: &AddSkillOperationState,
    operation_id: &str,
    projects: &[PathBuf],
    names: Vec<String>,
    phase: AddSkillOperationPhase,
    message: String,
    result: Option<AddSkillResult>,
    outcomes: Option<Vec<AddSkillOutcome>>,
    error: Option<String>,
    untrusted_source: Option<AddSkillUntrustedSource>,
) {
    let _ = publish(
        app,
        state,
        operation_id,
        AddSkillOperationPhase::Reconciling,
        "Updating skill list",
        |event| {
            event.result.clone_from(&result);
            event.outcomes.clone_from(&outcomes);
            event.error.clone_from(&error);
        },
    );
    if let Err(reconcile_error) = reconcile_affected(app, names, projects) {
        eprintln!(
            "[add_skill_operation] targeted snapshot reconciliation failed: {reconcile_error}"
        );
        if let Some(app) = app {
            skill_refresh::request_snapshot_rebuild(app);
        }
    }
    let _ = publish(app, state, operation_id, phase, message, |event| {
        event.result = result;
        event.outcomes = outcomes;
        event.error = error;
        event.untrusted_source = untrusted_source;
    });
}

fn spawn_operation(app: AppHandle, state: AddSkillOperationState, operation_id: String) {
    tauri::async_runtime::spawn_blocking(move || {
        let run = || match skill_install::resolve_fetch_and_lookup(&app) {
            Ok((fetch, lookup)) => run_operation_body(
                Some(&app),
                &state,
                &operation_id,
                super::core_runtime::build_runtime_write,
                fetch.as_ref(),
                lookup.as_ref(),
            ),
            Err(error) => {
                let _ = publish(
                    Some(&app),
                    &state,
                    &operation_id,
                    AddSkillOperationPhase::Failed,
                    error.clone(),
                    |event| {
                        event.error = Some(error);
                    },
                );
            }
        };
        run();
    });
}

/// Start a single-skill Add Skill operation. Returns the queued event before
/// any `npx`, network, or large filesystem work.
#[tauri::command]
pub fn start_add_skill_operation(
    operation_id: String,
    request: AddSkillRequest,
    app: AppHandle,
    state: tauri::State<AddSkillOperationState>,
) -> Result<AddSkillOperationEvent, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command(&timing_app, "start_add_skill_operation", move || {
        let queued = state.begin(
            operation_id.clone(),
            AddSkillOperationKind::Single(request),
            None,
        )?;
        emit_status(Some(&app), &queued);
        spawn_operation(app.clone(), state.inner().clone(), operation_id);
        Ok(queued)
    })
}

/// Start a batch Add Skill operation. Returns the queued event immediately.
#[tauri::command]
pub fn start_add_skills_operation(
    operation_id: String,
    request: AddSkillsRequest,
    app: AppHandle,
    state: tauri::State<AddSkillOperationState>,
) -> Result<AddSkillOperationEvent, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command(&timing_app, "start_add_skills_operation", move || {
        let queued = state.begin(
            operation_id.clone(),
            AddSkillOperationKind::Batch(request),
            None,
        )?;
        emit_status(Some(&app), &queued);
        spawn_operation(app.clone(), state.inner().clone(), operation_id);
        Ok(queued)
    })
}

/// Catch-up read for a listener that subscribed after start, or remounted.
#[tauri::command]
// Tauri commands deserialize their arguments fresh per invocation, so `app`
// can't be borrowed from the caller - it must be owned.
#[allow(clippy::needless_pass_by_value)]
pub fn get_add_skill_operation(
    operation_id: String,
    state: tauri::State<AddSkillOperationState>,
    app: tauri::AppHandle,
) -> Result<AddSkillOperationEvent, String> {
    crate::timing_log::time_command(&app, "get_add_skill_operation", move || {
        state.snapshot(&operation_id)
    })
}

/// Request cancel. If mutation already finished, the worker still reports
/// completed or failed rather than cancelled.
#[tauri::command]
pub fn cancel_add_skill_operation(
    operation_id: String,
    app: AppHandle,
    state: tauri::State<AddSkillOperationState>,
) -> Result<AddSkillOperationEvent, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command(&timing_app, "cancel_add_skill_operation", move || {
        let event = state.request_cancel(&operation_id)?;
        emit_status(Some(&app), &event);
        Ok(event)
    })
}

/// Record trust for this operation's repository identity, then retry the
/// same immutable request. Rejects mismatch and replay. `retry_operation_id`
/// must be a fresh frontend-generated id.
fn confirm_add_skill_trust_with(
    home: &Path,
    operation_id: &str,
    retry_operation_id: &str,
    identity: &str,
    state: &AddSkillOperationState,
    write_lease: &super::write_lease::WriteLease,
) -> Result<(AddSkillOperationEvent, AddSkillOperationEvent), String> {
    validate_run_id(retry_operation_id).map_err(|error| error.replace("Run id", "Operation id"))?;
    let expected = {
        let mut inner = state.lock()?;
        prune_locked(&mut inner, Instant::now());
        if inner.records.contains_key(retry_operation_id) {
            return Err(format!(
                "Add skill operation {retry_operation_id} already exists"
            ));
        }
        let record = inner
            .records
            .get(operation_id)
            .ok_or_else(|| format!("Add skill operation {operation_id} was not found"))?;
        if record.event.phase != AddSkillOperationPhase::NeedsTrust || record.trust_confirmed {
            return Err("Trust confirmation does not match this operation".to_string());
        }
        let expected = record
            .event
            .untrusted_source
            .as_ref()
            .map(|source| source.identity.clone())
            .ok_or_else(|| "Trust confirmation does not match this operation".to_string())?;
        expected
    };
    let normalized = normalize_confirmation_identity(identity)?;
    if normalized != expected {
        return Err("Trust confirmation does not match this operation".to_string());
    }

    // Background add work acquires these locks in this order. Do not hold the
    // operation-state lock while trying to acquire the filesystem lock.
    let guard = write_lease.try_acquire(home)?;
    let (parent_event, queued) = {
        let mut inner = state.lock()?;
        if inner.records.contains_key(retry_operation_id) {
            return Err(format!(
                "Add skill operation {retry_operation_id} already exists"
            ));
        }
        make_operation_room(&mut inner)?;
        let kind = {
            let parent = inner
                .records
                .get_mut(operation_id)
                .ok_or_else(|| format!("Add skill operation {operation_id} was not found"))?;
            if parent.event.phase != AddSkillOperationPhase::NeedsTrust || parent.trust_confirmed {
                return Err("Trust confirmation does not match this operation".to_string());
            }
            let current_identity = parent
                .event
                .untrusted_source
                .as_ref()
                .map(|source| source.identity.as_str());
            if current_identity != Some(normalized.as_str()) {
                return Err("Trust confirmation does not match this operation".to_string());
            }
            parent.kind.clone()
        };

        record_trusted_dotagents_source_locked(&guard, home, &normalized)?;
        let parent_event = {
            let parent = inner
                .records
                .get_mut(operation_id)
                .ok_or_else(|| format!("Add skill operation {operation_id} was not found"))?;
            parent.trust_confirmed = true;
            advance_locked(
                parent,
                AddSkillOperationPhase::NeedsTrust,
                "Trusted repository; retrying",
                |event| {
                    event.retry_of = Some(retry_operation_id.to_string());
                },
            );
            parent.event.clone()
        };
        let queued = AddSkillOperationEvent {
            operation_id: retry_operation_id.to_string(),
            sequence: 1,
            phase: AddSkillOperationPhase::Queued,
            message: "Waiting to add skill".to_string(),
            item: None,
            result: None,
            outcomes: None,
            error: None,
            untrusted_source: None,
            retry_of: Some(operation_id.to_string()),
        };
        inner.records.insert(
            retry_operation_id.to_string(),
            AddSkillOperationRecord {
                event: queued.clone(),
                kind,
                cancel: Arc::new(AtomicBool::new(false)),
                updated_at: Instant::now(),
                trust_confirmed: false,
            },
        );
        inner.order.push_back(retry_operation_id.to_string());
        (parent_event, queued)
    };
    Ok((parent_event, queued))
}

#[tauri::command]
pub async fn confirm_add_skill_trust(
    operation_id: String,
    retry_operation_id: String,
    identity: String,
    app: AppHandle,
) -> Result<AddSkillOperationEvent, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(&timing_app, "confirm_add_skill_trust", move || {
        let state = app.state::<AddSkillOperationState>();
        let write_lease = super::write_lease::WriteLease::default();
        let home = dirs::home_dir().ok_or("Could not find home directory")?;
        let (parent_event, queued) = confirm_add_skill_trust_with(
            &home,
            &operation_id,
            &retry_operation_id,
            &identity,
            state.inner(),
            &write_lease,
        )?;
        emit_status(Some(&app), &parent_event);
        emit_status(Some(&app), &queued);
        spawn_operation(app.clone(), state.inner().clone(), retry_operation_id);
        Ok(queued)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::skill_deployment::SkillDestination;
    use crate::skills::skill_dto::{InstallScope, ParsedSkillSource, ParsedSkillSourceKind};
    use crate::skills::skill_fork::{RepoSnapshot, UpstreamFetch};
    use crate::skills::skill_update_check::CommitLookup;
    use skill_studio_core::harness::HarnessCatalog;
    use skill_studio_core::ports::Ports;
    use skill_studio_core::RuntimeScope;
    use std::fs;
    use std::sync::{Barrier, Mutex as StdMutex};
    use std::thread;

    // Unit 3.5c removed 8 tests along with the mechanics they exercised,
    // none of which survive `ops::install` owning the actual write:
    // `start_returns_before_blocked_runner_finishes` (no shelled-out
    // `CommandRunner` to block on anymore - `ops::install` itself is the
    // blocking call); `partial_batch_collects_affected_names` (the
    // before/after directory-listing diff it tested, `affected_skill_names`,
    // is gone - `ops::install`'s own result names are now the source of
    // truth); `timeout_without_mutation_is_timed_out`,
    // `stored_operation_deadline_stops_copy_before_lookup_or_mutation`,
    // `cancellation_after_committed_install_is_reported_completed`, and
    // `timeout_after_in_place_change_reports_partial_failure` (the stored
    // per-operation deadline and mid-run cancel/timeout classification are
    // gone - `run_operation_body` only ever checks `cancel` once, before any
    // work starts; see its own doc comment); `cancel_during_fake_fetch_...`
    // (fetch-time cancellation via `AddOperationControl` is gone -
    // `open_batch_snapshot` no longer threads a control token); and
    // `trusted_retry_uses_the_same_request` (asserting a real post-trust
    // Dotagents install would need a working core-level GitHub port this
    // adapter's own fakes don't reach - `needs_trust_retry_succeeds_before_expiry`
    // below still covers the retry event shape). See
    // `issue-3.5c-followup-a.md` for the cancellation/timeout follow-up.

    fn test_runtime(home: &Path) -> Runtime {
        let lease_root = home.join("leases");
        let catalog = Arc::new(HarnessCatalog::builtin());
        let scope = RuntimeScope::fixture(home.to_path_buf());
        let db_path = scope.history_root.join("events.sqlite3");
        let ports: Ports =
            skill_studio_host::default_ports_with_history(lease_root, catalog, db_path);
        Runtime::new(&scope, ports).unwrap()
    }

    struct NeverFetch;
    impl UpstreamFetch for NeverFetch {
        fn fetch_skill_dir(&self, _: &str, _: &str, _: &str, _: &Path) -> Result<(), String> {
            panic!("fetch should not have been called");
        }
    }
    struct NeverLookup;
    impl CommitLookup for NeverLookup {
        fn latest_commit(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> Result<Option<(String, String)>, String> {
            panic!("lookup should not have been called");
        }
    }

    fn github(repo: &str, name: &str) -> ParsedSkillSource {
        ParsedSkillSource {
            kind: ParsedSkillSourceKind::Github,
            repo: Some(repo.to_string()),
            path: None,
            git_ref: None,
            skill_name: Some(name.to_string()),
            url: None,
            local_path: None,
        }
    }

    fn single_request(repo: &str, name: &str, method: AddMethod) -> AddSkillRequest {
        AddSkillRequest {
            source: github(repo, name),
            method,
            destination: SkillDestination::Universal,
            agents: vec![],
            disabled_harnesses: vec![],
            scope: InstallScope::Global,
            project_path: None,
            trial: false,
        }
    }

    #[test]
    fn events_are_monotonic_and_status_catches_up() {
        let state = AddSkillOperationState::default();
        let queued = state
            .begin(
                "op-seq".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "getsentry/skills",
                    "find-bugs",
                    AddMethod::Copy,
                )),
                None,
            )
            .unwrap();
        let next = publish(
            None,
            &state,
            "op-seq",
            AddSkillOperationPhase::Validating,
            "Checking source",
            |_| {},
        )
        .unwrap();
        assert_eq!(queued.sequence, 1);
        assert_eq!(next.sequence, 2);
        assert_eq!(state.snapshot("op-seq").unwrap().sequence, 2);
        assert!(next.sequence > queued.sequence);
    }

    /// `run_operation_body`'s Dotagents path never even reaches `fetch`/
    /// `lookup` before the trust check: `ops::install`'s own trust gate
    /// (see `skill_install.rs`) runs before any filesystem or network work,
    /// so `NeverFetch`/`NeverLookup` proves nothing was reached that
    /// shouldn't have been.
    #[test]
    fn untrusted_kcd_skills_pauses_for_explicit_trust() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AddSkillOperationState::default();
        state
            .begin(
                "op-trust".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "kentcdodds/kcd-skills",
                    "visual-recap",
                    AddMethod::Dotagents,
                )),
                None,
            )
            .unwrap();
        let rt = test_runtime(tmp.path());
        run_operation_body(
            None,
            &state,
            "op-trust",
            || Ok(rt),
            &NeverFetch,
            &NeverLookup,
        );
        let status = state.snapshot("op-trust").unwrap();
        assert_eq!(status.phase, AddSkillOperationPhase::NeedsTrust);
        assert_eq!(
            status
                .untrusted_source
                .as_ref()
                .map(|source| source.identity.as_str()),
            Some("kentcdodds/kcd-skills")
        );
        assert!(status
            .error
            .as_deref()
            .unwrap()
            .starts_with("Untrusted dotagents source"));
    }

    #[test]
    fn trust_confirm_rejects_mismatch_and_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AddSkillOperationState::default();
        state
            .begin(
                "op-replay".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "kentcdodds/kcd-skills",
                    "visual-recap",
                    AddMethod::Dotagents,
                )),
                None,
            )
            .unwrap();
        let rt = test_runtime(tmp.path());
        run_operation_body(
            None,
            &state,
            "op-replay",
            || Ok(rt),
            &NeverFetch,
            &NeverLookup,
        );

        let lock =
            super::super::write_lease::WriteLease::with_lease_root(tmp.path().join("leases"));
        let mismatch = confirm_add_skill_trust_with(
            tmp.path(),
            "op-replay",
            "op-retry",
            "evil/repo",
            &state,
            &lock,
        )
        .unwrap_err();
        assert_eq!(mismatch, "Trust confirmation does not match this operation");
        assert!(
            super::super::skill_fork_registry::read_fork_registry(tmp.path())
                .unwrap()
                .trusted_dotagents_sources
                .is_empty()
        );

        let (_, queued) = confirm_add_skill_trust_with(
            tmp.path(),
            "op-replay",
            "op-retry",
            "kentcdodds/kcd-skills",
            &state,
            &lock,
        )
        .unwrap();
        assert_eq!(queued.retry_of.as_deref(), Some("op-replay"));

        let replay = confirm_add_skill_trust_with(
            tmp.path(),
            "op-replay",
            "op-retry-2",
            "kentcdodds/kcd-skills",
            &state,
            &lock,
        )
        .unwrap_err();
        assert_eq!(replay, "Trust confirmation does not match this operation");
    }

    #[test]
    fn trust_confirmation_serializes_registry_updates_without_losing_them() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let state = AddSkillOperationState::default();
        state
            .begin(
                "op-concurrent".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "kentcdodds/kcd-skills",
                    "visual-recap",
                    AddMethod::Dotagents,
                )),
                None,
            )
            .unwrap();
        let rt = test_runtime(&home);
        run_operation_body(
            None,
            &state,
            "op-concurrent",
            || Ok(rt),
            &NeverFetch,
            &NeverLookup,
        );

        let lock = Arc::new(super::super::write_lease::WriteLease::with_lease_root(
            tmp.path().join("leases"),
        ));
        let ready = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let writer_home = home.clone();
        let writer_lock = Arc::clone(&lock);
        let writer_ready = Arc::clone(&ready);
        let writer_release = Arc::clone(&release);
        let writer = thread::spawn(move || {
            let _guard = writer_lock.try_acquire(&writer_home).unwrap();
            let mut registry =
                super::super::skill_fork_registry::read_fork_registry(&writer_home).unwrap();
            registry.preferred_editor = Some("Cursor".to_string());
            writer_ready.wait();
            writer_release.wait();
            super::super::skill_fork_registry::write_fork_registry(&writer_home, &registry)
                .unwrap();
        });
        ready.wait();

        let busy = confirm_add_skill_trust_with(
            &home,
            "op-concurrent",
            "op-concurrent-retry",
            "kentcdodds/kcd-skills",
            &state,
            &lock,
        )
        .unwrap_err();
        assert!(
            busy.starts_with("Another write is in progress"),
            "unexpected message: {busy}"
        );
        release.wait();
        writer.join().unwrap();

        confirm_add_skill_trust_with(
            &home,
            "op-concurrent",
            "op-concurrent-retry",
            "kentcdodds/kcd-skills",
            &state,
            &lock,
        )
        .unwrap();
        let registry = super::super::skill_fork_registry::read_fork_registry(&home).unwrap();
        assert_eq!(registry.preferred_editor.as_deref(), Some("Cursor"));
        assert!(registry
            .trusted_dotagents_sources
            .contains("kentcdodds/kcd-skills"));
    }

    /// Cancel is only checked once, before any work starts (see
    /// `run_operation_body`'s own doc comment) - `build_runtime` panicking
    /// if called proves this request never got that far.
    #[test]
    fn cancel_without_mutation_is_cancelled() {
        let state = AddSkillOperationState::default();
        state
            .begin(
                "op-cancel".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "getsentry/skills",
                    "find-bugs",
                    AddMethod::SkillsSh,
                )),
                None,
            )
            .unwrap();
        state.request_cancel("op-cancel").unwrap();
        run_operation_body(
            None,
            &state,
            "op-cancel",
            || -> Result<Runtime, String> { panic!("build_runtime must not run once cancelled") },
            &NeverFetch,
            &NeverLookup,
        );
        assert_eq!(
            state.snapshot("op-cancel").unwrap().phase,
            AddSkillOperationPhase::Cancelled
        );
    }

    struct CountingFetch {
        downloads: StdMutex<usize>,
    }
    struct FakeSnapshot;
    impl RepoSnapshot for FakeSnapshot {
        fn copy_dir(&self, _path: &str, into: &Path) -> Result<(), String> {
            fs::create_dir_all(into).unwrap();
            fs::write(
                into.join("SKILL.md"),
                "---\nname: visual-recap\ndescription: test\n---\nBody.",
            )
            .unwrap();
            Ok(())
        }
    }
    impl UpstreamFetch for CountingFetch {
        fn fetch_skill_dir(&self, _: &str, _: &str, _: &str, _: &Path) -> Result<(), String> {
            panic!("batch copy must use the snapshot");
        }
        fn open_repo(&self, _: &str, _: &str) -> Result<Option<Box<dyn RepoSnapshot>>, String> {
            *self.downloads.lock().unwrap() += 1;
            Ok(Some(Box::new(FakeSnapshot)))
        }
    }

    #[test]
    fn partial_batch_reconciles_verified_and_root_names() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".agents/skills/other")).unwrap();
        let state = AddSkillOperationState::default();
        let request = AddSkillsRequest {
            source: {
                let mut source = github("kentcdodds/kcd-skills", "visual-recap");
                source.skill_name = None;
                source.git_ref = Some("main".to_string());
                source.path = Some("skills".to_string());
                source
            },
            skills: vec![
                crate::skills::github_skill_listing::GithubSkillEntry {
                    name: "other".to_string(),
                    path: "skills/other".to_string(),
                },
                crate::skills::github_skill_listing::GithubSkillEntry {
                    name: "visual-recap".to_string(),
                    path: "skills/visual-recap".to_string(),
                },
            ],
            method: AddMethod::Copy,
            destination: SkillDestination::Universal,
            agents: vec![],
            disabled_harnesses: vec![],
            scope: InstallScope::Global,
            project_path: None,
            trial: false,
        };
        state
            .begin(
                "op-batch".to_string(),
                AddSkillOperationKind::Batch(request),
                None,
            )
            .unwrap();
        let rt = test_runtime(home);
        run_operation_body(
            None,
            &state,
            "op-batch",
            || Ok(rt),
            &CountingFetch {
                downloads: StdMutex::new(0),
            },
            &NeverLookup,
        );
        let status = state.snapshot("op-batch").unwrap();
        assert_eq!(status.phase, AddSkillOperationPhase::Completed);
        let outcomes = status.outcomes.unwrap();
        assert!(outcomes[0]
            .error
            .as_deref()
            .unwrap()
            .contains("already exists"));
        assert!(outcomes[1].result.is_some());
        assert!(home.join(".agents/skills/visual-recap/SKILL.md").exists());
    }

    #[test]
    fn abandoned_needs_trust_record_expires() {
        let state = AddSkillOperationState::default();
        state
            .begin(
                "op-expired-trust".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "owner/repo",
                    "skill",
                    AddMethod::Dotagents,
                )),
                None,
            )
            .unwrap();
        state
            .advance(
                "op-expired-trust",
                AddSkillOperationPhase::NeedsTrust,
                "Needs trust",
                |_| {},
            )
            .unwrap();
        state
            .lock()
            .unwrap()
            .records
            .get_mut("op-expired-trust")
            .unwrap()
            .updated_at = Instant::now()
            .checked_sub(OPERATION_TTL)
            .unwrap()
            .checked_sub(Duration::from_secs(1))
            .unwrap();

        assert!(state
            .snapshot("op-expired-trust")
            .unwrap_err()
            .contains("not found"));
    }

    #[test]
    fn cancelled_needs_trust_record_becomes_terminal_and_evictable() {
        let state = AddSkillOperationState::default();
        state
            .begin(
                "cancel-trust".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "owner/repo",
                    "skill",
                    AddMethod::Dotagents,
                )),
                None,
            )
            .unwrap();
        state
            .advance(
                "cancel-trust",
                AddSkillOperationPhase::NeedsTrust,
                "Needs trust",
                |_| {},
            )
            .unwrap();
        let cancelled = state.request_cancel("cancel-trust").unwrap();
        assert_eq!(cancelled.phase, AddSkillOperationPhase::Cancelled);

        let mut inner = state.lock().unwrap();
        assert!(remove_oldest_terminal(&mut inner));
        assert!(!inner.records.contains_key("cancel-trust"));
    }

    #[test]
    fn capacity_prunes_terminal_record_behind_active_record() {
        let state = AddSkillOperationState::default();
        let kind = || {
            AddSkillOperationKind::Single(single_request("owner/repo", "skill", AddMethod::Copy))
        };
        state
            .begin("active-first".to_string(), kind(), None)
            .unwrap();
        state
            .begin("terminal-old".to_string(), kind(), None)
            .unwrap();
        state
            .advance(
                "terminal-old",
                AddSkillOperationPhase::Completed,
                "Done",
                |_| {},
            )
            .unwrap();
        for index in 0..(MAX_RETAINED_OPERATIONS - 2) {
            state
                .begin(format!("active-{index}"), kind(), None)
                .unwrap();
        }

        state
            .begin("replacement".to_string(), kind(), None)
            .unwrap();
        assert!(state.snapshot("active-first").is_ok());
        assert!(state.snapshot("terminal-old").is_err());
        assert!(state.snapshot("replacement").is_ok());
    }

    #[test]
    fn all_active_capacity_refuses_a_new_start() {
        let state = AddSkillOperationState::default();
        for index in 0..MAX_RETAINED_OPERATIONS {
            state
                .begin(
                    format!("active-cap-{index}"),
                    AddSkillOperationKind::Single(single_request(
                        "owner/repo",
                        "skill",
                        AddMethod::Copy,
                    )),
                    None,
                )
                .unwrap();
        }
        let error = state
            .begin(
                "active-cap-overflow".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "owner/repo",
                    "skill",
                    AddMethod::Copy,
                )),
                None,
            )
            .unwrap_err();
        assert!(error.contains("all 32 operation slots are active"));
        assert_eq!(state.lock().unwrap().records.len(), MAX_RETAINED_OPERATIONS);
    }

    #[test]
    fn needs_trust_retry_succeeds_before_expiry() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AddSkillOperationState::default();
        state
            .begin(
                "trust-parent".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "kentcdodds/kcd-skills",
                    "visual-recap",
                    AddMethod::Dotagents,
                )),
                None,
            )
            .unwrap();
        state
            .advance(
                "trust-parent",
                AddSkillOperationPhase::NeedsTrust,
                "Needs trust",
                |event| {
                    event.untrusted_source = Some(AddSkillUntrustedSource {
                        identity: "kentcdodds/kcd-skills".to_string(),
                    });
                },
            )
            .unwrap();
        let (_, retry) = confirm_add_skill_trust_with(
            tmp.path(),
            "trust-parent",
            "trust-retry",
            "kentcdodds/kcd-skills",
            &state,
            &super::super::write_lease::WriteLease::with_lease_root(tmp.path().join("leases")),
        )
        .unwrap();
        assert_eq!(retry.phase, AddSkillOperationPhase::Queued);
        assert_eq!(retry.retry_of.as_deref(), Some("trust-parent"));
    }
}
