// ============================================================================
// Skills Module - skill_add_operation
// Background Add Skill: start returns after scheduling, events carry an
// operation id and a strictly increasing sequence, and targeted snapshot
// reconciliation runs from the before/after root names plus verified results.
// ============================================================================

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use super::event_store::fingerprint_path;
use super::skill_add::{
    add_skill_with, add_skills_with_progress, dir_entry_names, resolve_fetch_and_lookup,
    shared_skills_dir, CommandRunner, RealCommandRunner,
};
use super::skill_agent_runner::validate_run_id;
use super::skill_deployment::{universal_skills_dir, SkillDestination};
use super::skill_dto::{
    AddSkillOutcome, AddSkillRequest, AddSkillResult, AddSkillsRequest, InstallScope,
};
use super::skill_fork::ForkMutationLock;
use super::skill_fork_registry::AddMethod;
use super::skill_process::{AddOperationControl, DEFAULT_ADD_PROCESS_TIMEOUT};
use super::skill_process::{PROCESS_CANCELLED_MESSAGE, PROCESS_TIMED_OUT_MESSAGE};
use super::skill_refresh::{self, SkillRefreshState};
use super::skill_trust_policy::{
    normalize_confirmation_identity, record_trusted_dotagents_source,
    require_trusted_dotagents_source, DotagentsSourceTrustError,
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
    deadline: Instant,
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
                deadline: Instant::now() + DEFAULT_ADD_PROCESS_TIMEOUT,
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

    fn operation_control(&self, operation_id: &str) -> Result<AddOperationControl, String> {
        let inner = self.lock()?;
        let record = inner
            .records
            .get(operation_id)
            .ok_or_else(|| format!("Add skill operation {operation_id} was not found"))?;
        Ok(AddOperationControl::with_deadline(
            Arc::clone(&record.cancel),
            record.deadline,
        ))
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

fn install_roots_for_single(home: &Path, request: &AddSkillRequest) -> Vec<PathBuf> {
    let project = request.project_path.as_deref().map(Path::new);
    match request.destination {
        SkillDestination::Universal => vec![shared_skills_dir(home, request)],
        SkillDestination::PerHarness => request
            .agents
            .iter()
            .map(|agent| match request.scope {
                InstallScope::Global => agent.global_skills_dir(home),
                InstallScope::Project => {
                    agent.project_skills_dir(project.unwrap_or_else(|| Path::new("")))
                }
            })
            .collect(),
    }
}

fn install_roots_for_batch(home: &Path, request: &AddSkillsRequest) -> Vec<PathBuf> {
    let project = request.project_path.as_deref().map(Path::new);
    match request.destination {
        SkillDestination::Universal => {
            vec![universal_skills_dir(home, request.scope.clone(), project)]
        }
        SkillDestination::PerHarness => request
            .agents
            .iter()
            .map(|agent| match request.scope {
                InstallScope::Global => agent.global_skills_dir(home),
                InstallScope::Project => {
                    agent.project_skills_dir(project.unwrap_or_else(|| Path::new("")))
                }
            })
            .collect(),
    }
}

fn capture_root_entry_fingerprints(roots: &[PathBuf]) -> BTreeMap<PathBuf, String> {
    let mut entries = BTreeMap::new();
    for root in roots {
        for name in dir_entry_names(root) {
            let path = root.join(name);
            entries.insert(path.clone(), fingerprint_path(&path));
        }
    }
    entries
}

fn changed_root_entry_names(
    before: &BTreeMap<PathBuf, String>,
    after: &BTreeMap<PathBuf, String>,
) -> BTreeSet<String> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect()
}

fn names_from_result(result: &AddSkillResult) -> BTreeSet<String> {
    result
        .name
        .split(", ")
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn names_from_outcomes(outcomes: &[AddSkillOutcome]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for outcome in outcomes {
        if let Some(result) = &outcome.result {
            names.extend(names_from_result(result));
        }
        if outcome.result.is_some() {
            names.insert(outcome.name.clone());
        }
    }
    names
}

/// Union of changed root entries and verified result names.
pub fn affected_skill_names(
    before: &BTreeMap<PathBuf, String>,
    after: &BTreeMap<PathBuf, String>,
    verified: &BTreeSet<String>,
) -> Vec<String> {
    let mut names = changed_root_entry_names(before, after);
    names.extend(verified.iter().cloned());
    names.into_iter().collect()
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

fn source_of(kind: &AddSkillOperationKind) -> &super::skill_dto::ParsedSkillSource {
    match kind {
        AddSkillOperationKind::Single(request) => &request.source,
        AddSkillOperationKind::Batch(request) => &request.source,
    }
}

fn roots_of(home: &Path, kind: &AddSkillOperationKind) -> Vec<PathBuf> {
    match kind {
        AddSkillOperationKind::Single(request) => install_roots_for_single(home, request),
        AddSkillOperationKind::Batch(request) => install_roots_for_batch(home, request),
    }
}

fn fetching_phase(kind: &AddSkillOperationKind) -> bool {
    matches!(method_of(kind), AddMethod::Copy)
}

enum AddWork {
    Single(AddSkillResult),
    Batch(Vec<AddSkillOutcome>),
}

struct OperationCommandRunner<'a> {
    inner: &'a dyn CommandRunner,
    control: AddOperationControl,
}

impl CommandRunner for OperationCommandRunner<'_> {
    fn run_npx(&self, args: &[String], cwd: Option<&Path>) -> Result<(), String> {
        self.control.check_message()?;
        self.inner.run_npx(args, cwd)
    }

    fn is_cancelled(&self) -> bool {
        self.control.check().is_err() || self.inner.is_cancelled()
    }

    fn operation_control(&self) -> AddOperationControl {
        self.control.clone()
    }
}

fn terminal_from_interrupt(
    cancel: bool,
    timed_out: bool,
    mutation_completed: bool,
    any_success: bool,
) -> AddSkillOperationPhase {
    if mutation_completed || any_success {
        if any_success {
            AddSkillOperationPhase::Completed
        } else {
            AddSkillOperationPhase::Failed
        }
    } else if timed_out {
        AddSkillOperationPhase::TimedOut
    } else if cancel {
        AddSkillOperationPhase::Cancelled
    } else {
        AddSkillOperationPhase::Failed
    }
}

fn classify_interrupt(error: Option<&str>) -> (bool, bool) {
    let text = error.unwrap_or("");
    (
        text.contains(PROCESS_CANCELLED_MESSAGE),
        text.contains(PROCESS_TIMED_OUT_MESSAGE),
    )
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

fn run_operation_body(
    app: Option<&AppHandle>,
    state: &AddSkillOperationState,
    operation_id: &str,
    home: &Path,
    runner: &dyn CommandRunner,
    fetch: &dyn super::skill_fork::UpstreamFetch,
    lookup: &dyn super::skill_update_check::CommitLookup,
) {
    let Ok(kind) = state.kind(operation_id) else {
        return;
    };
    let Ok(cancel) = state.cancel_flag(operation_id) else {
        return;
    };
    let Ok(control) = state.operation_control(operation_id) else {
        return;
    };
    if let Err(error) = control.check_message() {
        let (_, timed_out) = classify_interrupt(Some(&error));
        let _ = publish(
            app,
            state,
            operation_id,
            if timed_out {
                AddSkillOperationPhase::TimedOut
            } else {
                AddSkillOperationPhase::Cancelled
            },
            error.clone(),
            |event| event.error = Some(error),
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

    if method_of(&kind) == AddMethod::Dotagents {
        match require_trusted_dotagents_source(home, source_of(&kind)) {
            Err(DotagentsSourceTrustError::Untrusted { identity }) => {
                let error = DotagentsSourceTrustError::Untrusted {
                    identity: identity.clone(),
                }
                .to_string();
                let _ = publish(
                    app,
                    state,
                    operation_id,
                    AddSkillOperationPhase::NeedsTrust,
                    UNTRUSTED_DOTAGENTS_SOURCE_MESSAGE,
                    |event| {
                        event.untrusted_source = Some(AddSkillUntrustedSource { identity });
                        event.error = Some(error);
                    },
                );
                return;
            }
            Ok(()) => {}
            Err(error) => {
                let error = error.to_string();
                let _ = publish(
                    app,
                    state,
                    operation_id,
                    AddSkillOperationPhase::Failed,
                    error.clone(),
                    |event| {
                        event.error = Some(error);
                    },
                );
                return;
            }
        }
    }

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

    let roots = roots_of(home, &kind);
    let before = capture_root_entry_fingerprints(&roots);
    let _ = publish(
        app,
        state,
        operation_id,
        AddSkillOperationPhase::Installing,
        "Installing",
        |_| {},
    );

    let operation_runner = OperationCommandRunner {
        inner: runner,
        control,
    };
    let work = match &kind {
        AddSkillOperationKind::Single(request) => {
            add_skill_with(home, request, &operation_runner, fetch, lookup).map(AddWork::Single)
        }
        AddSkillOperationKind::Batch(request) => add_skills_with_progress(
            home,
            request,
            &operation_runner,
            fetch,
            lookup,
            |current, total, name| {
                let _ = publish(
                    app,
                    state,
                    operation_id,
                    AddSkillOperationPhase::Installing,
                    format!("Installing {name} ({current} of {total})"),
                    |event| {
                        event.item = Some(AddSkillItemProgress {
                            current,
                            total,
                            name: name.to_string(),
                        });
                    },
                );
            },
        )
        .map(AddWork::Batch),
    };

    let after = capture_root_entry_fingerprints(&roots);
    let (result, outcomes, error) = match work {
        Ok(AddWork::Single(result)) => (Some(result), None, None),
        Ok(AddWork::Batch(outcomes)) => (None, Some(outcomes), None),
        Err(error) => (None, None, Some(error)),
    };

    let mut verified = BTreeSet::new();
    if let Some(result) = &result {
        verified.extend(names_from_result(result));
    }
    if let Some(outcomes) = &outcomes {
        verified.extend(names_from_outcomes(outcomes));
    }
    let names = affected_skill_names(&before, &after, &verified);
    let mutation_completed = !before.eq(&after) || !verified.is_empty();
    let any_success = result.is_some()
        || outcomes
            .as_ref()
            .is_some_and(|items| items.iter().any(|item| item.result.is_some()));

    let _ = publish(
        app,
        state,
        operation_id,
        AddSkillOperationPhase::Reconciling,
        "Updating skill list",
        |event| {
            event.result = result.clone();
            event.outcomes = outcomes.clone();
            event.error = error.clone();
        },
    );
    if let Err(reconcile_error) = reconcile_affected(app, names, &affected_projects(&kind)) {
        eprintln!(
            "[add_skill_operation] targeted snapshot reconciliation failed: {reconcile_error}"
        );
        if let Some(app) = app {
            skill_refresh::request_snapshot_rebuild(app);
        }
    }

    let outcome_interrupt = outcomes.as_ref().and_then(|items| {
        items
            .iter()
            .filter_map(|item| item.error.as_deref())
            .find(|item_error| {
                let (cancelled, timed_out) = classify_interrupt(Some(item_error));
                cancelled || timed_out
            })
    });
    let interrupt_error = error.as_deref().or(outcome_interrupt);
    let (cancel_hit, timed_out) = classify_interrupt(interrupt_error);
    let cancel_requested = cancel.load(Ordering::SeqCst) || cancel_hit;
    let phase = if error.is_none() && any_success {
        AddSkillOperationPhase::Completed
    } else {
        terminal_from_interrupt(cancel_requested, timed_out, mutation_completed, any_success)
    };
    let partial_error = if mutation_completed && !any_success {
        Some(match (cancel_requested, timed_out, error.as_deref()) {
            (_, true, _) => {
                "Add skill timed out after changing skill files; installation may be partial"
                    .to_string()
            }
            (true, _, _) => {
                "Add skill was cancelled after changing skill files; installation may be partial"
                    .to_string()
            }
            (_, _, Some(error)) => format!(
                "Add skill failed after changing skill files; installation may be partial: {error}"
            ),
            _ => "Add skill failed after changing skill files; installation may be partial"
                .to_string(),
        })
    } else {
        None
    };
    let message = match phase {
        AddSkillOperationPhase::Completed => result
            .as_ref()
            .map(|item| format!("Added {}", item.name))
            .or_else(|| {
                outcomes.as_ref().map(|items| {
                    let count = items.iter().filter(|item| item.result.is_some()).count();
                    format!("Added {count} skill{}", if count == 1 { "" } else { "s" })
                })
            })
            .unwrap_or_else(|| "Added skill".to_string()),
        AddSkillOperationPhase::Cancelled => "Add skill cancelled".to_string(),
        AddSkillOperationPhase::TimedOut => "Add skill timed out".to_string(),
        AddSkillOperationPhase::Failed => partial_error.clone().unwrap_or_else(|| {
            error
                .clone()
                .unwrap_or_else(|| "Add skill failed".to_string())
        }),
        _ => "Add skill failed".to_string(),
    };
    let _ = publish(app, state, operation_id, phase, message, |event| {
        event.result = result;
        event.outcomes = outcomes;
        event.error = partial_error.or(error);
    });
}

fn spawn_operation(app: AppHandle, state: AddSkillOperationState, operation_id: String) {
    tauri::async_runtime::spawn_blocking(move || {
        let home = match dirs::home_dir() {
            Some(home) => home,
            None => {
                let _ = publish(
                    Some(&app),
                    &state,
                    &operation_id,
                    AddSkillOperationPhase::Failed,
                    "Could not find home directory",
                    |event| {
                        event.error = Some("Could not find home directory".to_string());
                    },
                );
                return;
            }
        };
        let control = match state.operation_control(&operation_id) {
            Ok(control) => control,
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
                return;
            }
        };
        let run = || {
            let runner = RealCommandRunner::with_control(control);
            match resolve_fetch_and_lookup(&app) {
                Ok((fetch, lookup)) => run_operation_body(
                    Some(&app),
                    &state,
                    &operation_id,
                    &home,
                    &runner,
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
            }
        };
        if let Some(lock) = app.try_state::<ForkMutationLock>() {
            let _guard = match lock.try_acquire() {
                Ok(guard) => guard,
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
                    return;
                }
            };
            run();
            return;
        }
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
    let queued = state.begin(
        operation_id.clone(),
        AddSkillOperationKind::Single(request),
        None,
    )?;
    emit_status(Some(&app), &queued);
    spawn_operation(app.clone(), state.inner().clone(), operation_id);
    Ok(queued)
}

/// Start a batch Add Skill operation. Returns the queued event immediately.
#[tauri::command]
pub fn start_add_skills_operation(
    operation_id: String,
    request: AddSkillsRequest,
    app: AppHandle,
    state: tauri::State<AddSkillOperationState>,
) -> Result<AddSkillOperationEvent, String> {
    let queued = state.begin(
        operation_id.clone(),
        AddSkillOperationKind::Batch(request),
        None,
    )?;
    emit_status(Some(&app), &queued);
    spawn_operation(app.clone(), state.inner().clone(), operation_id);
    Ok(queued)
}

/// Catch-up read for a listener that subscribed after start, or remounted.
#[tauri::command]
pub fn get_add_skill_operation(
    operation_id: String,
    state: tauri::State<AddSkillOperationState>,
) -> Result<AddSkillOperationEvent, String> {
    state.snapshot(&operation_id)
}

/// Request cancel. If mutation already finished, the worker still reports
/// completed or failed rather than cancelled.
#[tauri::command]
pub fn cancel_add_skill_operation(
    operation_id: String,
    app: AppHandle,
    state: tauri::State<AddSkillOperationState>,
) -> Result<AddSkillOperationEvent, String> {
    let event = state.request_cancel(&operation_id)?;
    emit_status(Some(&app), &event);
    Ok(event)
}

/// Record trust for this operation's repository identity, then retry the
/// same immutable request. Rejects mismatch and replay. `retry_operation_id`
/// must be a fresh frontend-generated id.
fn confirm_add_skill_trust_with(
    home: &Path,
    operation_id: String,
    retry_operation_id: String,
    identity: String,
    state: &AddSkillOperationState,
    fork_lock: &ForkMutationLock,
) -> Result<(AddSkillOperationEvent, AddSkillOperationEvent), String> {
    validate_run_id(&retry_operation_id)
        .map_err(|error| error.replace("Run id", "Operation id"))?;
    let expected = {
        let mut inner = state.lock()?;
        prune_locked(&mut inner, Instant::now());
        if inner.records.contains_key(&retry_operation_id) {
            return Err(format!(
                "Add skill operation {retry_operation_id} already exists"
            ));
        }
        let record = inner
            .records
            .get(&operation_id)
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
    let normalized = normalize_confirmation_identity(&identity)?;
    if normalized != expected {
        return Err("Trust confirmation does not match this operation".to_string());
    }

    // Background add work acquires these locks in this order. Do not hold the
    // operation-state lock while trying to acquire the filesystem lock.
    let _guard = fork_lock.try_acquire()?;
    let (parent_event, queued) = {
        let mut inner = state.lock()?;
        if inner.records.contains_key(&retry_operation_id) {
            return Err(format!(
                "Add skill operation {retry_operation_id} already exists"
            ));
        }
        make_operation_room(&mut inner)?;
        let kind = {
            let parent = inner
                .records
                .get_mut(&operation_id)
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

        record_trusted_dotagents_source(home, &normalized)?;
        let parent_event = {
            let parent = inner
                .records
                .get_mut(&operation_id)
                .expect("parent operation remains present while the operation-state lock is held");
            parent.trust_confirmed = true;
            advance_locked(
                parent,
                AddSkillOperationPhase::NeedsTrust,
                "Trusted repository; retrying",
                |event| {
                    event.retry_of = Some(retry_operation_id.clone());
                },
            );
            parent.event.clone()
        };
        let queued = AddSkillOperationEvent {
            operation_id: retry_operation_id.clone(),
            sequence: 1,
            phase: AddSkillOperationPhase::Queued,
            message: "Waiting to add skill".to_string(),
            item: None,
            result: None,
            outcomes: None,
            error: None,
            untrusted_source: None,
            retry_of: Some(operation_id.clone()),
        };
        inner.records.insert(
            retry_operation_id.clone(),
            AddSkillOperationRecord {
                event: queued.clone(),
                kind,
                cancel: Arc::new(AtomicBool::new(false)),
                updated_at: Instant::now(),
                deadline: Instant::now() + DEFAULT_ADD_PROCESS_TIMEOUT,
                trust_confirmed: false,
            },
        );
        inner.order.push_back(retry_operation_id.clone());
        (parent_event, queued)
    };
    Ok((parent_event, queued))
}

#[tauri::command]
pub fn confirm_add_skill_trust(
    operation_id: String,
    retry_operation_id: String,
    identity: String,
    app: AppHandle,
    state: tauri::State<AddSkillOperationState>,
    fork_lock: tauri::State<ForkMutationLock>,
) -> Result<AddSkillOperationEvent, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let (parent_event, queued) = confirm_add_skill_trust_with(
        &home,
        operation_id,
        retry_operation_id.clone(),
        identity,
        state.inner(),
        fork_lock.inner(),
    )?;
    emit_status(Some(&app), &parent_event);
    emit_status(Some(&app), &queued);
    spawn_operation(app.clone(), state.inner().clone(), retry_operation_id);
    Ok(queued)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::skill_add::CommandRunner;
    use crate::skills::skill_dto::{ParsedSkillSource, ParsedSkillSourceKind};
    use crate::skills::skill_fork::{RepoSnapshot, UpstreamFetch};
    use crate::skills::skill_update_check::CommitLookup;
    use std::fs;
    use std::sync::{Barrier, Mutex as StdMutex};
    use std::thread;
    use std::time::Duration;

    struct BlockingRunner {
        gate: Arc<(StdMutex<bool>, std::sync::Condvar)>,
        cancel: Arc<AtomicBool>,
        calls: StdMutex<usize>,
    }

    impl CommandRunner for BlockingRunner {
        fn run_npx(&self, _args: &[String], _cwd: Option<&Path>) -> Result<(), String> {
            *self.calls.lock().unwrap() += 1;
            let (lock, cond) = &*self.gate;
            let mut ready = lock.lock().unwrap();
            while !*ready {
                ready = cond.wait(ready).unwrap();
            }
            if self.cancel.load(Ordering::SeqCst) {
                return Err(PROCESS_CANCELLED_MESSAGE.to_string());
            }
            Ok(())
        }

        fn is_cancelled(&self) -> bool {
            self.cancel.load(Ordering::SeqCst)
        }
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
    fn start_returns_before_blocked_runner_finishes() {
        let state = AddSkillOperationState::default();
        let gate = Arc::new((StdMutex::new(false), std::sync::Condvar::new()));
        let cancel = Arc::new(AtomicBool::new(false));
        let runner = BlockingRunner {
            gate: Arc::clone(&gate),
            cancel: Arc::clone(&cancel),
            calls: StdMutex::new(0),
        };
        let queued = state
            .begin(
                "op-block".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "getsentry/skills",
                    "find-bugs",
                    AddMethod::SkillsSh,
                )),
                None,
            )
            .unwrap();
        assert_eq!(queued.phase, AddSkillOperationPhase::Queued);
        assert_eq!(queued.sequence, 1);

        let state_worker = state.clone();
        let handle = thread::spawn(move || {
            let tmp = tempfile::tempdir().unwrap();
            run_operation_body(
                None,
                &state_worker,
                "op-block",
                tmp.path(),
                &runner,
                &NeverFetch,
                &NeverLookup,
            );
        });

        thread::sleep(Duration::from_millis(30));
        let status = state.snapshot("op-block").unwrap();
        assert!(
            status.sequence >= 1,
            "status should be readable while work is blocked"
        );
        assert!(!status.phase.is_terminal());

        {
            let (lock, cond) = &*gate;
            *lock.lock().unwrap() = true;
            cond.notify_all();
        }
        handle.join().unwrap();
        let done = state.snapshot("op-block").unwrap();
        assert!(done.sequence > status.sequence);
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
        struct PanicRunner;
        impl CommandRunner for PanicRunner {
            fn run_npx(&self, _: &[String], _: Option<&Path>) -> Result<(), String> {
                panic!("untrusted source must not run npx");
            }
        }
        run_operation_body(
            None,
            &state,
            "op-trust",
            tmp.path(),
            &PanicRunner,
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
        struct PanicRunner;
        impl CommandRunner for PanicRunner {
            fn run_npx(&self, _: &[String], _: Option<&Path>) -> Result<(), String> {
                panic!("untrusted source must not run npx");
            }
        }
        run_operation_body(
            None,
            &state,
            "op-replay",
            tmp.path(),
            &PanicRunner,
            &NeverFetch,
            &NeverLookup,
        );

        let lock = ForkMutationLock::default();
        let mismatch = confirm_add_skill_trust_with(
            tmp.path(),
            "op-replay".to_string(),
            "op-retry".to_string(),
            "evil/repo".to_string(),
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
            "op-replay".to_string(),
            "op-retry".to_string(),
            "kentcdodds/kcd-skills".to_string(),
            &state,
            &lock,
        )
        .unwrap();
        assert_eq!(queued.retry_of.as_deref(), Some("op-replay"));

        let replay = confirm_add_skill_trust_with(
            tmp.path(),
            "op-replay".to_string(),
            "op-retry-2".to_string(),
            "kentcdodds/kcd-skills".to_string(),
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
        struct PanicRunner;
        impl CommandRunner for PanicRunner {
            fn run_npx(&self, _: &[String], _: Option<&Path>) -> Result<(), String> {
                panic!("untrusted source must not run npx");
            }
        }
        run_operation_body(
            None,
            &state,
            "op-concurrent",
            &home,
            &PanicRunner,
            &NeverFetch,
            &NeverLookup,
        );

        let lock = Arc::new(ForkMutationLock::default());
        let ready = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let writer_home = home.clone();
        let writer_lock = Arc::clone(&lock);
        let writer_ready = Arc::clone(&ready);
        let writer_release = Arc::clone(&release);
        let writer = thread::spawn(move || {
            let _guard = writer_lock.try_acquire().unwrap();
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
            "op-concurrent".to_string(),
            "op-concurrent-retry".to_string(),
            "kentcdodds/kcd-skills".to_string(),
            &state,
            &lock,
        )
        .unwrap_err();
        assert_eq!(busy, "Another fork operation is in progress");
        release.wait();
        writer.join().unwrap();

        confirm_add_skill_trust_with(
            &home,
            "op-concurrent".to_string(),
            "op-concurrent-retry".to_string(),
            "kentcdodds/kcd-skills".to_string(),
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

    #[test]
    fn trusted_retry_uses_the_same_request() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(home.join(".agents/skills")).unwrap();
        record_trusted_dotagents_source(home, "kentcdodds/kcd-skills").unwrap();
        let state = AddSkillOperationState::default();
        let request = single_request(
            "kentcdodds/kcd-skills",
            "visual-recap",
            AddMethod::Dotagents,
        );
        state
            .begin(
                "op-retry".to_string(),
                AddSkillOperationKind::Single(request),
                Some("op-trust".to_string()),
            )
            .unwrap();
        struct CreateRunner {
            home: PathBuf,
        }
        impl CommandRunner for CreateRunner {
            fn run_npx(&self, _: &[String], _: Option<&Path>) -> Result<(), String> {
                fs::create_dir_all(self.home.join(".agents/skills/visual-recap")).unwrap();
                Ok(())
            }
        }
        run_operation_body(
            None,
            &state,
            "op-retry",
            home,
            &CreateRunner {
                home: home.to_path_buf(),
            },
            &NeverFetch,
            &NeverLookup,
        );
        let status = state.snapshot("op-retry").unwrap();
        assert_eq!(status.phase, AddSkillOperationPhase::Completed);
        assert_eq!(status.retry_of.as_deref(), Some("op-trust"));
        assert_eq!(status.result.as_ref().unwrap().name, "visual-recap");
    }

    #[test]
    fn partial_batch_collects_affected_names() {
        let before = BTreeMap::from([(PathBuf::from("/root/other"), "same".to_string())]);
        let after = BTreeMap::from([
            (PathBuf::from("/root/other"), "same".to_string()),
            (PathBuf::from("/root/visual-recap"), "new".to_string()),
        ]);
        let verified = BTreeSet::from(["visual-recap".to_string()]);
        let names = affected_skill_names(&before, &after, &verified);
        assert_eq!(names, vec!["visual-recap".to_string()]);
    }

    #[test]
    fn cancel_without_mutation_is_cancelled() {
        let tmp = tempfile::tempdir().unwrap();
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
        struct CancelRunner;
        impl CommandRunner for CancelRunner {
            fn run_npx(&self, _: &[String], _: Option<&Path>) -> Result<(), String> {
                Err(PROCESS_CANCELLED_MESSAGE.to_string())
            }
            fn is_cancelled(&self) -> bool {
                true
            }
        }
        run_operation_body(
            None,
            &state,
            "op-cancel",
            tmp.path(),
            &CancelRunner,
            &NeverFetch,
            &NeverLookup,
        );
        assert_eq!(
            state.snapshot("op-cancel").unwrap().phase,
            AddSkillOperationPhase::Cancelled
        );
    }

    #[test]
    fn timeout_without_mutation_is_timed_out() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AddSkillOperationState::default();
        state
            .begin(
                "op-timeout".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "getsentry/skills",
                    "find-bugs",
                    AddMethod::SkillsSh,
                )),
                None,
            )
            .unwrap();
        struct TimeoutRunner;
        impl CommandRunner for TimeoutRunner {
            fn run_npx(&self, _: &[String], _: Option<&Path>) -> Result<(), String> {
                Err(PROCESS_TIMED_OUT_MESSAGE.to_string())
            }
        }
        run_operation_body(
            None,
            &state,
            "op-timeout",
            tmp.path(),
            &TimeoutRunner,
            &NeverFetch,
            &NeverLookup,
        );
        assert_eq!(
            state.snapshot("op-timeout").unwrap().phase,
            AddSkillOperationPhase::TimedOut
        );
    }

    #[test]
    fn stored_operation_deadline_stops_copy_before_lookup_or_mutation() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AddSkillOperationState::default();
        state
            .begin(
                "op-stored-timeout".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "owner/repo",
                    "timed-out-copy",
                    AddMethod::Copy,
                )),
                None,
            )
            .unwrap();
        state
            .lock()
            .unwrap()
            .records
            .get_mut("op-stored-timeout")
            .unwrap()
            .deadline = Instant::now() - Duration::from_millis(1);

        run_operation_body(
            None,
            &state,
            "op-stored-timeout",
            tmp.path(),
            &BlockingRunner {
                gate: Arc::new((StdMutex::new(true), std::sync::Condvar::new())),
                cancel: Arc::new(AtomicBool::new(false)),
                calls: StdMutex::new(0),
            },
            &NeverFetch,
            &NeverLookup,
        );
        assert_eq!(
            state.snapshot("op-stored-timeout").unwrap().phase,
            AddSkillOperationPhase::TimedOut
        );
        assert!(!tmp.path().join(".agents/skills/timed-out-copy").exists());
    }

    #[test]
    fn cancellation_after_committed_install_is_reported_completed() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let state = AddSkillOperationState::default();
        state
            .begin(
                "op-mut".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "getsentry/skills",
                    "find-bugs",
                    AddMethod::SkillsSh,
                )),
                None,
            )
            .unwrap();
        struct InstallThenCancel {
            home: PathBuf,
            cancel: Arc<AtomicBool>,
        }
        impl CommandRunner for InstallThenCancel {
            fn run_npx(&self, _: &[String], _: Option<&Path>) -> Result<(), String> {
                fs::create_dir_all(self.home.join(".agents/skills/find-bugs")).unwrap();
                self.cancel.store(true, Ordering::SeqCst);
                Ok(())
            }
        }
        let cancel = state.cancel_flag("op-mut").unwrap();
        run_operation_body(
            None,
            &state,
            "op-mut",
            home,
            &InstallThenCancel {
                home: home.to_path_buf(),
                cancel,
            },
            &NeverFetch,
            &NeverLookup,
        );
        assert_eq!(
            state.snapshot("op-mut").unwrap().phase,
            AddSkillOperationPhase::Completed
        );
    }

    #[test]
    fn timeout_after_in_place_change_reports_partial_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let skill_md = home.join(".agents/skills/find-bugs/SKILL.md");
        fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        fs::write(&skill_md, "before").unwrap();
        let state = AddSkillOperationState::default();
        state
            .begin(
                "op-in-place-timeout".to_string(),
                AddSkillOperationKind::Single(single_request(
                    "getsentry/skills",
                    "find-bugs",
                    AddMethod::SkillsSh,
                )),
                None,
            )
            .unwrap();
        struct ChangeThenTimeout(PathBuf);
        impl CommandRunner for ChangeThenTimeout {
            fn run_npx(&self, _: &[String], _: Option<&Path>) -> Result<(), String> {
                fs::write(&self.0, "after").unwrap();
                Err(PROCESS_TIMED_OUT_MESSAGE.to_string())
            }
        }

        run_operation_body(
            None,
            &state,
            "op-in-place-timeout",
            home,
            &ChangeThenTimeout(skill_md),
            &NeverFetch,
            &NeverLookup,
        );

        let status = state.snapshot("op-in-place-timeout").unwrap();
        assert_eq!(status.phase, AddSkillOperationPhase::Failed);
        assert!(status
            .error
            .unwrap()
            .contains("installation may be partial"));
    }

    struct CountingFetch {
        downloads: StdMutex<usize>,
    }
    struct FakeSnapshot;
    impl RepoSnapshot for FakeSnapshot {
        fn copy_dir(&self, _path: &str, into: &Path) -> Result<(), String> {
            fs::create_dir_all(into).unwrap();
            fs::write(into.join("SKILL.md"), "body").unwrap();
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
        struct NoNpx;
        impl CommandRunner for NoNpx {
            fn run_npx(&self, _: &[String], _: Option<&Path>) -> Result<(), String> {
                panic!("copy must not run npx");
            }
        }
        run_operation_body(
            None,
            &state,
            "op-batch",
            home,
            &NoNpx,
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
    fn cancel_during_fake_fetch_cleans_stage_without_deployment_or_registry_mutation() {
        struct BlockingFetch {
            entered: Arc<Barrier>,
        }
        impl UpstreamFetch for BlockingFetch {
            fn fetch_skill_dir(&self, _: &str, _: &str, _: &str, _: &Path) -> Result<(), String> {
                panic!("controlled fetch must be used")
            }

            fn fetch_skill_dir_controlled(
                &self,
                _: &str,
                _: &str,
                _: &str,
                into: &Path,
                control: &AddOperationControl,
            ) -> Result<(), String> {
                fs::create_dir_all(into).unwrap();
                fs::write(into.join("partial"), "partial").unwrap();
                self.entered.wait();
                loop {
                    control.check_message()?;
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }
        struct NoNpx;
        impl CommandRunner for NoNpx {
            fn run_npx(&self, _: &[String], _: Option<&Path>) -> Result<(), String> {
                panic!("Copy must not run npx")
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let state = AddSkillOperationState::default();
        let mut request = single_request("owner/repo", "cancel-fetch", AddMethod::Copy);
        request.source.git_ref = Some("abc123".to_string());
        state
            .begin(
                "op-cancel-fetch".to_string(),
                AddSkillOperationKind::Single(request),
                None,
            )
            .unwrap();
        let entered = Arc::new(Barrier::new(2));
        let worker_state = state.clone();
        let worker_entered = Arc::clone(&entered);
        let worker_home = home.clone();
        let worker = thread::spawn(move || {
            run_operation_body(
                None,
                &worker_state,
                "op-cancel-fetch",
                &worker_home,
                &NoNpx,
                &BlockingFetch {
                    entered: worker_entered,
                },
                &NeverLookup,
            );
        });

        entered.wait();
        assert!(!home.join(".agents/skills/cancel-fetch").exists());
        assert!(super::super::skill_fork_registry::read_fork_registry(&home)
            .unwrap()
            .copies
            .is_empty());
        state.request_cancel("op-cancel-fetch").unwrap();
        worker.join().unwrap();

        let status = state.snapshot("op-cancel-fetch").unwrap();
        assert_eq!(status.phase, AddSkillOperationPhase::Cancelled);
        let root = home.join(".agents/skills");
        assert!(!root.join("cancel-fetch").exists());
        assert!(fs::read_dir(root).unwrap().next().is_none());
        assert!(super::super::skill_fork_registry::read_fork_registry(&home)
            .unwrap()
            .copies
            .is_empty());
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
            .updated_at = Instant::now() - OPERATION_TTL - Duration::from_secs(1);

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
            "trust-parent".to_string(),
            "trust-retry".to_string(),
            "kentcdodds/kcd-skills".to_string(),
            &state,
            &ForkMutationLock::default(),
        )
        .unwrap();
        assert_eq!(retry.phase, AddSkillOperationPhase::Queued);
        assert_eq!(retry.retry_of.as_deref(), Some("trust-parent"));
    }
}
