//! Durable Park and Unpark for one Global Universal deployment.
//!
//! The persisted intent contains the exact tree, reader link, selected raw
//! registry records, and deterministic destinations. Recovery changes state
//! only when every observed input is one of those recorded states.

use crate::{
    skill_backup_copy::{inspect_entry, BackupCopyReport},
    skill_backup_reservation::BackupCopyLimits,
    skill_backup_source::{BackupSource, BackupSourceRoot},
    skill_coordination::{CancellationToken, FinalizedWriteLease},
    skill_deployment::{deployment_id, parse_deployment_id, BackingRelationship, SkillDestination},
    skill_event::{EventDraft, EventRow, EventStatus},
    skill_event_operations::{EventWriteFailure, GuardedEventStore},
    skill_event_store::EventStore,
    skill_fork_registry::{
        deployment_trial_key, trial_key, ForkRegistry, ParkedOwnerRecord, ParkedRecord,
        RegistryOwnerRecord, TrialRecord, TrialScope, TrialStatus,
    },
    skill_inventory::Deployment,
    skill_ownership::LifecycleOwnerKind,
    skill_park_transition::{ParkRegistryState, ParkRegistryTransition, TrialTransition},
    skill_provenance::SourceKind,
    skill_scope::SkillReadScope,
    skill_service::{InventoryRead, ScopedSkillService},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    path::{Path, PathBuf},
    time::Duration,
};

pub const PARK_EVENT_KIND: &str = "park_global_universal";
pub const UNPARK_EVENT_KIND: &str = "unpark_global_universal";
const REGISTRY_LIMIT: usize = 8 * 1024 * 1024;
const PARKED_ROOT: &str = "skills-parked";
const PARKED_LINK_ROOT: &str = "skills-parked-links";
const RETAINED_LINK_ROOT: &str = "skills-parked-links-retained";
const TRASH_ROOT: &str = "skills-trash";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParkSkillRequest {
    pub deployment_id: String,
    pub source_kind: SourceKind,
    pub parked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnparkSkillRequest {
    pub deployment_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum UnparkOutcome {
    Restored,
    Reconciled,
    ConflictTrashed { trash_path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum OperationKind {
    Park,
    UnparkRestored,
    UnparkReconciled { trash_path: PathBuf },
    UnparkConflict { trash_path: PathBuf },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParkIntent {
    version: u32,
    operation_id: String,
    operation: OperationKind,
    name: String,
    active_path: PathBuf,
    parked_path: PathBuf,
    reader_path: PathBuf,
    staged_reader_path: PathBuf,
    #[serde(default)]
    retained_reader_path: PathBuf,
    registry_path: PathBuf,
    expected_tree: String,
    live_tree: Option<String>,
    reader_target: Option<PathBuf>,
    #[serde(default)]
    reader_root_target: Option<PathBuf>,
    reader_was_staged: bool,
    #[serde(default)]
    retained_reader_required: bool,
    parked_record: ParkedRecord,
    registry: ParkRegistryTransition,
}

impl ParkIntent {
    fn event_kind(&self) -> &'static str {
        match &self.operation {
            OperationKind::Park => PARK_EVENT_KIND,
            OperationKind::UnparkRestored
            | OperationKind::UnparkReconciled { .. }
            | OperationKind::UnparkConflict { .. } => UNPARK_EVENT_KIND,
        }
    }

    fn trash_path(&self) -> Option<&Path> {
        match &self.operation {
            OperationKind::UnparkReconciled { trash_path }
            | OperationKind::UnparkConflict { trash_path } => Some(trash_path),
            OperationKind::Park | OperationKind::UnparkRestored => None,
        }
    }

    fn validate(&self, home: &Path) -> Result<(), String> {
        validate_name(&self.name)?;
        if self.version != 1
            || self.operation_id.is_empty()
            || self.registry.name() != self.name
            || self.expected_tree.is_empty()
            || self.active_path != home.join(".agents/skills").join(&self.name)
            || self.parked_path != home.join(".agents").join(PARKED_ROOT).join(&self.name)
            || self.reader_path != home.join(".claude/skills").join(&self.name)
            || self.staged_reader_path
                != home.join(".agents").join(PARKED_LINK_ROOT).join(&self.name)
            || self.retained_reader_path
                != home
                    .join(".agents")
                    .join(RETAINED_LINK_ROOT)
                    .join(format!("{}-{}", self.name, self.operation_id))
            || self.registry_path != home.join(".agents/skill-studio.json")
            || self.parked_record.skill_dir != self.parked_path
            || self.parked_record.claude_link != self.reader_target
        {
            return Err("Park intent paths or evidence are invalid".into());
        }
        if let Some(target) = &self.reader_root_target {
            let root = self
                .reader_path
                .parent()
                .ok_or("Park reader root is missing")?;
            if matches!(&self.operation, OperationKind::Park)
                || !is_exact_reader_root_link(root, target, &home.join(".agents/skills"))?
            {
                return Err("Park intent reader root evidence is invalid".into());
            }
        }
        if self.retained_reader_required
            && (matches!(&self.operation, OperationKind::Park)
                || !self.reader_was_staged
                || self.reader_target.is_none())
        {
            return Err("Park intent retained reader evidence is invalid".into());
        }
        match &self.operation {
            OperationKind::Park | OperationKind::UnparkRestored => {
                if self.live_tree.is_some() || self.trash_path().is_some() {
                    return Err("Park intent has unexpected live or trash evidence".into());
                }
            }
            OperationKind::UnparkReconciled { trash_path }
            | OperationKind::UnparkConflict { trash_path } => {
                if self.live_tree.as_deref().is_none()
                    || trash_path
                        != &home
                            .join(".agents")
                            .join(TRASH_ROOT)
                            .join(format!("{}-{}", self.name, self.operation_id))
                {
                    return Err("Unpark reconciliation evidence is invalid".into());
                }
            }
        }
        Ok(())
    }

    fn draft(&self) -> Result<EventDraft, String> {
        Ok(EventDraft {
            kind: self.event_kind().into(),
            skill: self.name.clone(),
            harness: None,
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::to_value(self).map_err(|error| error.to_string())?,
            inverse: None,
            backup_dir: None,
            restorable: false,
        })
    }

    fn from_event(home: &Path, row: &EventRow) -> Result<Self, String> {
        if !matches!(row.kind.as_str(), PARK_EVENT_KIND | UNPARK_EVENT_KIND)
            || !matches!(row.status.as_str(), "pending" | "interrupted")
            || row.inverse.is_some()
            || row.backup_dir.is_some()
            || row.restorable
            || row.reverted_by.is_some()
            || row.scope.as_deref() != Some("global")
        {
            return Err("Park recovery event is not an unresolved non-restorable intent".into());
        }
        let mut intent: Self = serde_json::from_value(row.payload.clone())
            .map_err(|error| format!("Park recovery intent is incomplete: {error}"))?;
        if intent.retained_reader_path.as_os_str().is_empty() {
            intent.retained_reader_path = home
                .join(".agents")
                .join(RETAINED_LINK_ROOT)
                .join(format!("{}-{}", intent.name, intent.operation_id));
        }
        intent.validate(home)?;
        if row.id != intent.operation_id
            || row.kind != intent.event_kind()
            || row.skill != intent.name
        {
            return Err("Park recovery event does not match its exact intent".into());
        }
        Ok(intent)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EntryState {
    Absent,
    Tree(String),
    Symlink(PathBuf),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilesystemPhase {
    Before,
    ReaderMoved,
    TreeMoved,
    Complete,
}

pub fn park_skill(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &ParkSkillRequest,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<ParkedRecord, String> {
    let operation_id = crate::skill_event_store::allocate_id();
    let (intent, lease) = prepare_park(
        service,
        store,
        request,
        &operation_id,
        limits,
        timeout,
        cancellation,
    )?;
    record_intent(store, &lease, &intent)?;
    drop(lease);
    settle(service, store, &intent, limits, timeout)
        .map_err(|error| format!("Park needs recovery from event {operation_id}: {error}"))?;
    Ok(intent.parked_record)
}

pub fn unpark_skill(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &UnparkSkillRequest,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<UnparkOutcome, String> {
    let operation_id = crate::skill_event_store::allocate_id();
    let (intent, lease) = prepare_unpark(
        service,
        store,
        request,
        &operation_id,
        limits,
        timeout,
        cancellation,
    )?;
    record_intent(store, &lease, &intent)?;
    drop(lease);
    settle(service, store, &intent, limits, timeout)
        .map_err(|error| format!("Unpark needs recovery from event {operation_id}: {error}"))?;
    Ok(match intent.operation {
        OperationKind::UnparkRestored => UnparkOutcome::Restored,
        OperationKind::UnparkReconciled { .. } => UnparkOutcome::Reconciled,
        OperationKind::UnparkConflict { trash_path } => UnparkOutcome::ConflictTrashed {
            trash_path: trash_path.to_string_lossy().into_owned(),
        },
        OperationKind::Park => return Err("Unpark produced a Park outcome".into()),
    })
}

pub fn recover_park_operation(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<(), String> {
    let intent = ParkIntent::from_event(&service.scope().home, row)?;
    settle(service, store, &intent, limits, timeout)
}

fn record_intent(
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    intent: &ParkIntent,
) -> Result<(), String> {
    let guarded = GuardedEventStore::bind(store, lease)?;
    guarded
        .record_pending(lease, &intent.operation_id, intent.draft()?)
        .map_err(|failure| match failure {
            EventWriteFailure::CancelledBeforeWrite | EventWriteFailure::BeforeWrite(_) => {
                failure.to_string()
            }
            EventWriteFailure::MayHaveWritten(_) => format!(
                "Park event {} may have been recorded and must be recovered: {failure}",
                intent.operation_id
            ),
        })
}

fn prepare_park<'scope>(
    service: &'scope mut ScopedSkillService,
    store: &EventStore,
    request: &ParkSkillRequest,
    operation_id: &str,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<(ParkIntent, FinalizedWriteLease<'scope>), String> {
    let selected =
        parse_deployment_id(&request.deployment_id).ok_or("Park needs a valid deployment ID")?;
    let home = service.scope().home;
    let name = selected.name;
    validate_name(&name)?;
    let paths = OperationPaths::new(&home, &name, None, None);
    if selected.lexical_path != paths.active {
        return Err("Park is limited to the selected Global Universal directory".into());
    }
    let (inventory, lease) =
        prepare_lease(service, store, &name, &paths, timeout, cancellation.clone())?;
    let deployment = exact_deployment(&inventory, &request.deployment_id)?;
    validate_live_park_deployment(deployment, &paths.active)?;
    let active = inspect_tree(&paths.active, limits, &cancellation)?
        .ok_or("Selected Global Universal directory is missing")?;
    if inspect_entry_state(&paths.parked, limits, &cancellation)? != EntryState::Absent {
        return Err("Park destination is occupied".into());
    }
    let (reader_target, reader_admitted) = admitted_reader(&paths.reader, &paths.active)?;
    if inspect_link(&paths.staged_reader)? != EntryState::Absent {
        return Err("Park reader stage is occupied".into());
    }
    let registry_original = lease
        .read_ownership_registry(&paths.registry, REGISTRY_LIMIT)
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| b"{}".to_vec());
    let (registry, document) = parse_registry(&registry_original)?;
    if registry.parked.contains_key(&name) {
        return Err("Skill is already parked".into());
    }
    let owner = suspended_owner(deployment, &registry, &document, &paths.active)?;
    let parked_id = deployment_id(
        &name,
        "parked",
        SkillDestination::Universal,
        "universal",
        None,
        &paths.parked,
    );
    let record = ParkedRecord {
        deployment_id: parked_id.clone(),
        skill_dir: paths.parked.clone(),
        parked_at: request.parked_at.clone(),
        source_kind: request.source_kind,
        claude_link: reader_target.clone(),
        suspended_owner: owner.as_ref().map(|owner| owner.record.clone()),
    };
    let trial = selected_trial_transition(
        &registry,
        &document,
        &name,
        &paths.active,
        &request.deployment_id,
        &paths.parked,
        &parked_id,
        None,
        None,
    )?;
    let mut transition = ParkRegistryTransition::park(
        name.clone(),
        serde_json::to_value(&record).map_err(|error| error.to_string())?,
        trial,
    )?;
    if let Some(owner) = owner {
        transition = transition.with_owner(
            owner.section,
            owner.key.clone(),
            Some(owner.value),
            owner.key,
            None,
        )?;
    }
    transition.apply(&registry_original)?;
    let retained_reader = home
        .join(".agents")
        .join(RETAINED_LINK_ROOT)
        .join(format!("{name}-{operation_id}"));
    let intent = ParkIntent {
        version: 1,
        operation_id: operation_id.into(),
        operation: OperationKind::Park,
        name,
        active_path: paths.active,
        parked_path: paths.parked,
        reader_path: paths.reader,
        staged_reader_path: paths.staged_reader,
        retained_reader_path: retained_reader,
        registry_path: paths.registry,
        expected_tree: active.tree_identity,
        live_tree: None,
        reader_target,
        reader_root_target: None,
        reader_was_staged: reader_admitted,
        retained_reader_required: false,
        parked_record: record,
        registry: transition,
    };
    intent.validate(&home)?;
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok((intent, lease))
}

fn prepare_unpark<'scope>(
    service: &'scope mut ScopedSkillService,
    store: &EventStore,
    request: &UnparkSkillRequest,
    operation_id: &str,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<(ParkIntent, FinalizedWriteLease<'scope>), String> {
    let selected =
        parse_deployment_id(&request.deployment_id).ok_or("Unpark needs a valid deployment ID")?;
    let home = service.scope().home;
    let name = selected.name;
    validate_name(&name)?;
    let trash = home
        .join(".agents")
        .join(TRASH_ROOT)
        .join(format!("{name}-{operation_id}"));
    let retained_reader = home
        .join(".agents")
        .join(RETAINED_LINK_ROOT)
        .join(format!("{name}-{operation_id}"));
    let paths = OperationPaths::new(
        &home,
        &name,
        Some(trash.clone()),
        Some(retained_reader.clone()),
    );
    if selected.lexical_path != paths.parked {
        return Err("Unpark is limited to the selected parked directory".into());
    }
    let (inventory, lease) =
        prepare_lease(service, store, &name, &paths, timeout, cancellation.clone())?;
    let parked_deployment = exact_deployment(&inventory, &request.deployment_id)?;
    if parked_deployment.scope != "parked"
        || Path::new(&parked_deployment.path) != paths.parked
        || parked_deployment.is_symlink
    {
        return Err("Selected Unpark deployment changed".into());
    }
    let parked_tree =
        inspect_tree(&paths.parked, limits, &cancellation)?.ok_or("Parked directory is missing")?;
    if inspect_entry_state(&trash, limits, &cancellation)? != EntryState::Absent {
        return Err("Unpark trash destination is occupied".into());
    }
    if inspect_link(&retained_reader)? != EntryState::Absent {
        return Err("Unpark retained reader destination is occupied".into());
    }
    let registry_original = lease
        .read_ownership_registry(&paths.registry, REGISTRY_LIMIT)
        .map_err(|error| error.to_string())?
        .ok_or("Parked registry is missing")?;
    let (registry, document) = parse_registry(&registry_original)?;
    let parked_raw = section_value(&document, "parked", &name)
        .ok_or("Selected parked registry record is missing")?
        .clone();
    let mut record = registry
        .parked
        .get(&name)
        .cloned()
        .ok_or("Selected parked registry record is missing")?;
    if (!record.skill_dir.as_os_str().is_empty() && record.skill_dir != paths.parked)
        || (!record.deployment_id.is_empty() && record.deployment_id != request.deployment_id)
    {
        return Err("Selected parked registry record changed".into());
    }
    record.skill_dir = paths.parked.clone();
    let stage_state = inspect_link(&paths.staged_reader)?;
    let reader_target = record.claude_link.clone();
    let reader_root_target = admitted_reader_root(&paths.reader, &paths.active)?;
    let reader_was_staged = if reader_root_target.is_some() {
        match (&reader_target, &stage_state) {
            (Some(expected), EntryState::Symlink(actual)) if expected == actual => true,
            (_, EntryState::Absent) => false,
            _ => return Err("Staged Park reader link changed".into()),
        }
    } else {
        let reader_state = inspect_link(&paths.reader)?;
        if let Some(target) = &reader_target {
            let reader_matches = match &reader_state {
                EntryState::Absent => true,
                EntryState::Symlink(actual) => actual == target,
                EntryState::Tree(_) | EntryState::Other => false,
            };
            if !reader_matches {
                return Err("Claude reader path is occupied".into());
            }
        }
        match (&reader_target, &stage_state) {
            (Some(expected), EntryState::Symlink(actual)) if expected == actual => true,
            (Some(_), EntryState::Absent) | (None, EntryState::Absent) => false,
            _ => return Err("Staged Park reader link changed".into()),
        }
    };
    let reader_is_live = match &reader_target {
        Some(target) => inspect_link(&paths.reader)? == EntryState::Symlink(target.clone()),
        None => false,
    };
    let retained_reader_required = reader_was_staged
        && reader_target.is_some()
        && (reader_root_target.is_some() || reader_is_live);
    let live = inspect_tree(&paths.active, limits, &cancellation)?;
    let operation = match &live {
        None => OperationKind::UnparkRestored,
        Some(report) if report.tree_identity == parked_tree.tree_identity => {
            OperationKind::UnparkReconciled {
                trash_path: trash.clone(),
            }
        }
        Some(_) => OperationKind::UnparkConflict {
            trash_path: trash.clone(),
        },
    };
    let live_deployment = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| {
            deployment.scope == "global"
                && deployment.destination == SkillDestination::Universal
                && matches!(deployment.backing, BackingRelationship::Canonical)
                && Path::new(&deployment.path) == paths.active
        })
        .collect::<Vec<_>>();
    if live_deployment.len() > 1 {
        return Err("Unpark live deployment is ambiguous".into());
    }
    let live_owners = live_registry_owners(&registry, &document, &name, &paths.active)?;
    if live_owners.len() > 1 {
        return Err("Unpark found competing live ownership".into());
    }
    if live.is_none() && !live_owners.is_empty() {
        return Err("Unpark found stale ownership for a missing live directory".into());
    }
    if let (Some(deployment), Some(owner)) = (live_deployment.first(), live_owners.first()) {
        validate_live_owner(deployment, owner)?;
    }
    if live.is_some() && live_deployment.len() != 1 {
        return Err("Unpark live deployment evidence is missing".into());
    }
    let restore_suspended = should_restore_suspended_owner(
        &operation,
        live.is_some(),
        live_deployment
            .first()
            .map(|deployment| deployment.owner_kind),
        live_owners.is_empty(),
    );
    let active_id = deployment_id(
        &name,
        "global",
        SkillDestination::Universal,
        "universal",
        None,
        &paths.active,
    );
    let (trial_path, trial_id, trial_link, trial_status) = match &operation {
        OperationKind::UnparkConflict { trash_path } => (
            trash_path.as_path(),
            deployment_id(
                &name,
                "trash",
                SkillDestination::Universal,
                "universal",
                None,
                trash_path,
            ),
            None,
            Some(TrialStatus::RecoveryRequired),
        ),
        _ => (
            paths.active.as_path(),
            active_id,
            reader_target.as_ref().map(|_| paths.reader.clone()),
            None,
        ),
    };
    let trial = selected_trial_transition(
        &registry,
        &document,
        &name,
        &paths.parked,
        &request.deployment_id,
        trial_path,
        &trial_id,
        trial_link,
        trial_status,
    )?;
    let mut transition = ParkRegistryTransition::unpark(name.clone(), parked_raw, trial)?;
    if restore_suspended {
        if let Some(owner) = suspended_owner_transition(&record, &paths.active, parked_deployment)?
        {
            transition = transition.with_owner(
                owner.section,
                owner.key.clone(),
                None,
                owner.key,
                Some(owner.value),
            )?;
        }
    }
    transition.apply(&registry_original)?;
    let intent = ParkIntent {
        version: 1,
        operation_id: operation_id.into(),
        operation,
        name,
        active_path: paths.active,
        parked_path: paths.parked,
        reader_path: paths.reader,
        staged_reader_path: paths.staged_reader,
        retained_reader_path: retained_reader,
        registry_path: paths.registry,
        expected_tree: parked_tree.tree_identity,
        live_tree: live.map(|report| report.tree_identity),
        reader_target,
        reader_root_target,
        reader_was_staged,
        retained_reader_required,
        parked_record: record,
        registry: transition,
    };
    intent.validate(&home)?;
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok((intent, lease))
}

fn settle(
    service: &mut ScopedSkillService,
    store: &EventStore,
    intent: &ParkIntent,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<(), String> {
    let home = service.scope().home.clone();
    intent.validate(&home)?;
    let cancellation = CancellationToken::default();
    for _ in 0..12 {
        let paths = OperationPaths {
            active: intent.active_path.clone(),
            parked: intent.parked_path.clone(),
            reader: intent.reader_path.clone(),
            staged_reader: intent.staged_reader_path.clone(),
            retained_reader: intent
                .retained_reader_required
                .then(|| intent.retained_reader_path.clone()),
            registry: intent.registry_path.clone(),
            trash: intent.trash_path().map(Path::to_path_buf),
        };
        let (_inventory, mut lease) = prepare_lease(
            service,
            store,
            &intent.name,
            &paths,
            timeout,
            cancellation.clone(),
        )?;
        let snapshot = require_pending_intent(store, &lease, intent, &home)?;
        let registry_bytes = lease
            .read_ownership_registry(&intent.registry_path, REGISTRY_LIMIT)
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| b"{}".to_vec());
        let registry_state = intent.registry.state(&registry_bytes)?;
        if registry_state == ParkRegistryState::Conflict {
            return Err(
                "Park registry selected records changed; content was left untouched".into(),
            );
        }
        let phase = filesystem_phase(intent, limits, &cancellation, registry_state)?;
        match phase {
            FilesystemPhase::Before => match &intent.operation {
                OperationKind::Park if intent.reader_target.is_some() => {
                    if ensure_named_root(&home, PARKED_LINK_ROOT, &lease)? {
                        continue;
                    }
                    move_reader_to_stage(intent, &lease)?;
                }
                _ => {
                    let destination = match &intent.operation {
                        OperationKind::Park => &intent.parked_path,
                        OperationKind::UnparkRestored => &intent.active_path,
                        OperationKind::UnparkReconciled { trash_path }
                        | OperationKind::UnparkConflict { trash_path } => trash_path,
                    };
                    if let Some(root_name) = match &intent.operation {
                        OperationKind::Park => Some(PARKED_ROOT),
                        OperationKind::UnparkReconciled { .. }
                        | OperationKind::UnparkConflict { .. } => Some(TRASH_ROOT),
                        OperationKind::UnparkRestored => None,
                    } {
                        if ensure_named_root(&home, root_name, &lease)? {
                            continue;
                        }
                    }
                    let source = match &intent.operation {
                        OperationKind::Park => &intent.active_path,
                        _ => &intent.parked_path,
                    };
                    move_tree(source, destination, &intent.expected_tree, lease, limits)?;
                }
            },
            FilesystemPhase::ReaderMoved => {
                let destination = match &intent.operation {
                    OperationKind::Park => &intent.parked_path,
                    _ => return Err("Unpark reader moved before its tree".into()),
                };
                if ensure_named_root(&home, PARKED_ROOT, &lease)? {
                    continue;
                }
                move_tree(
                    &intent.active_path,
                    destination,
                    &intent.expected_tree,
                    lease,
                    limits,
                )?;
            }
            FilesystemPhase::TreeMoved if registry_state == ParkRegistryState::Before => {
                if !matches!(&intent.operation, OperationKind::Park)
                    && intent.reader_target.is_some()
                    && !reader_is_restored(intent)?
                {
                    if intent.reader_root_target.is_some()
                        || inspect_link(&intent.reader_path)?
                            == EntryState::Symlink(intent.reader_target.clone().unwrap())
                    {
                        if intent.retained_reader_required {
                            if ensure_named_root(&home, RETAINED_LINK_ROOT, &lease)? {
                                continue;
                            }
                            retire_staged_reader(intent, &lease)?;
                        } else {
                            restore_reader(intent, &lease)?;
                        }
                    } else {
                        restore_reader(intent, &lease)?;
                    }
                    continue;
                }
                publish_registry(&mut lease, intent, &registry_bytes)?;
            }
            FilesystemPhase::Complete if registry_state == ParkRegistryState::After => {
                let current = require_pending_intent(store, &lease, intent, &home)?;
                if serde_json::to_value(&current).map_err(|error| error.to_string())?
                    != serde_json::to_value(&snapshot).map_err(|error| error.to_string())?
                {
                    return Err("Park event changed before completion".into());
                }
                GuardedEventStore::bind(store, &lease)?
                    .finish_recovery_snapshot(&lease, &snapshot, EventStatus::Done, None)
                    .map_err(|error| error.to_string())?;
                return Ok(());
            }
            FilesystemPhase::Complete | FilesystemPhase::TreeMoved => {
                return Err("Park operation reached an inconsistent registry checkpoint".into())
            }
        }
    }
    Err("Park recovery exceeded its checkpoint limit".into())
}

fn filesystem_phase(
    intent: &ParkIntent,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
    registry: ParkRegistryState,
) -> Result<FilesystemPhase, String> {
    let active = inspect_entry_state(&intent.active_path, limits, cancellation)?;
    let parked = inspect_entry_state(&intent.parked_path, limits, cancellation)?;
    let trash = intent
        .trash_path()
        .map(|path| inspect_entry_state(path, limits, cancellation))
        .transpose()?
        .unwrap_or(EntryState::Absent);
    let reader = inspect_link(&intent.reader_path)?;
    let staged = inspect_link(&intent.staged_reader_path)?;
    let retained = inspect_link(&intent.retained_reader_path)?;
    let whole_root = match &intent.reader_root_target {
        Some(target) => is_exact_reader_root_link(
            intent
                .reader_path
                .parent()
                .ok_or("Park reader root is missing")?,
            target,
            intent
                .active_path
                .parent()
                .ok_or("Park active root is missing")?,
        )?,
        None => false,
    };
    if intent.reader_root_target.is_some() && !whole_root {
        return Err("Claude skills root link changed during Unpark".into());
    }
    let reader_before = (match (&intent.reader_target, whole_root) {
        (Some(target), true) if intent.reader_was_staged => {
            staged == EntryState::Symlink(target.clone())
        }
        (_, true) => staged == EntryState::Absent,
        (Some(target), false) if intent.reader_was_staged => {
            (reader == EntryState::Absent || reader == EntryState::Symlink(target.clone()))
                && staged == EntryState::Symlink(target.clone())
        }
        (Some(target), false) => {
            staged == EntryState::Absent
                && (matches!(&reader, EntryState::Absent)
                    || reader == EntryState::Symlink(target.clone()))
        }
        (None, false) => staged == EntryState::Absent,
    }) && retained == EntryState::Absent;
    let reader_after = (match (&intent.reader_target, whole_root) {
        (_, true) => staged == EntryState::Absent,
        (Some(target), false) => {
            reader == EntryState::Symlink(target.clone()) && staged == EntryState::Absent
        }
        (None, false) => staged == EntryState::Absent,
    }) && if intent.retained_reader_required {
        retained == EntryState::Symlink(intent.reader_target.clone().unwrap())
    } else {
        retained == EntryState::Absent
    };
    let park_link_before = match &intent.reader_target {
        Some(target) => {
            reader == EntryState::Symlink(target.clone()) && staged == EntryState::Absent
        }
        None => staged == EntryState::Absent,
    };
    let park_link_after = match &intent.reader_target {
        Some(target) => {
            reader == EntryState::Absent && staged == EntryState::Symlink(target.clone())
        }
        None => staged == EntryState::Absent,
    };
    let (tree_before, tree_after) = match &intent.operation {
        OperationKind::Park => (
            active == EntryState::Tree(intent.expected_tree.clone())
                && parked == EntryState::Absent
                && trash == EntryState::Absent,
            active == EntryState::Absent
                && parked == EntryState::Tree(intent.expected_tree.clone())
                && trash == EntryState::Absent,
        ),
        OperationKind::UnparkRestored => (
            active == EntryState::Absent
                && parked == EntryState::Tree(intent.expected_tree.clone())
                && trash == EntryState::Absent,
            active == EntryState::Tree(intent.expected_tree.clone())
                && parked == EntryState::Absent
                && trash == EntryState::Absent,
        ),
        OperationKind::UnparkReconciled { .. } | OperationKind::UnparkConflict { .. } => {
            let live = intent
                .live_tree
                .clone()
                .ok_or("Unpark live tree evidence is missing")?;
            (
                active == EntryState::Tree(live.clone())
                    && parked == EntryState::Tree(intent.expected_tree.clone())
                    && trash == EntryState::Absent,
                active == EntryState::Tree(live)
                    && parked == EntryState::Absent
                    && trash == EntryState::Tree(intent.expected_tree.clone()),
            )
        }
    };
    let phase = match &intent.operation {
        OperationKind::Park if tree_before && park_link_before => FilesystemPhase::Before,
        OperationKind::Park if tree_before && park_link_after => FilesystemPhase::ReaderMoved,
        OperationKind::Park if tree_after && park_link_after => {
            if registry == ParkRegistryState::After {
                FilesystemPhase::Complete
            } else {
                FilesystemPhase::TreeMoved
            }
        }
        OperationKind::UnparkRestored
        | OperationKind::UnparkReconciled { .. }
        | OperationKind::UnparkConflict { .. }
            if tree_before && reader_before =>
        {
            FilesystemPhase::Before
        }
        OperationKind::UnparkRestored
        | OperationKind::UnparkReconciled { .. }
        | OperationKind::UnparkConflict { .. }
            if tree_after && reader_after =>
        {
            if registry == ParkRegistryState::After {
                FilesystemPhase::Complete
            } else {
                FilesystemPhase::TreeMoved
            }
        }
        OperationKind::UnparkRestored
        | OperationKind::UnparkReconciled { .. }
        | OperationKind::UnparkConflict { .. }
            if tree_after && reader_before =>
        {
            FilesystemPhase::TreeMoved
        }
        _ => return Err(
            "Park recovery found content, link, or destination drift; both versions were preserved"
                .into(),
        ),
    };
    Ok(phase)
}

fn publish_registry(
    lease: &mut FinalizedWriteLease<'_>,
    intent: &ParkIntent,
    original: &[u8],
) -> Result<(), String> {
    let proposed = intent.registry.apply(original)?;
    let target = crate::skill_document_target::SkillRegistryTarget::bind(
        intent
            .registry_path
            .parent()
            .ok_or("Park registry parent is missing")?,
    )?;
    if original == b"{}"
        && lease
            .read_ownership_registry(&intent.registry_path, REGISTRY_LIMIT)
            .map_err(|error| error.to_string())?
            .is_none()
    {
        target
            .create(lease, &proposed)
            .map_err(|error| error.to_string())?;
    } else {
        target
            .replace(lease, original, &proposed)
            .map_err(|error| error.to_string())?;
    }
    lease.revalidate().map_err(|error| error.to_string())
}

fn require_pending_intent(
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    intent: &ParkIntent,
    home: &Path,
) -> Result<EventRow, String> {
    let row = GuardedEventStore::bind(store, lease)?
        .next_recovery_event(lease)?
        .ok_or("Park recovery event is no longer pending")?;
    let current = ParkIntent::from_event(home, &row)?;
    if serde_json::to_value(&current).map_err(|error| error.to_string())?
        != serde_json::to_value(intent).map_err(|error| error.to_string())?
    {
        return Err("Park recovery event changed".into());
    }
    Ok(row)
}

fn prepare_lease<'scope>(
    service: &'scope mut ScopedSkillService,
    store: &EventStore,
    name: &str,
    paths: &OperationPaths,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<(InventoryRead, FinalizedWriteLease<'scope>), String> {
    let mut trees = vec![store.app_data.clone()];
    for path in [&paths.active, &paths.parked]
        .into_iter()
        .chain(paths.trash.iter())
    {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => trees.push(path.clone()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let mut entries = vec![
        paths.active.clone(),
        paths.parked.clone(),
        paths.reader.clone(),
        paths
            .reader
            .parent()
            .ok_or("Claude reader root missing")?
            .to_path_buf(),
        paths.staged_reader.clone(),
        paths.registry.clone(),
        paths
            .active
            .parent()
            .ok_or("Active root missing")?
            .to_path_buf(),
        paths
            .parked
            .parent()
            .ok_or("Parked root missing")?
            .to_path_buf(),
        paths
            .staged_reader
            .parent()
            .ok_or("Reader stage root missing")?
            .to_path_buf(),
    ];
    if let Some(trash) = &paths.trash {
        entries.push(trash.clone());
        entries.push(trash.parent().ok_or("Trash root missing")?.to_path_buf());
    }
    if let Some(retained_reader) = &paths.retained_reader {
        entries.push(retained_reader.clone());
        entries.push(
            retained_reader
                .parent()
                .ok_or("Retained reader root missing")?
                .to_path_buf(),
        );
    }
    service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([name.to_string()])),
            &trees,
            &entries,
            timeout,
            cancellation,
        )
        .map_err(|error| error.to_string())
}

struct OperationPaths {
    active: PathBuf,
    parked: PathBuf,
    reader: PathBuf,
    staged_reader: PathBuf,
    retained_reader: Option<PathBuf>,
    registry: PathBuf,
    trash: Option<PathBuf>,
}

impl OperationPaths {
    fn new(
        home: &Path,
        name: &str,
        trash: Option<PathBuf>,
        retained_reader: Option<PathBuf>,
    ) -> Self {
        Self {
            active: home.join(".agents/skills").join(name),
            parked: home.join(".agents").join(PARKED_ROOT).join(name),
            reader: home.join(".claude/skills").join(name),
            staged_reader: home.join(".agents").join(PARKED_LINK_ROOT).join(name),
            retained_reader,
            registry: home.join(".agents/skill-studio.json"),
            trash,
        }
    }
}

#[derive(Clone)]
struct RawOwner {
    section: &'static str,
    key: String,
    value: Value,
    record: ParkedOwnerRecord,
}

fn suspended_owner(
    deployment: &Deployment,
    registry: &ForkRegistry,
    document: &Value,
    active: &Path,
) -> Result<Option<RawOwner>, String> {
    match deployment.owner_kind {
        LifecycleOwnerKind::Copy => {
            let key = deployment.id.clone();
            let record = registry
                .copies
                .get(&key)
                .ok_or("Selected Copy owner record is missing")?;
            if record.path != active
                || deployment.owner_revision.as_deref()
                    != RegistryOwnerRecord::Copy(record).revision().as_deref()
            {
                return Err("Selected Copy owner changed before Park".into());
            }
            let value = section_value(document, "copies", &key)
                .ok_or("Selected Copy raw owner record is missing")?
                .clone();
            Ok(Some(RawOwner {
                section: "copies",
                key: key.clone(),
                value: value.clone(),
                record: ParkedOwnerRecord::Copy { key, value },
            }))
        }
        LifecycleOwnerKind::Fork => {
            let key = parse_deployment_id(&deployment.id)
                .map(|id| id.name)
                .ok_or("Selected Fork deployment ID is invalid")?;
            let record = registry
                .forks
                .get(&key)
                .ok_or("Selected Fork owner record is missing")?;
            if record.skill_dir != active
                || (!record.deployment_id.is_empty() && record.deployment_id != deployment.id)
                || deployment.owner_revision.as_deref()
                    != RegistryOwnerRecord::Fork(record).revision().as_deref()
            {
                return Err("Selected Fork owner changed before Park".into());
            }
            let value = section_value(document, "forks", &key)
                .ok_or("Selected Fork raw owner record is missing")?
                .clone();
            Ok(Some(RawOwner {
                section: "forks",
                key: key.clone(),
                value: value.clone(),
                record: ParkedOwnerRecord::Fork { key, value },
            }))
        }
        LifecycleOwnerKind::Manual
        | LifecycleOwnerKind::SkillsSh
        | LifecycleOwnerKind::Dotagents => Ok(None),
        LifecycleOwnerKind::Plugin
        | LifecycleOwnerKind::InRepo
        | LifecycleOwnerKind::WildcardDotagents
        | LifecycleOwnerKind::Ambiguous
        | LifecycleOwnerKind::Unknown => {
            Err("Selected Global Universal ownership is not safe to Park".into())
        }
    }
}

fn suspended_owner_transition(
    record: &ParkedRecord,
    active: &Path,
    parked_deployment: &Deployment,
) -> Result<Option<RawOwner>, String> {
    let Some(owner) = &record.suspended_owner else {
        return Ok(None);
    };
    let (section, key, value) = match owner {
        ParkedOwnerRecord::Copy { key, value } => {
            let copy: crate::skill_fork_registry::CopyDeploymentRecord =
                serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
            if copy.deployment_id != *key
                || copy.path != active
                || (!copy.content_hash.is_empty()
                    && copy.content_hash != parked_deployment.content_hash)
            {
                return Err("Suspended Copy owner does not match parked content".into());
            }
            ("copies", key, value)
        }
        ParkedOwnerRecord::Fork { key, value } => {
            let fork: crate::skill_fork_registry::ForkRecord =
                serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
            if fork.skill_dir != active {
                return Err("Suspended Fork owner does not match the original directory".into());
            }
            ("forks", key, value)
        }
    };
    Ok(Some(RawOwner {
        section,
        key: key.clone(),
        value: value.clone(),
        record: owner.clone(),
    }))
}

fn live_registry_owners(
    registry: &ForkRegistry,
    document: &Value,
    name: &str,
    active: &Path,
) -> Result<Vec<RawOwner>, String> {
    let mut owners = Vec::new();
    for (key, record) in &registry.copies {
        if record.name == name && record.path == active {
            let value = section_value(document, "copies", key)
                .ok_or("Live Copy raw owner record is missing")?
                .clone();
            owners.push(RawOwner {
                section: "copies",
                key: key.clone(),
                value: value.clone(),
                record: ParkedOwnerRecord::Copy {
                    key: key.clone(),
                    value,
                },
            });
        }
    }
    for (key, record) in &registry.forks {
        if key == name && record.skill_dir == active {
            let value = section_value(document, "forks", key)
                .ok_or("Live Fork raw owner record is missing")?
                .clone();
            owners.push(RawOwner {
                section: "forks",
                key: key.clone(),
                value: value.clone(),
                record: ParkedOwnerRecord::Fork {
                    key: key.clone(),
                    value,
                },
            });
        }
    }
    Ok(owners)
}

fn validate_live_owner(deployment: &Deployment, owner: &RawOwner) -> Result<(), String> {
    let expected = match &owner.record {
        ParkedOwnerRecord::Copy { value, .. } => {
            let record: crate::skill_fork_registry::CopyDeploymentRecord =
                serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
            RegistryOwnerRecord::Copy(&record).revision()
        }
        ParkedOwnerRecord::Fork { value, .. } => {
            let record: crate::skill_fork_registry::ForkRecord =
                serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
            RegistryOwnerRecord::Fork(&record).revision()
        }
    };
    if deployment.owner_revision != expected {
        return Err("Live reinstall ownership changed during Unpark".into());
    }
    Ok(())
}

fn should_restore_suspended_owner(
    operation: &OperationKind,
    has_live_tree: bool,
    live_owner: Option<LifecycleOwnerKind>,
    has_no_registry_owner: bool,
) -> bool {
    !matches!(operation, OperationKind::UnparkConflict { .. })
        && has_no_registry_owner
        && matches!(
            (has_live_tree, live_owner),
            (false, None) | (true, Some(LifecycleOwnerKind::Manual))
        )
}

#[allow(clippy::too_many_arguments)]
fn selected_trial_transition(
    registry: &ForkRegistry,
    document: &Value,
    name: &str,
    source_path: &Path,
    source_id: &str,
    destination_path: &Path,
    destination_id: &str,
    reader: Option<PathBuf>,
    status: Option<TrialStatus>,
) -> Result<Option<TrialTransition>, String> {
    let legacy = trial_key(TrialScope::Global, name);
    let matches = registry
        .trials
        .iter()
        .filter(|(key, trial)| {
            trial.scope == TrialScope::Global
                && trial.skill_dir == source_path
                && (trial.deployment_id == source_id
                    || (trial.deployment_id.is_empty() && key.as_str() == legacy))
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err("Park found ambiguous Global trial ownership".into());
    }
    let Some((key, _trial)) = matches.first() else {
        return Ok(None);
    };
    let before = section_value(document, "trials", key)
        .ok_or("Selected raw trial record is missing")?
        .clone();
    let mut after = before.clone();
    let object = after
        .as_object_mut()
        .ok_or("Selected trial record must be an object")?;
    object.insert("deployment_id".into(), Value::String(destination_id.into()));
    object.insert(
        "skill_dir".into(),
        serde_json::to_value(destination_path).map_err(|error| error.to_string())?,
    );
    object.insert(
        "claude_link".into(),
        serde_json::to_value(reader).map_err(|error| error.to_string())?,
    );
    if let Some(status) = status {
        object.insert(
            "status".into(),
            serde_json::to_value(status).map_err(|error| error.to_string())?,
        );
    }
    let _: TrialRecord =
        serde_json::from_value(after.clone()).map_err(|error| error.to_string())?;
    Ok(Some(ParkRegistryTransition::trial(
        (*key).clone(),
        before,
        deployment_trial_key(destination_id),
        after,
    )))
}

fn validate_live_park_deployment(deployment: &Deployment, active: &Path) -> Result<(), String> {
    if deployment.scope != "global"
        || deployment.destination != SkillDestination::Universal
        || !matches!(deployment.backing, BackingRelationship::Canonical)
        || Path::new(&deployment.path) != active
        || deployment.is_symlink
        || deployment.plugin.is_some()
        || !matches!(
            deployment.owner_kind,
            LifecycleOwnerKind::Manual
                | LifecycleOwnerKind::SkillsSh
                | LifecycleOwnerKind::Dotagents
                | LifecycleOwnerKind::Copy
                | LifecycleOwnerKind::Fork
        )
    {
        return Err("Park is limited to one supported Global Universal deployment".into());
    }
    Ok(())
}

fn exact_deployment<'a>(inventory: &'a InventoryRead, id: &str) -> Result<&'a Deployment, String> {
    let matches = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| deployment.id == id)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [deployment] => Ok(*deployment),
        [] => Err("Selected Park deployment is absent".into()),
        _ => Err("Selected Park deployment is ambiguous".into()),
    }
}

fn parse_registry(bytes: &[u8]) -> Result<(ForkRegistry, Value), String> {
    let document =
        serde_json::from_slice::<crate::skill_skills_sh_lock_transition::UniqueJson>(bytes)
            .map(|document| document.0)
            .map_err(|error| format!("Registry is malformed: {error}"))?;
    if !document.is_object() {
        return Err("Registry root must be an object".into());
    }
    let registry = serde_json::from_value(document.clone())
        .map_err(|error| format!("Registry is malformed: {error}"))?;
    Ok((registry, document))
}

fn section_value<'a>(document: &'a Value, section: &str, key: &str) -> Option<&'a Value> {
    document
        .get(section)
        .and_then(Value::as_object)
        .and_then(|records| records.get(key))
}

fn admitted_reader(reader: &Path, active: &Path) -> Result<(Option<PathBuf>, bool), String> {
    let parent = reader.parent().ok_or("Claude reader parent is missing")?;
    if let Ok(metadata) = std::fs::symlink_metadata(parent) {
        if metadata.file_type().is_symlink() {
            return Ok((None, false));
        }
    }
    let EntryState::Symlink(target) = inspect_link(reader)? else {
        return Ok((None, false));
    };
    let resolved = parent.join(&target);
    if std::fs::canonicalize(&resolved).ok().as_deref()
        == std::fs::canonicalize(active).ok().as_deref()
    {
        Ok((Some(target), true))
    } else {
        Ok((None, false))
    }
}

fn admitted_reader_root(reader: &Path, active: &Path) -> Result<Option<PathBuf>, String> {
    let root = reader.parent().ok_or("Claude reader root is missing")?;
    match inspect_link(root)? {
        EntryState::Absent | EntryState::Other | EntryState::Tree(_) => Ok(None),
        EntryState::Symlink(target) => {
            if is_exact_reader_root_link(
                root,
                &target,
                active.parent().ok_or("Park active root is missing")?,
            )? {
                Ok(Some(target))
            } else {
                Err("Claude skills root link is changed or misdirected".into())
            }
        }
    }
}

fn is_exact_reader_root_link(
    root: &Path,
    expected_target: &Path,
    active_root: &Path,
) -> Result<bool, String> {
    if inspect_link(root)? != EntryState::Symlink(expected_target.to_path_buf()) {
        return Ok(false);
    }
    let resolved = std::fs::canonicalize(
        root.parent()
            .ok_or("Claude reader root parent is missing")?
            .join(expected_target),
    )
    .map_err(|error| error.to_string())?;
    let expected = std::fs::canonicalize(active_root).map_err(|error| error.to_string())?;
    Ok(resolved == expected)
}

fn inspect_tree(
    path: &Path,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<Option<BackupCopyReport>, String> {
    match inspect_entry_state(path, limits, cancellation)? {
        EntryState::Absent => Ok(None),
        EntryState::Tree(_) => {
            let parent = path.parent().ok_or("Tree parent is missing")?;
            let root = BackupSourceRoot::bind(parent).map_err(|error| error.to_string())?;
            let source = root
                .select(path.file_name().ok_or("Tree name is missing")?)
                .map_err(|error| error.to_string())?;
            source
                .inspect(limits, cancellation)
                .map(Some)
                .map_err(|error| error.to_string())
        }
        EntryState::Symlink(_) | EntryState::Other => Err(format!(
            "{} is not an independent directory",
            path.display()
        )),
    }
}

fn inspect_entry_state(
    path: &Path,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<EntryState, String> {
    let parent = path.parent().ok_or("Entry parent is missing")?;
    let metadata = match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(EntryState::Absent)
        }
        Err(error) => return Err(error.to_string()),
        Ok(metadata) => metadata,
    };
    if metadata.file_type().is_symlink() {
        return std::fs::read_link(path)
            .map(EntryState::Symlink)
            .map_err(|error| error.to_string());
    }
    if !metadata.is_dir() {
        return Ok(EntryState::Other);
    }
    let scope = SkillReadScope::bind(&[parent.to_path_buf()]).map_err(|error| error.to_string())?;
    let directory: cap_std::fs::Dir = scope
        .clone_bound_directory(parent)
        .map_err(|error| error.to_string())?
        .into();
    inspect_entry(
        &directory,
        path.file_name().ok_or("Entry name is missing")?,
        limits,
        cancellation,
    )
    .map(|report| EntryState::Tree(report.tree_identity))
    .map_err(|error| error.to_string())
}

fn inspect_link(path: &Path) -> Result<EntryState, String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(EntryState::Absent),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if metadata.file_type().is_symlink() => std::fs::read_link(path)
            .map(EntryState::Symlink)
            .map_err(|error| error.to_string()),
        Ok(_) => Ok(EntryState::Other),
    }
}

fn ensure_named_root(
    home: &Path,
    name: &str,
    lease: &FinalizedWriteLease<'_>,
) -> Result<bool, String> {
    let agents = home.join(".agents");
    let root = BackupSourceRoot::bind(&agents).map_err(|error| error.to_string())?;
    let selected = root
        .select(OsStr::new(name))
        .map_err(|error| error.to_string())?;
    match std::fs::symlink_metadata(home.join(".agents").join(name)) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            lease.revalidate().map_err(|error| error.to_string())?;
            selected
                .create_directory()
                .map_err(|error| error.to_string())?;
            Ok(true)
        }
        Err(error) => Err(error.to_string()),
        Ok(_) => Err(format!("Park root {} is not a directory", name)),
    }
}

fn move_tree(
    source: &Path,
    destination: &Path,
    expected_tree: &str,
    lease: FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
) -> Result<(), String> {
    let source = source_for(source)?;
    let destination = source_for(destination)?;
    source
        .move_verified_tree(
            &destination,
            expected_tree,
            lease,
            limits,
            &CancellationToken::default(),
        )
        .map_err(|failure| match failure {
            crate::skill_tree_move::TreeMoveFailure::BeforeMove(message)
            | crate::skill_tree_move::TreeMoveFailure::MayHaveMoved(message) => message,
        })
}

fn source_for(path: &Path) -> Result<BackupSource, String> {
    BackupSourceRoot::bind(path.parent().ok_or("Entry parent is missing")?)
        .map_err(|error| error.to_string())?
        .select(path.file_name().ok_or("Entry name is missing")?)
        .map_err(|error| error.to_string())
}

fn move_reader_to_stage(
    intent: &ParkIntent,
    lease: &FinalizedWriteLease<'_>,
) -> Result<(), String> {
    let target = intent
        .reader_target
        .as_deref()
        .ok_or("Park reader target is missing")?;
    lease.validate_entry_move(&intent.reader_path, &intent.staged_reader_path)?;
    source_for(&intent.reader_path)?
        .move_exact_symlink_to(&source_for(&intent.staged_reader_path)?, target)
        .map_err(|error| error.to_string())
}

fn restore_reader(intent: &ParkIntent, lease: &FinalizedWriteLease<'_>) -> Result<(), String> {
    let target = intent
        .reader_target
        .as_deref()
        .ok_or("Unpark reader target is missing")?;
    lease.validate_entry_move(&intent.staged_reader_path, &intent.reader_path)?;
    let reader = source_for(&intent.reader_path)?;
    if intent.reader_was_staged {
        reader
            .restore_exact_symlink_from(&source_for(&intent.staged_reader_path)?, target)
            .map_err(|error| error.to_string())
    } else {
        reader
            .restore_absent_symlink(target)
            .map_err(|error| error.to_string())
    }
}

fn retire_staged_reader(
    intent: &ParkIntent,
    lease: &FinalizedWriteLease<'_>,
) -> Result<(), String> {
    let target = intent
        .reader_target
        .as_deref()
        .ok_or("Unpark reader target is missing")?;
    lease.validate_entry_move(&intent.staged_reader_path, &intent.retained_reader_path)?;
    source_for(&intent.staged_reader_path)?
        .move_exact_symlink_to(&source_for(&intent.retained_reader_path)?, target)
        .map_err(|error| error.to_string())
}

fn reader_is_restored(intent: &ParkIntent) -> Result<bool, String> {
    let Some(target) = &intent.reader_target else {
        return Ok(true);
    };
    if let Some(root_target) = &intent.reader_root_target {
        return Ok(is_exact_reader_root_link(
            intent
                .reader_path
                .parent()
                .ok_or("Park reader root is missing")?,
            root_target,
            intent
                .active_path
                .parent()
                .ok_or("Park active root is missing")?,
        )? && inspect_link(&intent.staged_reader_path)? == EntryState::Absent);
    }
    Ok(
        inspect_link(&intent.reader_path)? == EntryState::Symlink(target.clone())
            && inspect_link(&intent.staged_reader_path)? == EntryState::Absent,
    )
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.as_bytes().contains(&0)
    {
        return Err("Invalid Park skill name".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_deployment::InstallScope;
    use crate::skill_event_store::EventStore;
    use crate::skill_fork_registry::{AddMethod, CopyDeploymentRecord};
    use crate::skill_service::SkillScope;
    use serde_json::json;
    use std::fs;

    const LIMITS: BackupCopyLimits = BackupCopyLimits {
        max_bytes: 1024 * 1024,
        max_entries: 100,
        max_depth: 8,
    };
    const TIMEOUT: Option<Duration> = Some(Duration::from_secs(5));

    fn scope(home: &Path) -> SkillScope {
        SkillScope {
            home: home.to_path_buf(),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        }
    }

    fn write_skill_tree(path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(
            path.join("SKILL.md"),
            "---\nname: sample\ndescription: Test skill\n---\nBody\n",
        )
        .unwrap();
        fs::write(path.join("resource.txt"), "resource").unwrap();
        std::os::unix::fs::symlink("missing-target", path.join("broken-link")).unwrap();
    }

    fn selected_deployment(service: &mut ScopedSkillService, path: &Path) -> Deployment {
        service
            .scan(None, TIMEOUT)
            .unwrap()
            .skills
            .into_iter()
            .flat_map(|skill| skill.deployments)
            .find(|deployment| Path::new(&deployment.path) == path)
            .unwrap()
    }

    #[test]
    fn park_and_no_reader_unpark_complete_the_durable_operation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let active = home.join(".agents/skills/sample");
        write_skill_tree(&active);
        fs::write(
            home.join(".agents/skill-studio.json"),
            serde_json::to_vec(&json!({
                "version": 4,
                "future": {"keep": true},
                "trials": {}
            }))
            .unwrap(),
        )
        .unwrap();
        let mut service = ScopedSkillService::bind(scope(&home)).unwrap();
        let selected = selected_deployment(&mut service, &active);
        let store = EventStore::open(&root.join("events")).unwrap();

        let parked = park_skill(
            &mut service,
            &store,
            &ParkSkillRequest {
                deployment_id: selected.id,
                source_kind: SourceKind::Manual,
                parked_at: "2026-09-17T00:00:00Z".into(),
            },
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        assert!(parked.claude_link.is_none());
        assert!(!active.exists());
        assert!(parked.skill_dir.join("broken-link").is_symlink());

        let outcome = unpark_skill(
            &mut service,
            &store,
            &UnparkSkillRequest {
                deployment_id: parked.deployment_id,
            },
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();

        assert_eq!(outcome, UnparkOutcome::Restored);
        assert!(active.join("resource.txt").is_file());
        assert!(!parked.skill_dir.exists());
        let registry: Value =
            serde_json::from_slice(&fs::read(home.join(".agents/skill-studio.json")).unwrap())
                .unwrap();
        assert_eq!(registry["future"]["keep"], true);
        assert!(registry["parked"].as_object().unwrap().is_empty());
        assert!(store
            .list(10, Some("sample"))
            .unwrap()
            .iter()
            .all(|event| event.status == "done" && !event.restorable));
    }

    #[test]
    fn identical_manual_recreation_restores_copy_owner_and_reconciles_duplicate_reader() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let active = home.join(".agents/skills/sample");
        let reader = home.join(".claude/skills/sample");
        write_skill_tree(&active);
        fs::create_dir_all(reader.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/sample", &reader).unwrap();
        let mut service = ScopedSkillService::bind(scope(&home)).unwrap();
        let manual = selected_deployment(&mut service, &active);
        let copy = CopyDeploymentRecord {
            deployment_id: manual.id.clone(),
            name: "sample".into(),
            path: active.clone(),
            scope: InstallScope::Global,
            destination: SkillDestination::Universal,
            slot: "universal".into(),
            project_path: None,
            content_hash: manual.content_hash,
            disabled: false,
        };
        let mut raw_copy = serde_json::to_value(&copy).unwrap();
        raw_copy["future"] = json!({"keep": true});
        fs::write(
            home.join(".agents/skill-studio.json"),
            serde_json::to_vec(&json!({
                "version": 4,
                "copies": {(copy.deployment_id.clone()): raw_copy},
                "trials": {}
            }))
            .unwrap(),
        )
        .unwrap();
        let mut service = ScopedSkillService::bind(scope(&home)).unwrap();
        let owned = selected_deployment(&mut service, &active);
        assert_eq!(owned.owner_kind, LifecycleOwnerKind::Copy);
        let store = EventStore::open(&root.join("events")).unwrap();
        let parked = park_skill(
            &mut service,
            &store,
            &ParkSkillRequest {
                deployment_id: owned.id,
                source_kind: SourceKind::Manual,
                parked_at: "2026-09-17T00:00:00Z".into(),
            },
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        write_skill_tree(&active);
        std::os::unix::fs::symlink("../../.agents/skills/sample", &reader).unwrap();

        let outcome = unpark_skill(
            &mut service,
            &store,
            &UnparkSkillRequest {
                deployment_id: parked.deployment_id,
            },
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();

        assert_eq!(outcome, UnparkOutcome::Reconciled);
        assert_eq!(
            fs::read_link(&reader).unwrap(),
            PathBuf::from("../../.agents/skills/sample")
        );
        let registry: Value =
            serde_json::from_slice(&fs::read(home.join(".agents/skill-studio.json")).unwrap())
                .unwrap();
        assert_eq!(
            registry["copies"][copy.deployment_id.as_str()]["future"]["keep"],
            true
        );
        assert!(registry["parked"].as_object().unwrap().is_empty());
        assert_eq!(
            fs::read_dir(home.join(".agents/skills-trash"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn identical_reinstall_with_exact_claude_root_link_retires_staged_reader() {
        assert_whole_root_reinstall(false);
    }

    #[test]
    fn legacy_parked_record_reconciles_whole_root_without_staged_reader() {
        assert_whole_root_reinstall(true);
    }

    fn assert_whole_root_reinstall(legacy: bool) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let active = home.join(".agents/skills/sample");
        let reader_root = home.join(".claude/skills");
        let reader = reader_root.join("sample");
        write_skill_tree(&active);
        fs::create_dir_all(&reader_root).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/sample", &reader).unwrap();
        let mut service = ScopedSkillService::bind(scope(&home)).unwrap();
        let selected = selected_deployment(&mut service, &active);
        let store = EventStore::open(&root.join("events")).unwrap();
        let parked = park_skill(
            &mut service,
            &store,
            &ParkSkillRequest {
                deployment_id: selected.id,
                source_kind: SourceKind::Manual,
                parked_at: "2026-09-17T00:00:00Z".into(),
            },
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        if legacy {
            fs::remove_file(home.join(".agents/skills-parked-links/sample")).unwrap();
            let registry_path = home.join(".agents/skill-studio.json");
            let mut raw: Value =
                serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
            let record = raw["parked"]["sample"].as_object_mut().unwrap();
            record.remove("skill_dir");
            record.remove("deployment_id");
            fs::write(&registry_path, serde_json::to_vec(&raw).unwrap()).unwrap();
        }
        write_skill_tree(&active);
        fs::remove_dir(&reader_root).unwrap();
        std::os::unix::fs::symlink("../.agents/skills", &reader_root).unwrap();

        let outcome = unpark_skill(
            &mut service,
            &store,
            &UnparkSkillRequest {
                deployment_id: parked.deployment_id,
            },
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();

        assert_eq!(outcome, UnparkOutcome::Reconciled);
        assert_eq!(
            fs::read_link(&reader_root).unwrap(),
            PathBuf::from("../.agents/skills")
        );
        assert!(reader.join("SKILL.md").is_file());
        assert!(!home.join(".agents/skills-parked-links/sample").exists());
    }

    #[test]
    fn recovery_completes_reinstalled_readers_at_tree_and_reader_checkpoints() {
        for reader_mode in 0..3 {
            for reader_checkpoint in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().canonicalize().unwrap();
                let home = root.join("home");
                let active = home.join(".agents/skills/sample");
                let reader_root = home.join(".claude/skills");
                let reader = reader_root.join("sample");
                write_skill_tree(&active);
                fs::create_dir_all(&reader_root).unwrap();
                std::os::unix::fs::symlink("../../.agents/skills/sample", &reader).unwrap();
                let mut service = ScopedSkillService::bind(scope(&home)).unwrap();
                let selected = selected_deployment(&mut service, &active);
                let store = EventStore::open(&root.join("events")).unwrap();
                let parked = park_skill(
                    &mut service,
                    &store,
                    &ParkSkillRequest {
                        deployment_id: selected.id,
                        source_kind: SourceKind::Manual,
                        parked_at: "2026-09-17T00:00:00Z".into(),
                    },
                    LIMITS,
                    TIMEOUT,
                    CancellationToken::default(),
                )
                .unwrap();
                write_skill_tree(&active);
                if reader_mode == 0 {
                    fs::remove_dir(&reader_root).unwrap();
                    std::os::unix::fs::symlink("../.agents/skills", &reader_root).unwrap();
                } else if reader_mode == 1 {
                    std::os::unix::fs::symlink("../../.agents/skills/sample", &reader).unwrap();
                }
                let id = crate::skill_event_store::allocate_id();
                let (intent, lease) = prepare_unpark(
                    &mut service,
                    &store,
                    &UnparkSkillRequest {
                        deployment_id: parked.deployment_id,
                    },
                    &id,
                    LIMITS,
                    TIMEOUT,
                    CancellationToken::default(),
                )
                .unwrap();
                let mut malformed = intent.clone();
                malformed.retained_reader_required = true;
                malformed.reader_target = None;
                assert!(malformed.validate(&home).is_err());
                record_intent(&store, &lease, &intent).unwrap();
                drop(lease);
                fs::create_dir_all(intent.trash_path().unwrap().parent().unwrap()).unwrap();
                fs::rename(&intent.parked_path, intent.trash_path().unwrap()).unwrap();
                if reader_checkpoint {
                    let destination = if intent.retained_reader_required {
                        &intent.retained_reader_path
                    } else {
                        &intent.reader_path
                    };
                    fs::create_dir_all(destination.parent().unwrap()).unwrap();
                    fs::rename(&intent.staged_reader_path, destination).unwrap();
                }
                drop(service);

                let mut restarted = ScopedSkillService::bind(scope(&home)).unwrap();
                let row = store.get(&id).unwrap().unwrap();
                recover_park_operation(&mut restarted, &store, &row, LIMITS, TIMEOUT).unwrap();

                if reader_mode == 0 {
                    assert_eq!(
                        fs::read_link(&reader_root).unwrap(),
                        PathBuf::from("../.agents/skills")
                    );
                }
                assert_eq!(intent.retained_reader_path.is_symlink(), reader_mode != 2);
                if reader_mode != 2 {
                    assert_eq!(
                        fs::read_link(&intent.retained_reader_path).unwrap(),
                        PathBuf::from("../../.agents/skills/sample")
                    );
                }
                assert!(reader.join("SKILL.md").is_file());
                assert!(!home.join(".agents/skills-parked-links/sample").exists());
                assert_eq!(store.get(&id).unwrap().unwrap().status, "done");
            }
        }
    }

    #[test]
    fn recovery_refuses_changed_claude_root_link_after_tree_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let active = home.join(".agents/skills/sample");
        let reader_root = home.join(".claude/skills");
        let reader = reader_root.join("sample");
        write_skill_tree(&active);
        fs::create_dir_all(&reader_root).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/sample", &reader).unwrap();
        let mut service = ScopedSkillService::bind(scope(&home)).unwrap();
        let selected = selected_deployment(&mut service, &active);
        let store = EventStore::open(&root.join("events")).unwrap();
        let parked = park_skill(
            &mut service,
            &store,
            &ParkSkillRequest {
                deployment_id: selected.id,
                source_kind: SourceKind::Manual,
                parked_at: "2026-09-17T00:00:00Z".into(),
            },
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        write_skill_tree(&active);
        fs::remove_dir(&reader_root).unwrap();
        std::os::unix::fs::symlink("../.agents/skills", &reader_root).unwrap();
        let id = crate::skill_event_store::allocate_id();
        let (intent, lease) = prepare_unpark(
            &mut service,
            &store,
            &UnparkSkillRequest {
                deployment_id: parked.deployment_id,
            },
            &id,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        record_intent(&store, &lease, &intent).unwrap();
        drop(lease);
        fs::create_dir_all(intent.trash_path().unwrap().parent().unwrap()).unwrap();
        fs::rename(&intent.parked_path, intent.trash_path().unwrap()).unwrap();
        fs::remove_file(&reader_root).unwrap();
        fs::create_dir_all(home.join("else")).unwrap();
        std::os::unix::fs::symlink("../else", &reader_root).unwrap();

        let row = store.get(&id).unwrap().unwrap();
        assert!(recover_park_operation(&mut service, &store, &row, LIMITS, TIMEOUT).is_err());
        assert!(intent.trash_path().unwrap().join("SKILL.md").is_file());
        assert!(home.join(".agents/skills-parked-links/sample").is_symlink());
        assert_eq!(store.get(&id).unwrap().unwrap().status, "pending");
    }

    #[test]
    fn unpark_refuses_independent_claude_reader_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let active = home.join(".agents/skills/sample");
        let reader = home.join(".claude/skills/sample");
        write_skill_tree(&active);
        fs::create_dir_all(reader.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/sample", &reader).unwrap();
        let mut service = ScopedSkillService::bind(scope(&home)).unwrap();
        let selected = selected_deployment(&mut service, &active);
        let store = EventStore::open(&root.join("events")).unwrap();
        let parked = park_skill(
            &mut service,
            &store,
            &ParkSkillRequest {
                deployment_id: selected.id,
                source_kind: SourceKind::Manual,
                parked_at: "2026-09-17T00:00:00Z".into(),
            },
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        fs::create_dir(&reader).unwrap();
        fs::write(reader.join("unrelated"), "keep").unwrap();

        let error = unpark_skill(
            &mut service,
            &store,
            &UnparkSkillRequest {
                deployment_id: parked.deployment_id,
            },
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap_err();

        assert!(error.contains("Claude reader path is occupied"));
        assert!(reader.join("unrelated").is_file());
        assert!(parked.skill_dir.join("SKILL.md").is_file());
    }

    #[test]
    fn suspended_owner_is_not_restored_over_provider_ownership() {
        let operation = OperationKind::UnparkReconciled {
            trash_path: PathBuf::from("/trash/sample"),
        };
        assert!(should_restore_suspended_owner(
            &operation,
            true,
            Some(LifecycleOwnerKind::Manual),
            true,
        ));
        for owner in [
            LifecycleOwnerKind::SkillsSh,
            LifecycleOwnerKind::Dotagents,
            LifecycleOwnerKind::Copy,
            LifecycleOwnerKind::Fork,
        ] {
            assert!(!should_restore_suspended_owner(
                &operation,
                true,
                Some(owner),
                true,
            ));
        }
    }

    #[test]
    fn recovery_completes_park_and_unpark_from_each_persisted_checkpoint() {
        for unpark in [false, true] {
            for checkpoint in 0..=3 {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().canonicalize().unwrap();
                let home = root.join("home");
                let active = home.join(".agents/skills/sample");
                let reader = home.join(".claude/skills/sample");
                write_skill_tree(&active);
                fs::create_dir_all(reader.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink("../../.agents/skills/sample", &reader).unwrap();
                fs::write(
                    home.join(".agents/skill-studio.json"),
                    br#"{"version":4,"future":{"keep":true}}"#,
                )
                .unwrap();
                let original = fs::read(active.join("SKILL.md")).unwrap();
                let mut service = ScopedSkillService::bind(scope(&home)).unwrap();
                let selected = selected_deployment(&mut service, &active);
                let store = EventStore::open(&root.join("events")).unwrap();
                let request = ParkSkillRequest {
                    deployment_id: selected.id,
                    source_kind: SourceKind::Manual,
                    parked_at: "2026-09-17T00:00:00Z".into(),
                };
                let id = crate::skill_event_store::allocate_id();
                let (intent, lease) = if unpark {
                    let parked = park_skill(
                        &mut service,
                        &store,
                        &request,
                        LIMITS,
                        TIMEOUT,
                        CancellationToken::default(),
                    )
                    .unwrap();
                    prepare_unpark(
                        &mut service,
                        &store,
                        &UnparkSkillRequest {
                            deployment_id: parked.deployment_id,
                        },
                        &id,
                        LIMITS,
                        TIMEOUT,
                        CancellationToken::default(),
                    )
                    .unwrap()
                } else {
                    prepare_park(
                        &mut service,
                        &store,
                        &request,
                        &id,
                        LIMITS,
                        TIMEOUT,
                        CancellationToken::default(),
                    )
                    .unwrap()
                };
                record_intent(&store, &lease, &intent).unwrap();
                drop(lease);
                if unpark {
                    if checkpoint >= 1 {
                        fs::rename(&intent.parked_path, &intent.active_path).unwrap();
                    }
                    if checkpoint >= 2 {
                        fs::rename(&intent.staged_reader_path, &intent.reader_path).unwrap();
                    }
                } else {
                    if checkpoint >= 1 {
                        fs::create_dir_all(intent.staged_reader_path.parent().unwrap()).unwrap();
                        fs::rename(&intent.reader_path, &intent.staged_reader_path).unwrap();
                    }
                    if checkpoint >= 2 {
                        fs::create_dir_all(intent.parked_path.parent().unwrap()).unwrap();
                        fs::rename(&intent.active_path, &intent.parked_path).unwrap();
                    }
                }
                if checkpoint == 3 {
                    let before = fs::read(&intent.registry_path).unwrap();
                    fs::write(
                        &intent.registry_path,
                        intent.registry.apply(&before).unwrap(),
                    )
                    .unwrap();
                }
                drop(service);
                let mut restarted = ScopedSkillService::bind(scope(&home)).unwrap();
                let row = store.get(&id).unwrap().unwrap();
                recover_park_operation(&mut restarted, &store, &row, LIMITS, TIMEOUT)
                    .unwrap_or_else(|error| {
                        panic!("unpark={unpark}, checkpoint={checkpoint}: {error}")
                    });
                let final_path = if unpark {
                    &intent.active_path
                } else {
                    &intent.parked_path
                };
                assert_eq!(fs::read(final_path.join("SKILL.md")).unwrap(), original);
                assert_eq!(
                    fs::read(final_path.join("resource.txt")).unwrap(),
                    b"resource"
                );
                assert_eq!(
                    fs::read_link(final_path.join("broken-link")).unwrap(),
                    PathBuf::from("missing-target")
                );
                assert_eq!(reader.is_symlink(), unpark);
                assert_eq!(store.get(&id).unwrap().unwrap().status, "done");
                assert!(store
                    .interrupted_events_of_kind(intent.event_kind())
                    .unwrap()
                    .is_empty());
                let registry: Value =
                    serde_json::from_slice(&fs::read(&intent.registry_path).unwrap()).unwrap();
                assert_eq!(registry["future"]["keep"], true);
                assert_eq!(registry["parked"].get("sample").is_some(), !unpark);
            }
        }
    }

    #[test]
    fn recovery_preserves_changed_content_and_leaves_intent_unresolved() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let active = home.join(".agents/skills/sample");
        write_skill_tree(&active);
        let mut service = ScopedSkillService::bind(scope(&home)).unwrap();
        let selected = selected_deployment(&mut service, &active);
        let store = EventStore::open(&root.join("events")).unwrap();
        let id = crate::skill_event_store::allocate_id();
        let (intent, lease) = prepare_park(
            &mut service,
            &store,
            &ParkSkillRequest {
                deployment_id: selected.id,
                source_kind: SourceKind::Manual,
                parked_at: "2026-09-17T00:00:00Z".into(),
            },
            &id,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        record_intent(&store, &lease, &intent).unwrap();
        drop(lease);
        fs::write(active.join("resource.txt"), "external change").unwrap();
        let row = store.get(&id).unwrap().unwrap();
        assert!(recover_park_operation(&mut service, &store, &row, LIMITS, TIMEOUT).is_err());
        assert_eq!(
            fs::read(active.join("resource.txt")).unwrap(),
            b"external change"
        );
        assert!(!intent.parked_path.exists());
        assert_eq!(store.get(&id).unwrap().unwrap().status, "pending");
    }

    #[test]
    fn recovery_refuses_name_only_intent_without_exact_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let store = EventStore::open(&home.join("state")).unwrap();
        store
            .record(
                "name-only",
                EventDraft {
                    kind: PARK_EVENT_KIND.into(),
                    skill: "sample".into(),
                    harness: None,
                    scope: Some("global".into()),
                    project_path: None,
                    payload: json!({"version": 1, "name": "sample"}),
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
        let row = store.get("name-only").unwrap().unwrap();
        assert!(ParkIntent::from_event(&home, &row).is_err());
        assert_eq!(store.get("name-only").unwrap().unwrap().status, "pending");
    }

    #[test]
    fn raw_trial_retarget_preserves_unknown_fields_and_refuses_ambiguity() {
        let source = PathBuf::from("/fixture/.agents/skills/sample");
        let destination = PathBuf::from("/fixture/.agents/skills-parked/sample");
        let mut registry = ForkRegistry::default();
        registry.trials.insert(
            "global/sample".into(),
            TrialRecord {
                deployment_id: String::new(),
                started_at: "start".into(),
                expires_at: "end".into(),
                status: TrialStatus::Active,
                method: AddMethod::Copy,
                scope: TrialScope::Global,
                project_path: None,
                skill_dir: source.clone(),
                deployment_fingerprint: "tree".into(),
                claude_link: None,
                claude_link_target: None,
            },
        );
        let document = json!({"trials":{"global/sample":{
            "started_at":"start","expires_at":"end","status":"active","method":"copy",
            "scope":"global","skill_dir":source,"deployment_fingerprint":"tree",
            "claude_link":null,"claude_link_target":null,"future":{"keep":true}
        }}});
        let transition = selected_trial_transition(
            &registry,
            &document,
            "sample",
            Path::new("/fixture/.agents/skills/sample"),
            "active",
            &destination,
            "parked",
            None,
            None,
        )
        .unwrap()
        .unwrap();
        let registry_transition =
            ParkRegistryTransition::park("sample", json!({"parked_at":"now"}), Some(transition))
                .unwrap();
        let after: Value = serde_json::from_slice(
            &registry_transition
                .apply(&serde_json::to_vec(&document).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(after["trials"]["deployment/parked"]["future"]["keep"], true);
        let mut duplicate = registry.trials["global/sample"].clone();
        duplicate.deployment_id = "active".into();
        registry
            .trials
            .insert("deployment/active".into(), duplicate);
        assert!(selected_trial_transition(
            &registry,
            &document,
            "sample",
            &source,
            "active",
            &destination,
            "parked",
            None,
            None
        )
        .unwrap_err()
        .contains("ambiguous"));
    }

    #[test]
    fn malformed_event_cannot_settle_from_folder_presence() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        fs::create_dir_all(home.join(".agents/skills/sample")).unwrap();
        fs::write(home.join(".agents/skills/sample/SKILL.md"), "body").unwrap();
        let store = EventStore::open(&home.join("state")).unwrap();
        store
            .record(
                "unbound",
                EventDraft {
                    kind: UNPARK_EVENT_KIND.into(),
                    skill: "sample".into(),
                    harness: None,
                    scope: Some("global".into()),
                    project_path: None,
                    payload: json!({"version":1,"name":"sample"}),
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
        let row = store.get("unbound").unwrap().unwrap();
        assert!(ParkIntent::from_event(&home, &row).is_err());
        assert_eq!(store.get("unbound").unwrap().unwrap().status, "pending");
        assert_eq!(
            fs::read(home.join(".agents/skills/sample/SKILL.md")).unwrap(),
            b"body"
        );
    }
}
