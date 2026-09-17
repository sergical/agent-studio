//! Direct document repair composition. The compatibility SQLite connection is
//! validated but its sidecar IO is not yet confined; adapters must not expose
//! this as the completed cross-process mutation service.
use crate::{
    skill_backup_reservation::{BackupCopyLimits, BackupStateRoot},
    skill_backup_source::BackupSourceRoot,
    skill_document_target::SkillDocumentTarget,
    skill_event::{EventDraft, EventStatus, InverseOp},
    skill_event_operations::GuardedEventStore,
    skill_event_store::{fingerprint_regular_bytes, EventStore},
    skill_frontmatter_repair::FrontmatterRepairApplyMode,
    skill_repair_intent::FrontmatterRepairIntent,
    skill_service::PreparedRepairSelection,
};
use std::{ffi::OsStr, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairExecutionStage {
    Prepare,
    Backup,
    Intent,
    Document,
    Registry,
    Finish,
    Recover,
}

#[derive(Debug)]
pub struct RepairExecutionError {
    pub event_id: String,
    pub stage: RepairExecutionStage,
    pub message: String,
}
impl std::fmt::Display for RepairExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Repair {} at {:?}: {}",
            self.event_id, self.stage, self.message
        )
    }
}
impl std::error::Error for RepairExecutionError {}

#[derive(Debug)]
pub struct RepairExecutionReceipt {
    pub event_id: String,
    pub deployment_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairRecoveryOutcome {
    NotApplied,
    Applied,
}

#[derive(Debug)]
pub enum RepairRecoveryStep {
    Idle,
    Resolved {
        event_id: String,
        outcome: RepairRecoveryOutcome,
    },
}

/// Processes at most one oldest unresolved event. Unsupported or conflicting
/// events remain unresolved; callers must not skip them to admit a new mutation.
pub fn recover_next_repair(
    service: &mut crate::skill_service::ScopedSkillService,
    store: &EventStore,
    timeout: Option<std::time::Duration>,
    cancellation: crate::skill_service::CancellationToken,
) -> Result<RepairRecoveryStep, RepairExecutionError> {
    use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
    let failure = |message: String| RepairExecutionError {
        event_id: "recovery".into(),
        stage: RepairExecutionStage::Prepare,
        message,
    };
    let (row, undo_recovery, redo_recovery) = {
        let scope = crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&store.app_data))
            .map_err(|error| failure(error.to_string()))?;
        let lease = CoordinationPlan::new_cancellable(
            vec![DirectoryEffect::tree(
                &store.app_data,
                CoordinationMode::Exclusive,
            )],
            timeout,
            cancellation.clone(),
        )
        .and_then(|plan| plan.acquire())
        .and_then(|guard| guard.finalize_write(&scope, &[]))
        .map_err(|error| failure(error.to_string()))?;
        let events = GuardedEventStore::bind(store, &lease).map_err(failure)?;
        let row = events.next_recovery_event(&lease).map_err(failure)?;
        let undo = row
            .as_ref()
            .filter(|row| row.kind == "undo_copy_frontmatter")
            .map(|row| events.read_copy_undo_recovery(&lease, row))
            .transpose()
            .map_err(failure)?;
        let redo = row
            .as_ref()
            .filter(|row| row.kind == "redo_copy_frontmatter")
            .map(|row| events.read_copy_redo_recovery(&lease, row))
            .transpose()
            .map_err(failure)?;
        (row, undo, redo)
    };
    let Some(row) = row else {
        return Ok(RepairRecoveryStep::Idle);
    };
    let preparation_error =
        |error: crate::skill_service::WritePreparationError| RepairExecutionError {
            event_id: row.id.clone(),
            stage: RepairExecutionStage::Prepare,
            message: error.to_string(),
        };
    let outcome = match row.kind.as_str() {
        "repair_skill_frontmatter" => {
            let prepared = service
                .prepare_repair_event_recovery(
                    &row,
                    std::slice::from_ref(&store.app_data),
                    timeout,
                    cancellation,
                )
                .map_err(preparation_error)?;
            recover_direct_repair(prepared, store)?
        }
        "repair_copy_frontmatter" => {
            let prepared = service
                .prepare_copy_repair_recovery(&row, store, timeout, cancellation)
                .map_err(preparation_error)?;
            recover_copy_repair(prepared, store)?
        }
        "undo_copy_frontmatter" => {
            let event = undo_recovery
                .ok_or_else(|| failure("Copy undo recovery pair is missing".into()))?;
            let prepared = service
                .prepare_copy_undo_recovery(
                    event.source(),
                    event.undo(),
                    store,
                    timeout,
                    cancellation,
                )
                .map_err(preparation_error)?;
            recover_copy_undo(prepared, store)?
        }
        "redo_copy_frontmatter" => {
            let event = redo_recovery
                .ok_or_else(|| failure("Copy redo recovery chain is missing".into()))?;
            let prepared = service
                .prepare_copy_redo_recovery(
                    event.source(),
                    event.undo(),
                    event.redo(),
                    store,
                    timeout,
                    cancellation,
                )
                .map_err(preparation_error)?;
            recover_copy_redo(prepared, store)?
        }
        _ => {
            return Err(RepairExecutionError {
                event_id: row.id.clone(),
                stage: RepairExecutionStage::Prepare,
                message: "Unresolved event requires a different recovery protocol".into(),
            })
        }
    };
    Ok(RepairRecoveryStep::Resolved {
        event_id: row.id,
        outcome,
    })
}

/// Resolves an interrupted direct repair without replaying a document write.
/// Original bytes close the attempt as failed; proposed bytes close it as done.
/// The compatibility SQLite sidecar and non-cooperating writer limits still apply.
pub fn recover_direct_repair(
    prepared: crate::skill_service::PreparedRepairEventRecovery<'_>,
    store: &EventStore,
) -> Result<RepairRecoveryOutcome, RepairExecutionError> {
    recover_direct_repair_using(prepared, &CompatibilityRepairEvents(store))
}

pub(crate) fn recover_direct_repair_using(
    prepared: crate::skill_service::PreparedRepairEventRecovery<'_>,
    events: &impl RepairEvents,
) -> Result<RepairRecoveryOutcome, RepairExecutionError> {
    let (event, recovery) = prepared.into_parts();
    let error = |stage, message: String| RepairExecutionError {
        event_id: event.id().to_owned(),
        stage,
        message,
    };
    let (deployment, state, lease) = recovery.into_parts();
    if event.intent().mode == FrontmatterRepairApplyMode::ForkAndFix
        || deployment.owner_kind == crate::skill_ownership::LifecycleOwnerKind::Copy
    {
        return Err(error(
            RepairExecutionStage::Prepare,
            "Recovery requires the complete ownership transaction for this repair".into(),
        ));
    }
    let backup =
        crate::skill_repair_backup::VerifiedRepairBackup::read(events.state_root(), &event, &lease)
            .map_err(|message| error(RepairExecutionStage::Backup, message))?;
    let (outcome, status, post) = match state {
        crate::skill_service::RepairRecoveryDocumentState::Original => {
            (RepairRecoveryOutcome::NotApplied, EventStatus::Failed, None)
        }
        crate::skill_service::RepairRecoveryDocumentState::Proposed => (
            RepairRecoveryOutcome::Applied,
            EventStatus::Done,
            Some(fingerprint_regular_bytes(
                event.intent().proposed_content.as_bytes(),
            )),
        ),
    };
    let inverse = serde_json::to_value(InverseOp::RestoreBackup {
        path: event.intent().path.join("SKILL.md"),
        pre_fingerprint: event.original_file_fingerprint().into(),
        post_fingerprint: post,
    })
    .map_err(|failure| error(RepairExecutionStage::Finish, failure.to_string()))?;
    backup
        .revalidate(&lease)
        .map_err(|message| error(RepairExecutionStage::Backup, message))?;
    let (_lease, result) = events.recover(lease, &event, status, inverse);
    result.map_err(|message| error(RepairExecutionStage::Finish, message))?;
    Ok(outcome)
}

pub fn recover_copy_repair(
    prepared: crate::skill_service::PreparedCopyRepairRecovery<'_>,
    store: &EventStore,
) -> Result<RepairRecoveryOutcome, RepairExecutionError> {
    recover_copy_repair_with(prepared, store, || {})
}

fn recover_copy_repair_with(
    mut prepared: crate::skill_service::PreparedCopyRepairRecovery<'_>,
    store: &EventStore,
    mut after_registry: impl FnMut(),
) -> Result<RepairRecoveryOutcome, RepairExecutionError> {
    use crate::skill_copy_repair::CopyRepairObservedState;
    let event_id = prepared.event.id().to_owned();
    let error = |stage, message: String| RepairExecutionError {
        event_id: event_id.clone(),
        stage,
        message,
    };
    prepared
        .revalidate()
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let events = GuardedEventStore::bind(store, &prepared.lease)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    events
        .validate_copy_recovery(&prepared.lease, &prepared.event)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    if prepared.state == CopyRepairObservedState::DocumentApplied {
        let intent = prepared.event.intent();
        let parent = intent.registry_path.parent().ok_or_else(|| {
            error(
                RepairExecutionStage::Registry,
                "Registry parent missing".into(),
            )
        })?;
        let target = crate::skill_document_target::SkillRegistryTarget::bind(parent)
            .map_err(|message| error(RepairExecutionStage::Registry, message))?;
        let proposed = intent
            .transition
            .apply_document(&prepared.registry_original)
            .map_err(|message| error(RepairExecutionStage::Registry, message))?;
        target
            .replace(&mut prepared.lease, &prepared.registry_original, &proposed)
            .map_err(|failure| error(RepairExecutionStage::Registry, failure.to_string()))?;
        after_registry();
    }
    prepared
        .revalidate()
        .map_err(|message| error(RepairExecutionStage::Finish, message))?;
    let (status, outcome) = if prepared.state == CopyRepairObservedState::Original {
        (EventStatus::Failed, RepairRecoveryOutcome::NotApplied)
    } else {
        (EventStatus::Done, RepairRecoveryOutcome::Applied)
    };
    events
        .finish_copy_recovery(&prepared.lease, &prepared.event, status)
        .map_err(|failure| error(RepairExecutionStage::Finish, failure.to_string()))?;
    Ok(outcome)
}

pub fn recover_copy_redo(
    prepared: crate::skill_service::PreparedCopyRedoRecovery<'_>,
    store: &EventStore,
) -> Result<RepairRecoveryOutcome, RepairExecutionError> {
    recover_copy_redo_with(prepared, store, || {})
}

fn recover_copy_redo_with(
    mut prepared: crate::skill_service::PreparedCopyRedoRecovery<'_>,
    store: &EventStore,
    mut after_registry: impl FnMut(),
) -> Result<RepairRecoveryOutcome, RepairExecutionError> {
    use crate::skill_copy_repair::CopyRepairObservedState;
    let event_id = prepared.event.redo().id.clone();
    let error = |stage, message: String| RepairExecutionError {
        event_id: event_id.clone(),
        stage,
        message,
    };
    prepared
        .revalidate()
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let events = GuardedEventStore::bind(store, &prepared.lease)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    events
        .validate_copy_redo_recovery(&prepared.lease, &prepared.event)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    if prepared.state == CopyRepairObservedState::DocumentApplied {
        let intent = &prepared.event.intent().repair;
        let parent = intent.registry_path.parent().ok_or_else(|| {
            error(
                RepairExecutionStage::Registry,
                "Registry parent missing".into(),
            )
        })?;
        let target = crate::skill_document_target::SkillRegistryTarget::bind(parent)
            .map_err(|message| error(RepairExecutionStage::Registry, message))?;
        let proposed = intent
            .transition
            .apply_document(&prepared.registry_original)
            .map_err(|message| error(RepairExecutionStage::Registry, message))?;
        target
            .replace(&mut prepared.lease, &prepared.registry_original, &proposed)
            .map_err(|failure| error(RepairExecutionStage::Registry, failure.to_string()))?;
        after_registry();
    }
    prepared
        .revalidate()
        .map_err(|message| error(RepairExecutionStage::Finish, message))?;
    let outcome = if prepared.state == CopyRepairObservedState::Original {
        RepairRecoveryOutcome::NotApplied
    } else {
        RepairRecoveryOutcome::Applied
    };
    events
        .finish_copy_redo_recovery(
            &prepared.lease,
            &prepared.event,
            prepared.state != CopyRepairObservedState::Original,
        )
        .map_err(|failure| error(RepairExecutionStage::Finish, failure.to_string()))?;
    Ok(outcome)
}

pub fn recover_copy_undo(
    prepared: crate::skill_service::PreparedCopyUndoRecovery<'_>,
    store: &EventStore,
) -> Result<RepairRecoveryOutcome, RepairExecutionError> {
    recover_copy_undo_with(prepared, store, || {})
}

fn recover_copy_undo_with(
    mut prepared: crate::skill_service::PreparedCopyUndoRecovery<'_>,
    store: &EventStore,
    mut after_registry: impl FnMut(),
) -> Result<RepairRecoveryOutcome, RepairExecutionError> {
    use crate::skill_copy_repair::CopyUndoObservedState;
    let event_id = prepared.event.undo().id.clone();
    let error = |stage, message: String| RepairExecutionError {
        event_id: event_id.clone(),
        stage,
        message,
    };
    prepared
        .revalidate()
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let events = GuardedEventStore::bind(store, &prepared.lease)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    events
        .validate_copy_undo_recovery(&prepared.lease, &prepared.event)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    if prepared.state == CopyUndoObservedState::DocumentRestored {
        let intent = prepared.event.intent();
        let parent = intent.registry_path.parent().ok_or_else(|| {
            error(
                RepairExecutionStage::Registry,
                "Registry parent missing".into(),
            )
        })?;
        let target = crate::skill_document_target::SkillRegistryTarget::bind(parent)
            .map_err(|message| error(RepairExecutionStage::Registry, message))?;
        let restored = intent
            .transition
            .roll_back_document(&prepared.registry_original)
            .map_err(|message| error(RepairExecutionStage::Registry, message))?;
        target
            .replace(&mut prepared.lease, &prepared.registry_original, &restored)
            .map_err(|failure| error(RepairExecutionStage::Registry, failure.to_string()))?;
        after_registry();
    }
    prepared
        .revalidate()
        .map_err(|message| error(RepairExecutionStage::Finish, message))?;
    let applied = prepared.state != CopyUndoObservedState::Unchanged;
    events
        .finish_copy_undo_recovery(&prepared.lease, &prepared.event, applied)
        .map_err(|failure| error(RepairExecutionStage::Finish, failure.to_string()))?;
    Ok(if applied {
        RepairRecoveryOutcome::Applied
    } else {
        RepairRecoveryOutcome::NotApplied
    })
}

/// Experimental two-file undo. Adapter integration and independent review remain
/// required before this entry point is exposed to users.
pub fn execute_copy_repair_undo(
    prepared: crate::skill_service::PreparedCopyRepairUndo<'_>,
    store: &EventStore,
    undo_id: &str,
) -> Result<RepairExecutionReceipt, RepairExecutionError> {
    execute_copy_repair_undo_with(prepared, store, undo_id, |_| {})
}

fn execute_copy_repair_undo_with(
    mut prepared: crate::skill_service::PreparedCopyRepairUndo<'_>,
    store: &EventStore,
    undo_id: &str,
    mut checkpoint: impl FnMut(RepairExecutionStage),
) -> Result<RepairExecutionReceipt, RepairExecutionError> {
    let error = |stage, message: String| RepairExecutionError {
        event_id: undo_id.into(),
        stage,
        message,
    };
    prepared
        .revalidate()
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let events = GuardedEventStore::bind(store, &prepared.lease)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    events
        .require_recovered(&prepared.lease)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let intent = prepared.source.intent();
    let directory = &intent.document.path;
    let registry_path = &intent.registry_path;
    let registry_parent = registry_path.parent().ok_or_else(|| {
        error(
            RepairExecutionStage::Prepare,
            "Registry parent missing".into(),
        )
    })?;
    let document_target = SkillDocumentTarget::bind(directory)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let registry_target = crate::skill_document_target::SkillRegistryTarget::bind(registry_parent)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let mut sources = Vec::new();
    for (parent, name) in [
        (directory.as_path(), "SKILL.md"),
        (registry_parent, "skill-studio.json"),
    ] {
        sources.push(
            BackupSourceRoot::bind(parent)
                .and_then(|root| root.select(OsStr::new(name)))
                .map_err(|failure| error(RepairExecutionStage::Backup, failure.to_string()))?,
        );
    }
    let (document_before, document_after) = prepared.plan.document_change();
    let (registry_before, registry_after) = prepared.plan.registry_change();
    let state = BackupStateRoot::bind(&store.app_data)
        .map_err(|failure| error(RepairExecutionStage::Backup, failure.to_string()))?;
    let manifest = prepared
        .lease
        .backup_documents(
            &state,
            undo_id,
            sources,
            BackupCopyLimits {
                max_bytes: (document_before.len() + registry_before.len()) as u64,
                max_entries: 2,
                max_depth: 0,
            },
        )
        .map_err(|message| error(RepairExecutionStage::Backup, message))?;
    for (path, bytes) in [
        (directory.join("SKILL.md"), document_before),
        (registry_path.clone(), registry_before),
    ] {
        if manifest
            .entries
            .get(path.to_string_lossy().as_ref())
            .is_none_or(|entry| entry.fingerprint != fingerprint_regular_bytes(bytes))
        {
            return Err(error(
                RepairExecutionStage::Backup,
                "Undo backup does not match current bytes".into(),
            ));
        }
    }
    let recorded = events
        .record_copy_undo(&prepared.lease, &prepared.source, undo_id)
        .map_err(|failure| error(RepairExecutionStage::Intent, failure.to_string()))?;
    checkpoint(RepairExecutionStage::Intent);
    document_target
        .replace(&mut prepared.lease, document_before, document_after)
        .map_err(|failure| error(RepairExecutionStage::Document, failure.to_string()))?;
    checkpoint(RepairExecutionStage::Document);
    registry_target
        .replace(&mut prepared.lease, registry_before, registry_after)
        .map_err(|failure| error(RepairExecutionStage::Registry, failure.to_string()))?;
    checkpoint(RepairExecutionStage::Registry);
    events
        .finish_copy_undo(&prepared.lease, &recorded)
        .map_err(|failure| error(RepairExecutionStage::Finish, failure.to_string()))?;
    Ok(RepairExecutionReceipt {
        event_id: undo_id.into(),
        deployment_id: intent.document.deployment_id.clone(),
    })
}

/// Experimental redo; adapter integration and independent review remain open.
pub fn execute_copy_repair_redo(
    prepared: crate::skill_service::PreparedCopyRepairRedo<'_>,
    store: &EventStore,
    redo_id: &str,
) -> Result<RepairExecutionReceipt, RepairExecutionError> {
    execute_copy_repair_redo_with(prepared, store, redo_id, |_| {})
}

fn execute_copy_repair_redo_with(
    mut prepared: crate::skill_service::PreparedCopyRepairRedo<'_>,
    store: &EventStore,
    redo_id: &str,
    mut checkpoint: impl FnMut(RepairExecutionStage),
) -> Result<RepairExecutionReceipt, RepairExecutionError> {
    let error = |stage, message: String| RepairExecutionError {
        event_id: redo_id.into(),
        stage,
        message,
    };
    prepared
        .revalidate()
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let events = GuardedEventStore::bind(store, &prepared.lease)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    events
        .require_recovered(&prepared.lease)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let redo_intent = crate::skill_copy_repair::CopyRepairRedoIntent::from_prepared(&prepared)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let intent = &redo_intent.repair;
    let directory = &intent.document.path;
    let registry_path = &intent.registry_path;
    let registry_parent = registry_path.parent().ok_or_else(|| {
        error(
            RepairExecutionStage::Prepare,
            "Registry parent missing".into(),
        )
    })?;
    let document_target = SkillDocumentTarget::bind(directory)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let registry_target = crate::skill_document_target::SkillRegistryTarget::bind(registry_parent)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let mut sources = Vec::new();
    for (parent, name) in [
        (directory.as_path(), "SKILL.md"),
        (registry_parent, "skill-studio.json"),
    ] {
        sources.push(
            BackupSourceRoot::bind(parent)
                .and_then(|root| root.select(OsStr::new(name)))
                .map_err(|failure| error(RepairExecutionStage::Backup, failure.to_string()))?,
        );
    }
    let (document_before, document_after) = prepared.plan.document_change();
    let (registry_before, registry_after) = prepared.plan.registry_change();
    let state = BackupStateRoot::bind(&store.app_data)
        .map_err(|failure| error(RepairExecutionStage::Backup, failure.to_string()))?;
    let manifest = prepared
        .lease
        .backup_documents(
            &state,
            redo_id,
            sources,
            BackupCopyLimits {
                max_bytes: (document_before.len() + registry_before.len()) as u64,
                max_entries: 2,
                max_depth: 0,
            },
        )
        .map_err(|message| error(RepairExecutionStage::Backup, message))?;
    for (path, bytes) in [
        (directory.join("SKILL.md"), document_before),
        (registry_path.clone(), registry_before),
    ] {
        if manifest
            .entries
            .get(path.to_string_lossy().as_ref())
            .is_none_or(|entry| entry.fingerprint != fingerprint_regular_bytes(bytes))
        {
            return Err(error(
                RepairExecutionStage::Backup,
                "Redo backup does not match current bytes".into(),
            ));
        }
    }
    let recorded = events
        .record_copy_redo(&prepared.lease, &prepared.source, &redo_intent, redo_id)
        .map_err(|failure| error(RepairExecutionStage::Intent, failure.to_string()))?;
    checkpoint(RepairExecutionStage::Intent);
    document_target
        .replace(&mut prepared.lease, document_before, document_after)
        .map_err(|failure| error(RepairExecutionStage::Document, failure.to_string()))?;
    checkpoint(RepairExecutionStage::Document);
    registry_target
        .replace(&mut prepared.lease, registry_before, registry_after)
        .map_err(|failure| error(RepairExecutionStage::Registry, failure.to_string()))?;
    checkpoint(RepairExecutionStage::Registry);
    events
        .finish_copy_redo(&prepared.lease, &recorded)
        .map_err(|failure| error(RepairExecutionStage::Finish, failure.to_string()))?;
    Ok(RepairExecutionReceipt {
        event_id: redo_id.into(),
        deployment_id: intent.document.deployment_id.clone(),
    })
}

/// Experimental copy execution; adapters must wait for copy recovery and restore.
/// The event is deliberately non-restorable by the legacy single-target undo path.
pub fn execute_copy_repair(
    prepared: crate::skill_service::PreparedCopyRepairSelection<'_>,
    store: &EventStore,
    event_id: &str,
) -> Result<RepairExecutionReceipt, RepairExecutionError> {
    execute_copy_repair_with(prepared, store, event_id, |_| {})
}

fn execute_copy_repair_with(
    prepared: crate::skill_service::PreparedCopyRepairSelection<'_>,
    store: &EventStore,
    event_id: &str,
    checkpoint: impl FnMut(RepairExecutionStage),
) -> Result<RepairExecutionReceipt, RepairExecutionError> {
    execute_copy_repair_using(
        prepared,
        &CompatibilityRepairEvents(store),
        event_id,
        checkpoint,
    )
}

pub(crate) fn execute_copy_repair_using(
    prepared: crate::skill_service::PreparedCopyRepairSelection<'_>,
    events: &impl RepairEvents,
    event_id: &str,
    mut checkpoint: impl FnMut(RepairExecutionStage),
) -> Result<RepairExecutionReceipt, RepairExecutionError> {
    let error = |stage, message: String| RepairExecutionError {
        event_id: event_id.into(),
        stage,
        message,
    };
    prepared
        .revalidate()
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    events
        .ensure_running()
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let intent = crate::skill_copy_repair::CopyRepairIntent::from_prepared(&prepared)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let (selection, registry_path, registry_before, registry_after) =
        prepared.into_execution_parts();
    let (preview, _, mut lease) = selection.into_parts();
    let (returned, result) = events.prepare(lease, event_id);
    lease = returned;
    result.map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let skill_dir = PathBuf::from(&preview.path);
    let document_path = skill_dir.join("SKILL.md");
    let registry_parent = registry_path.parent().ok_or_else(|| {
        error(
            RepairExecutionStage::Prepare,
            "Registry parent missing".into(),
        )
    })?;
    let document = SkillDocumentTarget::bind(&skill_dir)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let registry = crate::skill_document_target::SkillRegistryTarget::bind(registry_parent)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let mut sources = Vec::new();
    for (parent, name) in [
        (&*skill_dir, "SKILL.md"),
        (registry_parent, "skill-studio.json"),
    ] {
        sources.push(
            BackupSourceRoot::bind(parent)
                .and_then(|root| root.select(OsStr::new(name)))
                .map_err(|failure| error(RepairExecutionStage::Backup, failure.to_string()))?,
        );
    }
    let state = BackupStateRoot::bind(events.state_root())
        .map_err(|failure| error(RepairExecutionStage::Backup, failure.to_string()))?;
    let manifest = lease
        .backup_documents(
            &state,
            event_id,
            sources,
            BackupCopyLimits {
                max_bytes: (preview.original_content.len() + registry_before.len()) as u64,
                max_entries: 2,
                max_depth: 0,
            },
        )
        .map_err(|message| error(RepairExecutionStage::Backup, message))?;
    for (path, bytes) in [
        (&document_path, preview.original_content.as_bytes()),
        (&registry_path, registry_before.as_slice()),
    ] {
        if manifest
            .entries
            .get(path.to_string_lossy().as_ref())
            .is_none_or(|entry| entry.fingerprint != fingerprint_regular_bytes(bytes))
        {
            return Err(error(
                RepairExecutionStage::Backup,
                "Copy repair backup does not match original bytes".into(),
            ));
        }
    }
    let selected = crate::skill_deployment::parse_deployment_id(&preview.deployment_id)
        .ok_or_else(|| {
            error(
                RepairExecutionStage::Intent,
                "Invalid copy deployment".into(),
            )
        })?;
    let (returned, result) = events.record(
        lease,
        event_id,
        chrono::Utc::now().to_rfc3339(),
        EventDraft {
            kind: "repair_copy_frontmatter".into(),
            skill: intent.document.name.clone(),
            harness: None,
            scope: Some(selected.scope),
            project_path: selected.project_path,
            payload: serde_json::to_value(&intent)
                .map_err(|failure| error(RepairExecutionStage::Intent, failure.to_string()))?,
            inverse: None,
            backup_dir: Some(format!("backups/{event_id}")),
            restorable: false,
        },
    );
    lease = returned;
    result.map_err(|message| error(RepairExecutionStage::Intent, message))?;
    checkpoint(RepairExecutionStage::Intent);
    events
        .ensure_running()
        .map_err(|message| error(RepairExecutionStage::Document, message))?;
    document
        .replace(
            &mut lease,
            preview.original_content.as_bytes(),
            preview.proposed_content.as_bytes(),
        )
        .map_err(|failure| error(RepairExecutionStage::Document, failure.to_string()))?;
    checkpoint(RepairExecutionStage::Document);
    events
        .ensure_running()
        .map_err(|message| error(RepairExecutionStage::Registry, message))?;
    registry
        .replace(&mut lease, &registry_before, &registry_after)
        .map_err(|failure| error(RepairExecutionStage::Registry, failure.to_string()))?;
    checkpoint(RepairExecutionStage::Registry);
    let (_lease, result) = events.finish(lease, event_id, None);
    result.map_err(|message| error(RepairExecutionStage::Finish, message))?;
    Ok(RepairExecutionReceipt {
        event_id: event_id.into(),
        deployment_id: preview.deployment_id,
    })
}

/// Executes document-only ApplyFix or FixInstalledCopy; copy-owned deployments
/// require a separate registry transaction. Any failure after the intent
/// attempt requires checking the event; pending intent is never discarded here.
/// The prepared lease must explicitly include the authorized store state tree.
pub fn execute_direct_repair(
    selection: PreparedRepairSelection<'_>,
    store: &EventStore,
    event_id: &str,
) -> Result<RepairExecutionReceipt, RepairExecutionError> {
    execute_direct_repair_with(selection, store, event_id, |_| {})
}

type RepairLease<'scope> = crate::skill_coordination::FinalizedWriteLease<'scope>;
type RepairEventResult<'scope> = (RepairLease<'scope>, Result<(), String>);

pub(crate) trait RepairEvents {
    fn recover<'scope>(
        &self,
        lease: RepairLease<'scope>,
        event: &crate::skill_repair_recovery_event::RepairRecoveryEvent,
        status: EventStatus,
        inverse: serde_json::Value,
    ) -> RepairEventResult<'scope>;
    fn state_root(&self) -> &std::path::Path;
    fn ensure_running(&self) -> Result<(), String> {
        Ok(())
    }
    fn preflight_record(
        &self,
        _event: &str,
        _timestamp: &str,
        _draft: &EventDraft,
    ) -> Result<(), String> {
        Ok(())
    }
    fn prepare<'scope>(&self, lease: RepairLease<'scope>, event: &str)
        -> RepairEventResult<'scope>;
    fn record<'scope>(
        &self,
        lease: RepairLease<'scope>,
        event: &str,
        timestamp: String,
        draft: EventDraft,
    ) -> RepairEventResult<'scope>;
    fn finish<'scope>(
        &self,
        lease: RepairLease<'scope>,
        event: &str,
        inverse: Option<serde_json::Value>,
    ) -> RepairEventResult<'scope>;
}

struct CompatibilityRepairEvents<'a>(&'a EventStore);
impl RepairEvents for CompatibilityRepairEvents<'_> {
    fn recover<'scope>(
        &self,
        lease: RepairLease<'scope>,
        event: &crate::skill_repair_recovery_event::RepairRecoveryEvent,
        status: EventStatus,
        inverse: serde_json::Value,
    ) -> RepairEventResult<'scope> {
        let result = GuardedEventStore::bind(self.0, &lease).and_then(|store| {
            store
                .finish_recovery(&lease, event, status, Some(inverse))
                .map_err(|error| error.to_string())
        });
        (lease, result)
    }
    fn state_root(&self) -> &std::path::Path {
        &self.0.app_data
    }
    fn prepare<'scope>(&self, lease: RepairLease<'scope>, _: &str) -> RepairEventResult<'scope> {
        let result = GuardedEventStore::bind(self.0, &lease)
            .and_then(|store| store.require_recovered(&lease));
        (lease, result)
    }
    fn record<'scope>(
        &self,
        lease: RepairLease<'scope>,
        event: &str,
        _timestamp: String,
        draft: EventDraft,
    ) -> RepairEventResult<'scope> {
        let result = GuardedEventStore::bind(self.0, &lease).and_then(|store| {
            store
                .record_pending(&lease, event, draft)
                .map_err(|error| error.to_string())
        });
        (lease, result)
    }
    fn finish<'scope>(
        &self,
        lease: RepairLease<'scope>,
        event: &str,
        inverse: Option<serde_json::Value>,
    ) -> RepairEventResult<'scope> {
        let result = GuardedEventStore::bind(self.0, &lease).and_then(|store| {
            store
                .finish_pending(&lease, event, EventStatus::Done, inverse)
                .map_err(|error| error.to_string())
        });
        (lease, result)
    }
}

fn execute_direct_repair_with(
    selection: PreparedRepairSelection<'_>,
    store: &EventStore,
    event_id: &str,
    checkpoint: impl FnMut(RepairExecutionStage),
) -> Result<RepairExecutionReceipt, RepairExecutionError> {
    execute_direct_repair_using(
        selection,
        &CompatibilityRepairEvents(store),
        event_id,
        checkpoint,
    )
}

pub(crate) fn execute_direct_repair_using(
    selection: PreparedRepairSelection<'_>,
    events: &impl RepairEvents,
    event_id: &str,
    mut checkpoint: impl FnMut(RepairExecutionStage),
) -> Result<RepairExecutionReceipt, RepairExecutionError> {
    let error = |stage, message: String| RepairExecutionError {
        event_id: event_id.to_owned(),
        stage,
        message,
    };
    if selection.owner_kind() == crate::skill_ownership::LifecycleOwnerKind::Copy {
        return Err(error(
            RepairExecutionStage::Prepare,
            "Copy repair requires a registry content-hash transaction".into(),
        ));
    }
    let (preview, mode, mut lease) = selection.into_parts();
    if mode == FrontmatterRepairApplyMode::ForkAndFix {
        return Err(error(
            RepairExecutionStage::Prepare,
            "Fork repair requires its ownership transaction".into(),
        ));
    }
    let intent = FrontmatterRepairIntent::from_preview(&preview, mode, None)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let parent = PathBuf::from(&preview.path);
    let document = parent.join("SKILL.md");
    let target = SkillDocumentTarget::bind(&parent)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let source = BackupSourceRoot::bind(&parent)
        .and_then(|root| root.select(OsStr::new("SKILL.md")))
        .map_err(|failure| error(RepairExecutionStage::Prepare, failure.to_string()))?;
    let state = BackupStateRoot::bind(events.state_root())
        .map_err(|failure| error(RepairExecutionStage::Prepare, failure.to_string()))?;
    let pre_fingerprint = fingerprint_regular_bytes(preview.original_content.as_bytes());
    let inverse = |post| {
        serde_json::to_value(InverseOp::RestoreBackup {
            path: document.clone(),
            pre_fingerprint: pre_fingerprint.clone(),
            post_fingerprint: post,
        })
        .map_err(|failure| failure.to_string())
    };
    let draft = EventDraft {
        kind: "repair_skill_frontmatter".into(),
        skill: intent.name.clone(),
        harness: None,
        scope: Some(preview.scope.clone()),
        project_path: crate::skill_deployment::parse_deployment_id(&preview.deployment_id)
            .and_then(|selected| selected.project_path),
        payload: serde_json::to_value(&intent)
            .map_err(|failure| error(RepairExecutionStage::Intent, failure.to_string()))?,
        inverse: Some(
            inverse(None).map_err(|message| error(RepairExecutionStage::Intent, message))?,
        ),
        backup_dir: Some(format!("backups/{event_id}")),
        restorable: true,
    };
    let timestamp = chrono::Utc::now().to_rfc3339();
    events
        .preflight_record(event_id, &timestamp, &draft)
        .map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let (returned, result) = events.prepare(lease, event_id);
    lease = returned;
    result.map_err(|message| error(RepairExecutionStage::Prepare, message))?;
    let manifest = lease
        .backup_documents(
            &state,
            event_id,
            vec![source],
            BackupCopyLimits {
                max_bytes: preview.original_content.len() as u64,
                max_entries: 1,
                max_depth: 0,
            },
        )
        .map_err(|message| error(RepairExecutionStage::Backup, message))?;
    if manifest
        .entries
        .get(&document.to_string_lossy().into_owned())
        .is_none_or(|entry| entry.fingerprint != pre_fingerprint)
    {
        return Err(error(
            RepairExecutionStage::Backup,
            "Repair backup does not match the validated document".into(),
        ));
    }
    let (returned, result) = events.record(lease, event_id, timestamp, draft);
    lease = returned;
    result.map_err(|message| error(RepairExecutionStage::Intent, message))?;
    checkpoint(RepairExecutionStage::Intent);
    events
        .ensure_running()
        .map_err(|message| error(RepairExecutionStage::Document, message))?;
    target
        .replace(
            &mut lease,
            preview.original_content.as_bytes(),
            preview.proposed_content.as_bytes(),
        )
        .map_err(|failure| error(RepairExecutionStage::Document, failure.to_string()))?;
    checkpoint(RepairExecutionStage::Document);
    let post = fingerprint_regular_bytes(preview.proposed_content.as_bytes());
    let (_lease, result) = events.finish(
        lease,
        event_id,
        Some(inverse(Some(post)).map_err(|message| error(RepairExecutionStage::Finish, message))?),
    );
    result.map_err(|message| error(RepairExecutionStage::Finish, message))?;
    Ok(RepairExecutionReceipt {
        event_id: event_id.to_owned(),
        deployment_id: preview.deployment_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        skill_deployment::{deployment_id, SkillDestination},
        skill_fork_registry::{write_fork_registry, ForkRecord, ForkRegistry, OriginTool},
        skill_frontmatter_repair::{preview_frontmatter_repair, BoundFrontmatterRepairRequest},
        skill_service::{CancellationToken, ScopedSkillService, SkillScope},
    };
    use std::{collections::BTreeSet, fs, time::Duration};

    struct Fixture {
        _temp: tempfile::TempDir,
        service: ScopedSkillService,
        store: EventStore,
        request: BoundFrontmatterRepairRequest,
        path: PathBuf,
        original: Vec<u8>,
    }
    impl Fixture {
        fn new(fork: bool) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().join("home");
            let skill = home.join(".agents/skills/alpha");
            fs::create_dir_all(&skill).unwrap();
            fs::create_dir(home.join(".git")).unwrap();
            let path = skill.join("SKILL.md");
            let original = b"---\nname: alpha\ndescription: this: fixture\n---\nbody\n".to_vec();
            fs::write(&path, &original).unwrap();
            if fork {
                let mut registry = ForkRegistry::default();
                registry.forks.insert(
                    "alpha".into(),
                    ForkRecord {
                        deployment_id: deployment_id(
                            "alpha",
                            "global",
                            SkillDestination::Universal,
                            "universal",
                            None,
                            &skill,
                        ),
                        skill_dir: skill.clone(),
                        forked_at: "now".into(),
                        origin_tool: OriginTool::SkillsSh,
                        origin_source: "owner/repo".into(),
                        repo: "owner/repo".into(),
                        path: "skills/alpha".into(),
                        declared_ref: None,
                        base_commit: "abc".into(),
                    },
                );
                write_fork_registry(&home, &registry).unwrap();
            } else {
                fs::write(home.join(".agents/.skill-lock.json"), r#"{"version":3,"skills":{"alpha":{"source":"owner/repo","sourceType":"github","sourceUrl":"https://example.test/owner/repo","skillFolderHash":"hash","installedAt":"2026-01-01","updatedAt":"2026-01-01"}}}"#).unwrap();
            }
            let store = EventStore::open(&temp.path().join("state")).unwrap();
            store
                .conn
                .pragma_update(None, "synchronous", "FULL")
                .unwrap();
            let mut service = ScopedSkillService::bind(SkillScope {
                home,
                projects: vec![],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            })
            .unwrap();
            let inventory = service
                .scan(
                    Some(&BTreeSet::from(["alpha".into()])),
                    Some(Duration::from_secs(10)),
                )
                .unwrap();
            let deployment = inventory
                .skills
                .iter()
                .flat_map(|skill| &skill.deployments)
                .find(|deployment| deployment.path == skill.to_str().unwrap())
                .unwrap();
            let preview = preview_frontmatter_repair(deployment, &original).unwrap();
            let request = BoundFrontmatterRepairRequest {
                deployment_id: deployment.id.clone(),
                proposal_id: preview.proposal_id,
                expected_content_fingerprint: preview.expected_content_fingerprint,
                mode: if fork {
                    FrontmatterRepairApplyMode::ApplyFix
                } else {
                    FrontmatterRepairApplyMode::FixInstalledCopy
                },
            };
            Self {
                _temp: temp,
                service,
                store,
                request,
                path,
                original,
            }
        }
    }

    fn backup_event(f: &mut Fixture) -> crate::skill_repair_recovery_event::RepairRecoveryEvent {
        let selection = f
            .service
            .prepare_repair_selection(
                &f.request,
                std::slice::from_ref(&f.store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        execute_direct_repair(selection, &f.store, "backup-check").unwrap();
        let mut row = f.store.get("backup-check").unwrap().unwrap();
        row.status = "pending".into();
        crate::skill_repair_recovery_event::RepairRecoveryEvent::from_row(&row).unwrap()
    }

    #[test]
    fn recovery_backup_rejects_invalid_artifacts() {
        use crate::skill_repair_backup::VerifiedRepairBackup;
        for case in [
            "malformed",
            "relative-path",
            "fingerprint",
            "bytes",
            "manifest-link",
            "document-link",
            "hardlink",
            "oversize",
        ] {
            let mut f = Fixture::new(false);
            let event = backup_event(&mut f);
            let directory = f.store.app_data.join("backups/backup-check");
            let manifest = directory.join("manifest.json");
            let document = directory.join("0-SKILL.md");
            match case {
                "malformed" => fs::write(&manifest, "{").unwrap(),
                "relative-path" | "fingerprint" => {
                    let mut value: crate::skill_event::BackupManifest =
                        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
                    let entry = value.entries.values_mut().next().unwrap();
                    if case == "relative-path" {
                        entry.relative_path = "../outside".into();
                    } else {
                        entry.fingerprint = "0".repeat(64);
                    }
                    fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                "bytes" => fs::write(&document, "changed").unwrap(),
                "manifest-link" | "document-link" => {
                    let path = if case == "manifest-link" {
                        &manifest
                    } else {
                        &document
                    };
                    let saved = directory.join("saved");
                    fs::rename(path, &saved).unwrap();
                    std::os::unix::fs::symlink(&saved, path).unwrap();
                }
                "hardlink" => fs::hard_link(&document, directory.join("alias")).unwrap(),
                "oversize" => fs::write(&manifest, vec![b' '; 64 * 1024 + 1]).unwrap(),
                _ => unreachable!(),
            }
            let prepared = f
                .service
                .prepare_repair_recovery(
                    event.intent(),
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let (_, _, lease) = prepared.into_parts();
            assert!(
                VerifiedRepairBackup::read(&f.store.app_data, &event, &lease).is_err(),
                "{case}"
            );
        }
    }

    #[test]
    fn recovery_backup_rechecks_files_and_parent_bindings() {
        use crate::skill_repair_backup::VerifiedRepairBackup;
        for case in ["manifest", "document", "operation", "container"] {
            let mut f = Fixture::new(false);
            let event = backup_event(&mut f);
            let prepared = f
                .service
                .prepare_repair_recovery(
                    event.intent(),
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let (_, _, lease) = prepared.into_parts();
            let backup = VerifiedRepairBackup::read(&f.store.app_data, &event, &lease).unwrap();
            assert_eq!(backup.original(), f.original);
            backup.revalidate(&lease).unwrap();
            let container = f.store.app_data.join("backups");
            let directory = container.join("backup-check");
            match case {
                "manifest" => fs::write(directory.join("manifest.json"), "{}").unwrap(),
                "document" => fs::write(directory.join("0-SKILL.md"), "changed").unwrap(),
                "operation" | "container" => {
                    let path = if case == "operation" {
                        directory
                    } else {
                        container
                    };
                    fs::rename(&path, path.with_extension("saved")).unwrap();
                    fs::create_dir(&path).unwrap();
                }
                _ => unreachable!(),
            }
            assert!(backup.revalidate(&lease).is_err(), "{case}");
        }
    }

    #[test]
    fn recovery_step_resolves_then_reports_idle_and_preserves_unknown_events() {
        let mut f = Fixture::new(false);
        backup_event(&mut f);
        f.store
            .conn
            .execute("UPDATE events SET status = 'interrupted'", [])
            .unwrap();
        let result = recover_next_repair(
            &mut f.service,
            &f.store,
            Some(Duration::from_secs(10)),
            CancellationToken::default(),
        )
        .unwrap();
        assert!(
            matches!(result, RepairRecoveryStep::Resolved { event_id, outcome: RepairRecoveryOutcome::Applied } if event_id == "backup-check")
        );
        assert!(matches!(
            recover_next_repair(
                &mut f.service,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .unwrap(),
            RepairRecoveryStep::Idle
        ));
        f.store
            .conn
            .execute(
                "UPDATE events SET status = 'interrupted', kind = 'unknown-operation'",
                [],
            )
            .unwrap();
        assert!(recover_next_repair(
            &mut f.service,
            &f.store,
            Some(Duration::from_secs(10)),
            CancellationToken::default()
        )
        .is_err());
        assert_eq!(
            f.store.get("backup-check").unwrap().unwrap().status,
            "interrupted"
        );
        let cancel = CancellationToken::default();
        cancel.cancel();
        assert!(recover_next_repair(
            &mut f.service,
            &f.store,
            Some(Duration::from_secs(10)),
            cancel
        )
        .is_err());
        assert_eq!(
            f.store.get("backup-check").unwrap().unwrap().status,
            "interrupted"
        );
    }

    #[test]
    fn new_repair_requires_prior_recovery_before_backup() {
        for status in ["pending", "interrupted", "done", "failed"] {
            let mut f = Fixture::new(false);
            f.store
                .record(
                    "earlier",
                    EventDraft {
                        kind: "unknown-operation".into(),
                        skill: "other".into(),
                        harness: None,
                        scope: None,
                        project_path: None,
                        payload: serde_json::json!({}),
                        inverse: None,
                        backup_dir: None,
                        restorable: false,
                    },
                )
                .unwrap();
            f.store
                .conn
                .execute(
                    "UPDATE events SET status = ?1 WHERE id = 'earlier'",
                    [status],
                )
                .unwrap();
            let selection = f
                .service
                .prepare_repair_selection(
                    &f.request,
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let result = execute_direct_repair(selection, &f.store, "next");
            if matches!(status, "pending" | "interrupted") {
                assert_eq!(result.unwrap_err().stage, RepairExecutionStage::Prepare);
                assert!(!f.store.app_data.join("backups/next").exists());
                assert!(f.store.get("next").unwrap().is_none());
                assert_eq!(fs::read(&f.path).unwrap(), f.original);
            } else {
                result.unwrap();
            }
            assert_eq!(f.store.get("earlier").unwrap().unwrap().status, status);
        }
    }

    #[test]
    fn direct_recovery_refuses_drift_without_finishing_event() {
        for change in ["document", "backup", "event"] {
            let mut f = Fixture::new(false);
            backup_event(&mut f);
            f.store
                .conn
                .execute("UPDATE events SET status = 'interrupted'", [])
                .unwrap();
            let row = f.store.get("backup-check").unwrap().unwrap();
            let prepared = f
                .service
                .prepare_repair_event_recovery(
                    &row,
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            match change {
                "document" => fs::write(&f.path, "user edit").unwrap(),
                "backup" => fs::write(
                    f.store.app_data.join("backups/backup-check/0-SKILL.md"),
                    "changed",
                )
                .unwrap(),
                "event" => {
                    f.store
                        .conn
                        .execute("UPDATE events SET payload = '{}'", [])
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let before = fs::read(&f.path).unwrap();
            assert!(
                recover_direct_repair(prepared, &f.store).is_err(),
                "{change}"
            );
            assert_eq!(fs::read(&f.path).unwrap(), before);
            assert_eq!(
                f.store.get("backup-check").unwrap().unwrap().status,
                "interrupted"
            );
        }
    }

    #[test]
    fn recovery_completion_preserves_changed_events() {
        for status in ["pending", "interrupted"] {
            for change in ["none", "payload", "inverse", "claim", "status", "deleted"] {
                let mut f = Fixture::new(false);
                backup_event(&mut f);
                f.store
                    .conn
                    .execute(
                        "UPDATE events SET status = ?1 WHERE id = 'backup-check'",
                        [status],
                    )
                    .unwrap();
                let row = f.store.get("backup-check").unwrap().unwrap();
                let event = crate::skill_repair_recovery_event::RepairRecoveryEvent::from_row(&row)
                    .unwrap();
                let prepared = f
                    .service
                    .prepare_repair_recovery(
                        event.intent(),
                        std::slice::from_ref(&f.store.app_data),
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                let (_, _, lease) = prepared.into_parts();
                let guarded = GuardedEventStore::bind(&f.store, &lease).unwrap();
                let sql = match change {
                    "none" => None,
                    "payload" => Some("UPDATE events SET payload = '{}'"),
                    "inverse" => Some("UPDATE events SET inverse = NULL"),
                    "claim" => Some("UPDATE events SET reverted_by = 'another'"),
                    "status" => Some("UPDATE events SET status = 'failed'"),
                    "deleted" => Some("DELETE FROM events"),
                    _ => unreachable!(),
                };
                if let Some(sql) = sql {
                    f.store.conn.execute(sql, []).unwrap();
                }
                let before = serde_json::to_value(f.store.get(event.id()).unwrap()).unwrap();
                let result =
                    guarded.finish_recovery(&lease, &event, EventStatus::Done, row.inverse.clone());
                if change == "none" {
                    result.unwrap();
                    assert_eq!(f.store.get(event.id()).unwrap().unwrap().status, "done");
                    assert!(guarded
                        .finish_recovery(&lease, &event, EventStatus::Failed, None)
                        .is_err());
                    assert_eq!(f.store.get(event.id()).unwrap().unwrap().status, "done");
                } else {
                    assert!(result.is_err(), "{status}/{change}");
                    assert_eq!(
                        serde_json::to_value(f.store.get(event.id()).unwrap()).unwrap(),
                        before
                    );
                }
                assert!(f.store.conn.is_autocommit());
            }
        }
    }

    #[test]
    fn recovery_backup_requires_planned_state() {
        let mut f = Fixture::new(false);
        let event = backup_event(&mut f);
        let prepared = f
            .service
            .prepare_repair_recovery(
                event.intent(),
                &[],
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        let (_, _, lease) = prepared.into_parts();
        assert!(crate::skill_repair_backup::VerifiedRepairBackup::read(
            &f.store.app_data,
            &event,
            &lease
        )
        .is_err());
    }

    #[test]
    fn direct_modes_write_backup_intent_document_and_compatible_inverse() {
        for fork in [false, true] {
            let mut f = Fixture::new(fork);
            let selection = f
                .service
                .prepare_repair_selection(
                    &f.request,
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let proposed = selection.preview().proposed_content.clone();
            let receipt = execute_direct_repair(selection, &f.store, "repair").unwrap();
            assert_eq!(receipt.deployment_id, f.request.deployment_id);
            assert_eq!(fs::read_to_string(&f.path).unwrap(), proposed);
            assert_eq!(
                fs::read(f.store.app_data.join("backups/repair/0-SKILL.md")).unwrap(),
                f.original
            );
            let row = f.store.get("repair").unwrap().unwrap();
            assert_eq!(row.status, "done");
            let InverseOp::RestoreBackup {
                path,
                post_fingerprint,
                ..
            } = serde_json::from_value(row.inverse.unwrap()).unwrap()
            else {
                panic!("wrong inverse")
            };
            assert_eq!(path, f.path);
            assert_eq!(
                post_fingerprint,
                crate::skill_event_store::fingerprint_path_checked(&f.path).unwrap()
            );
            f.store.restore("repair", false).unwrap();
            assert_eq!(fs::read(&f.path).unwrap(), f.original);
        }
    }

    #[test]
    fn fork_source_revision_changes_invalidate_saved_recovery_proposals() {
        for proposed in [false, true] {
            let mut f = Fixture::new(true);
            let selection = f
                .service
                .prepare_repair_selection(
                    &f.request,
                    &[],
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let intent = FrontmatterRepairIntent::from_preview(
                selection.preview(),
                FrontmatterRepairApplyMode::ApplyFix,
                None,
            )
            .unwrap();
            drop(selection);
            if proposed {
                fs::write(&f.path, &intent.proposed_content).unwrap();
            }
            let home = f
                .path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap();
            let mut registry = crate::skill_fork_registry::read_fork_registry(home).unwrap();
            registry.preferred_editor = Some("unrelated preference".into());
            write_fork_registry(home, &registry).unwrap();
            drop(
                f.service
                    .prepare_repair_recovery(
                        &intent,
                        &[],
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap(),
            );
            registry.forks.get_mut("alpha").unwrap().base_commit = "changed".into();
            write_fork_registry(home, &registry).unwrap();
            assert!(f
                .service
                .prepare_repair_recovery(
                    &intent,
                    &[],
                    Some(Duration::from_secs(10)),
                    CancellationToken::default()
                )
                .is_err());
        }
    }

    fn copy_fixture() -> Fixture {
        use crate::{skill_deployment::InstallScope, skill_fork_registry::CopyDeploymentRecord};
        let mut f = Fixture::new(false);
        let skill = f.path.parent().unwrap();
        let agents = skill.parent().unwrap().parent().unwrap();
        let home = agents.parent().unwrap();
        fs::remove_file(agents.join(".skill-lock.json")).unwrap();
        let inventory = f
            .service
            .scan(
                Some(&BTreeSet::from(["alpha".into()])),
                Some(Duration::from_secs(10)),
            )
            .unwrap();
        let mut registry = ForkRegistry::default();
        registry.copies.insert(
            f.request.deployment_id.clone(),
            CopyDeploymentRecord {
                deployment_id: f.request.deployment_id.clone(),
                name: "alpha".into(),
                path: skill.to_path_buf(),
                scope: InstallScope::Global,
                destination: SkillDestination::Universal,
                slot: "universal".into(),
                project_path: None,
                content_hash: inventory.skills[0].content_hash.clone(),
                disabled: false,
            },
        );
        write_fork_registry(home, &registry).unwrap();
        let inventory = f
            .service
            .scan(
                Some(&BTreeSet::from(["alpha".into()])),
                Some(Duration::from_secs(10)),
            )
            .unwrap();
        let deployment = &inventory.skills[0].deployments[0];
        assert_eq!(
            deployment.owner_kind,
            crate::skill_ownership::LifecycleOwnerKind::Copy
        );
        assert!(deployment.owner_revision.is_some());
        let preview = preview_frontmatter_repair(deployment, &f.original).unwrap();
        f.request.proposal_id = preview.proposal_id;
        f.request.mode = FrontmatterRepairApplyMode::ApplyFix;
        f
    }

    #[test]
    fn copy_repair_refuses_before_backup_until_registry_hash_transition_exists() {
        let mut f = copy_fixture();
        let skill = f.path.parent().unwrap();
        let agents = skill.parent().unwrap().parent().unwrap();
        let selection = f
            .service
            .prepare_repair_selection(
                &f.request,
                std::slice::from_ref(&f.store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        let failure = execute_direct_repair(selection, &f.store, "repair").unwrap_err();
        assert_eq!(failure.stage, RepairExecutionStage::Prepare);
        assert_eq!(fs::read(&f.path).unwrap(), f.original);
        assert!(!f.store.app_data.join("backups/repair").exists());
        assert!(f.store.get("repair").unwrap().is_none());
        let prepared = f
            .service
            .prepare_copy_repair_selection(
                &f.request,
                std::slice::from_ref(&f.store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        prepared.revalidate().unwrap();
        let intent = crate::skill_copy_repair::CopyRepairIntent::from_prepared(&prepared).unwrap();
        let wire = serde_json::to_value(&intent).unwrap();
        let decoded: crate::skill_copy_repair::CopyRepairIntent =
            serde_json::from_value(wire.clone()).unwrap();
        decoded.validate_record().unwrap();
        decoded.document.validate_original(&f.original).unwrap();
        decoded
            .validate_registry_original(prepared.registry_change().1)
            .unwrap();
        for change in ["owner", "target", "mode", "registry", "post"] {
            let mut bad = wire.clone();
            match change {
                "owner" => {
                    bad["transition"]["before"]["content_hash"] = serde_json::json!("c".repeat(64))
                }
                "target" => bad["document"]["path"] = serde_json::json!("/other"),
                "mode" => bad["document"]["mode"] = serde_json::json!("fix-installed-copy"),
                "registry" => {
                    bad["registry_path"] = serde_json::json!("relative/skill-studio.json")
                }
                "post" => {
                    bad["registry_after_fingerprint"] =
                        serde_json::json!("sha256:".to_owned() + &"0".repeat(64))
                }
                _ => unreachable!(),
            }
            let bad: crate::skill_copy_repair::CopyRepairIntent =
                serde_json::from_value(bad).unwrap();
            assert!(
                bad.validate_registry_original(prepared.registry_change().1)
                    .is_err(),
                "{change}"
            );
        }

        let (_, original_registry, proposed_registry) = prepared.registry_change();
        let mut expected: ForkRegistry = serde_json::from_slice(original_registry).unwrap();
        prepared.transition().apply(&mut expected).unwrap();
        assert_eq!(
            serde_json::to_value(expected).unwrap(),
            serde_json::from_slice::<serde_json::Value>(proposed_registry).unwrap()
        );
        assert_eq!(fs::read(&f.path).unwrap(), f.original);
        fs::write(skill.join("new-resource"), "external edit").unwrap();
        assert!(prepared.revalidate().is_err());
        drop(prepared);
        fs::remove_file(skill.join("new-resource")).unwrap();
        let prepared = f
            .service
            .prepare_copy_repair_selection(
                &f.request,
                std::slice::from_ref(&f.store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        let proposed = prepared.preview().proposed_content.clone();
        let registry_after = prepared.registry_change().2.to_vec();
        execute_copy_repair(prepared, &f.store, "copy-repair").unwrap();
        assert_eq!(fs::read(&f.path).unwrap(), proposed.as_bytes());
        assert_eq!(
            fs::read(agents.join("skill-studio.json")).unwrap(),
            registry_after
        );
        let row = f.store.get("copy-repair").unwrap().unwrap();
        assert_eq!(row.status, "done");
        assert!(!row.restorable);
        let intent: crate::skill_copy_repair::CopyRepairIntent =
            serde_json::from_value(row.payload).unwrap();
        intent.validate_record().unwrap();
        let inventory = f
            .service
            .scan(
                Some(&BTreeSet::from(["alpha".into()])),
                Some(Duration::from_secs(10)),
            )
            .unwrap();
        assert_eq!(
            inventory.skills[0].deployments[0].owner_kind,
            crate::skill_ownership::LifecycleOwnerKind::Copy
        );
    }

    #[test]
    fn rejects_unplanned_state_and_fork_mode_before_backup() {
        for fork_mode in [false, true] {
            let mut f = Fixture::new(false);
            if fork_mode {
                f.request.mode = FrontmatterRepairApplyMode::ForkAndFix;
            }
            let roots = if fork_mode {
                vec![f.store.app_data.clone()]
            } else {
                vec![]
            };
            let selection = f
                .service
                .prepare_repair_selection(
                    &f.request,
                    &roots,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let failure = execute_direct_repair(selection, &f.store, "repair").unwrap_err();
            assert_eq!(failure.stage, RepairExecutionStage::Prepare);
            assert_eq!(fs::read(&f.path).unwrap(), f.original);
            assert!(f.store.get("repair").unwrap().is_none());
            assert!(!f.store.app_data.join("backups/repair").exists());
        }
    }

    #[test]
    fn failed_completion_keeps_replaced_document_and_pending_inverse() {
        let mut f = Fixture::new(false);
        f.store.conn.execute_batch("CREATE TRIGGER refuse_done BEFORE UPDATE OF status ON events WHEN NEW.status = 'done' BEGIN SELECT RAISE(FAIL, 'injected completion failure'); END;").unwrap();
        let selection = f
            .service
            .prepare_repair_selection(
                &f.request,
                std::slice::from_ref(&f.store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        let proposed = selection.preview().proposed_content.clone();
        let failure = execute_direct_repair(selection, &f.store, "repair").unwrap_err();
        assert_eq!(failure.stage, RepairExecutionStage::Finish);
        assert_eq!(fs::read_to_string(&f.path).unwrap(), proposed);
        let row = f.store.get("repair").unwrap().unwrap();
        assert_eq!(row.status, "pending");
        let InverseOp::RestoreBackup {
            post_fingerprint, ..
        } = serde_json::from_value(row.inverse.unwrap()).unwrap()
        else {
            panic!("wrong inverse")
        };
        assert!(post_fingerprint.is_none());
        assert_eq!(
            fs::read(f.store.app_data.join("backups/repair/0-SKILL.md")).unwrap(),
            f.original
        );
        let intent = serde_json::from_value(row.payload).unwrap();
        let prepared = f
            .service
            .prepare_repair_recovery(
                &intent,
                std::slice::from_ref(&f.store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        let (_, state, lease) = prepared.into_parts();
        assert_eq!(
            state,
            crate::skill_service::RepairRecoveryDocumentState::Proposed
        );
        lease.revalidate().unwrap();
    }

    #[test]
    fn abrupt_exit_preserves_pending_intent_and_releases_write_lease() {
        use crate::skill_service::RepairRecoveryDocumentState;
        use std::process::{Command, Stdio};
        for checkpoint in ["intent", "document"] {
            let mut f = Fixture::new(false);
            fs::write(
                f._temp.path().join("request.json"),
                serde_json::to_vec(&f.request).unwrap(),
            )
            .unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "skill_repair_execution::tests::repair_crash_child",
                    "--ignored",
                ])
                .env("SKILL_STUDIO_REPAIR_CRASH_ROOT", f._temp.path())
                .env("SKILL_STUDIO_REPAIR_CRASH_STAGE", checkpoint)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("repair child did not reach its checkpoint");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(status.code(), Some(77));
            let row = f.store.get("crashed").unwrap().unwrap();
            assert_eq!(row.status, "pending");
            let intent: FrontmatterRepairIntent =
                serde_json::from_value(row.payload.clone()).unwrap();
            let expected = if checkpoint == "intent" {
                &f.original
            } else {
                intent.proposed_content.as_bytes()
            };
            assert_eq!(fs::read(&f.path).unwrap(), expected);
            assert_eq!(
                fs::read(f.store.app_data.join("backups/crashed/0-SKILL.md")).unwrap(),
                f.original
            );
            let prepared = f
                .service
                .prepare_repair_event_recovery(
                    &row,
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let (event, recovery) = prepared.into_parts();
            assert_eq!(event.id(), "crashed");
            let (_, observed, lease) = recovery.into_parts();
            assert_eq!(
                observed,
                if checkpoint == "intent" {
                    RepairRecoveryDocumentState::Original
                } else {
                    RepairRecoveryDocumentState::Proposed
                }
            );
            lease.revalidate().unwrap();
            let backup = crate::skill_repair_backup::VerifiedRepairBackup::read(
                &f.store.app_data,
                &event,
                &lease,
            )
            .unwrap();
            assert_eq!(backup.original(), f.original);
            backup.revalidate(&lease).unwrap();
            drop(lease);
            let prepared = f
                .service
                .prepare_repair_event_recovery(
                    &row,
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let outcome = recover_direct_repair(prepared, &f.store).unwrap();
            let (expected_outcome, expected_status) = if checkpoint == "intent" {
                (RepairRecoveryOutcome::NotApplied, "failed")
            } else {
                (RepairRecoveryOutcome::Applied, "done")
            };
            assert_eq!(outcome, expected_outcome);
            assert_eq!(
                f.store.get("crashed").unwrap().unwrap().status,
                expected_status
            );
            assert_eq!(fs::read(&f.path).unwrap(), expected);
        }
    }

    #[test]
    fn copy_recovery_resolves_all_states_and_preserves_unrelated_preferences() {
        for stage in [
            RepairExecutionStage::Intent,
            RepairExecutionStage::Document,
            RepairExecutionStage::Registry,
        ] {
            let mut f = copy_fixture();
            let cancel = CancellationToken::default();
            let prepared = f
                .service
                .prepare_copy_repair_selection(
                    &f.request,
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    cancel.clone(),
                )
                .unwrap();
            let proposed = prepared.preview().proposed_content.clone();
            assert!(execute_copy_repair_with(
                prepared,
                &f.store,
                "interrupted-copy",
                |checkpoint| {
                    if checkpoint == stage {
                        cancel.cancel();
                    }
                }
            )
            .is_err());
            let registry_path = f._temp.path().join("home/.agents/skill-studio.json");
            let mut registry: serde_json::Value =
                serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
            registry["future_preference"] = serde_json::json!("preserve me");
            fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
            let row = f.store.get("interrupted-copy").unwrap().unwrap();
            assert_eq!(row.status, "pending");
            let step = recover_next_repair(
                &mut f.service,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
            let RepairRecoveryStep::Resolved { event_id, outcome } = step else {
                panic!("copy recovery was not dispatched");
            };
            assert_eq!(event_id, "interrupted-copy");
            assert!(matches!(
                recover_next_repair(
                    &mut f.service,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default()
                )
                .unwrap(),
                RepairRecoveryStep::Idle
            ));
            let (expected, status) = if stage == RepairExecutionStage::Intent {
                (RepairRecoveryOutcome::NotApplied, "failed")
            } else {
                (RepairRecoveryOutcome::Applied, "done")
            };
            assert_eq!(outcome, expected);
            assert_eq!(
                f.store.get("interrupted-copy").unwrap().unwrap().status,
                status
            );
            assert_eq!(
                fs::read(&f.path).unwrap(),
                if stage == RepairExecutionStage::Intent {
                    f.original
                } else {
                    proposed.into_bytes()
                }
            );
            let registry: serde_json::Value =
                serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
            assert_eq!(registry["future_preference"], "preserve me");
            let inventory = f
                .service
                .scan(
                    Some(&BTreeSet::from(["alpha".into()])),
                    Some(Duration::from_secs(10)),
                )
                .unwrap();
            assert_eq!(
                inventory.skills[0].deployments[0].owner_kind,
                crate::skill_ownership::LifecycleOwnerKind::Copy
            );
        }
    }

    #[test]
    fn copy_undo_claim_and_intent_commit_or_roll_back_together() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        for failure in [
            "none",
            "insert",
            "claim",
            "source-drift",
            "finish-source",
            "finish-undo",
            "finish-sql",
        ] {
            let mut f = copy_fixture();
            let prepared = f
                .service
                .prepare_copy_repair_selection(
                    &f.request,
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            execute_copy_repair(prepared, &f.store, "source-copy").unwrap();
            let row = f.store.get("source-copy").unwrap().unwrap();
            let source =
                crate::skill_repair_recovery_event::CopyRepairUndoSource::from_row(&row).unwrap();
            let prepared_undo = f
                .service
                .prepare_copy_repair_undo(
                    &row,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            prepared_undo.revalidate().unwrap();
            assert_eq!(prepared_undo.plan().document_change().1, f.original);
            if failure == "source-drift" {
                let resource = f.path.parent().unwrap().join("new-resource");
                fs::write(&resource, b"external change").unwrap();
                assert!(prepared_undo.revalidate().is_err());
                fs::remove_file(resource).unwrap();
            }
            drop(prepared_undo);
            let mut wrong_home = row.clone();
            wrong_home.payload["registry_path"] =
                serde_json::json!("/another-home/.agents/skill-studio.json");
            assert!(f
                .service
                .prepare_copy_repair_undo(
                    &wrong_home,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default()
                )
                .is_err());
            let scope =
                crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&f.store.app_data))
                    .unwrap();
            let lease = CoordinationPlan::new(
                vec![DirectoryEffect::tree(
                    &f.store.app_data,
                    CoordinationMode::Exclusive,
                )],
                Some(Duration::from_secs(10)),
            )
            .unwrap()
            .acquire()
            .unwrap()
            .finalize_write(&scope, &[])
            .unwrap();
            let backup = crate::skill_repair_backup::VerifiedCopyRepairBackup::read_undo_source(
                &f.store.app_data,
                &source,
                &lease,
            )
            .unwrap();
            let document = fs::read(&f.path).unwrap();
            let registry_path = &source.intent().registry_path;
            let mut registry: serde_json::Value =
                serde_json::from_slice(&fs::read(registry_path).unwrap()).unwrap();
            registry["later_preference"] = serde_json::json!({"preserve": true});
            let registry = serde_json::to_vec(&registry).unwrap();
            let skill_scope =
                crate::skill_scope::SkillReadScope::bind(&[f.path.parent().unwrap().to_path_buf()])
                    .unwrap();
            let hash = crate::skill_discovery::live_skill_content_hash(
                &skill_scope,
                f.path.parent().unwrap(),
            )
            .unwrap();
            let plan = crate::skill_copy_repair::CopyRepairUndoPlan::from_observed(
                &source, &backup, &document, &registry, &hash,
            )
            .unwrap();
            assert_eq!(
                plan.document_change(),
                (document.as_slice(), f.original.as_slice())
            );
            assert_eq!(plan.registry_change().0, registry);
            let restored: serde_json::Value =
                serde_json::from_slice(plan.registry_change().1).unwrap();
            assert_eq!(restored["later_preference"]["preserve"], true);
            assert!(crate::skill_copy_repair::CopyRepairUndoPlan::from_observed(
                &source,
                &backup,
                b"external edit",
                &registry,
                &hash
            )
            .is_err());
            assert!(crate::skill_copy_repair::CopyRepairUndoPlan::from_observed(
                &source,
                &backup,
                &document,
                &registry,
                &"f".repeat(64)
            )
            .is_err());
            let events = GuardedEventStore::bind(&f.store, &lease).unwrap();
            match failure {
                "insert" => f.store.conn.execute_batch("CREATE TRIGGER reject_undo_insert BEFORE INSERT ON events BEGIN SELECT RAISE(FAIL, 'injected insert failure'); END;").unwrap(),
                "claim" => f.store.conn.execute_batch("CREATE TRIGGER reject_undo_claim BEFORE UPDATE OF reverted_by ON events BEGIN SELECT RAISE(FAIL, 'injected claim failure'); END;").unwrap(),
                "source-drift" => { f.store.conn.execute("UPDATE events SET payload = '{}' WHERE id = 'source-copy'", []).unwrap(); },
                _ => {},
            }
            let result = events.record_copy_undo(&lease, &source, "undo-copy");
            let current = f.store.get("source-copy").unwrap().unwrap();
            if !matches!(failure, "insert" | "claim" | "source-drift") {
                let recorded = result.unwrap();
                assert_eq!(current.reverted_by.as_deref(), Some("undo-copy"));
                let undo = f.store.get("undo-copy").unwrap().unwrap();
                assert_eq!(undo.status, "pending");
                assert_eq!(undo.payload["target_event"], "source-copy");
                assert!(events
                    .record_copy_undo(&lease, &source, "second-undo")
                    .is_err());
                assert!(f.store.get("second-undo").unwrap().is_none());
                match failure {
                    "finish-source" => { f.store.conn.execute("UPDATE events SET reverted_by = NULL WHERE id = 'source-copy'", []).unwrap(); },
                    "finish-undo" => { f.store.conn.execute("UPDATE events SET payload = '{}' WHERE id = 'undo-copy'", []).unwrap(); },
                    "finish-sql" => f.store.conn.execute_batch("CREATE TRIGGER reject_undo_finish BEFORE UPDATE OF status ON events BEGIN SELECT RAISE(FAIL, 'injected completion failure'); END;").unwrap(),
                    _ => {},
                }
                let before = serde_json::to_value(f.store.list(10, None).unwrap()).unwrap();
                let finished = events.finish_copy_undo(&lease, &recorded);
                if failure == "none" {
                    finished.unwrap();
                    assert_eq!(f.store.get("undo-copy").unwrap().unwrap().status, "done");
                    assert!(events.finish_copy_undo(&lease, &recorded).is_err());
                } else {
                    assert!(finished.is_err());
                    assert_eq!(
                        serde_json::to_value(f.store.list(10, None).unwrap()).unwrap(),
                        before
                    );
                }
            } else {
                assert!(result.is_err(), "{failure}");
                assert!(current.reverted_by.is_none());
                assert!(f.store.get("undo-copy").unwrap().is_none());
            }
            assert!(f.store.conn.is_autocommit());
        }
    }

    #[test]
    fn copy_repair_undo_restores_document_and_owner_without_losing_preferences() {
        let mut f = copy_fixture();
        let prepared = f
            .service
            .prepare_copy_repair_selection(
                &f.request,
                std::slice::from_ref(&f.store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        execute_copy_repair(prepared, &f.store, "undo-source").unwrap();
        let row = f.store.get("undo-source").unwrap().unwrap();
        let registry_path = f._temp.path().join("home/.agents/skill-studio.json");
        let mut registry: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
        registry["later_preference"] = serde_json::json!("keep");
        fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
        let document_before = fs::read(&f.path).unwrap();
        let registry_before = fs::read(&registry_path).unwrap();
        let prepared = f
            .service
            .prepare_copy_repair_undo(
                &row,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        execute_copy_repair_undo(prepared, &f.store, "undo-result").unwrap();
        assert_eq!(fs::read(&f.path).unwrap(), f.original);
        let registry: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
        assert_eq!(registry["later_preference"], "keep");
        assert_eq!(
            f.store
                .get("undo-source")
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some("undo-result")
        );
        assert_eq!(f.store.get("undo-result").unwrap().unwrap().status, "done");
        let backup = f.store.app_data.join("backups/undo-result");
        assert_eq!(
            fs::read(backup.join("0-SKILL.md")).unwrap(),
            document_before
        );
        assert_eq!(
            fs::read(backup.join("1-skill-studio.json")).unwrap(),
            registry_before
        );
        let inventory = f
            .service
            .scan(
                Some(&BTreeSet::from(["alpha".into()])),
                Some(Duration::from_secs(10)),
            )
            .unwrap();
        assert_eq!(
            inventory.skills[0].deployments[0].owner_kind,
            crate::skill_ownership::LifecycleOwnerKind::Copy
        );
        use crate::skill_repair_recovery_event::CopyRepairRedoSource;
        let source_row = f.store.get("undo-source").unwrap().unwrap();
        let undo_row = f.store.get("undo-result").unwrap().unwrap();
        let redo = CopyRepairRedoSource::from_rows(&source_row, &undo_row).unwrap();
        for change in [
            "source-claim",
            "undo-claim",
            "undo-status",
            "source-status",
            "payload",
            "backup",
        ] {
            let mut source = source_row.clone();
            let mut undo = undo_row.clone();
            match change {
                "source-claim" => source.reverted_by = None,
                "undo-claim" => undo.reverted_by = Some("another".into()),
                "undo-status" => undo.status = "pending".into(),
                "source-status" => source.status = "failed".into(),
                "payload" => undo.payload["target_event"] = serde_json::json!("another"),
                "backup" => undo.backup_dir = Some("backups/another".into()),
                _ => unreachable!(),
            }
            assert!(
                CopyRepairRedoSource::from_rows(&source, &undo).is_err(),
                "{change}"
            );
        }
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        let state_scope =
            crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&f.store.app_data))
                .unwrap();
        let lease = CoordinationPlan::new(
            vec![DirectoryEffect::tree(
                &f.store.app_data,
                CoordinationMode::Exclusive,
            )],
            Some(Duration::from_secs(10)),
        )
        .unwrap()
        .acquire()
        .unwrap()
        .finalize_write(&state_scope, &[])
        .unwrap();
        let backups = crate::skill_repair_backup::VerifiedCopyUndoBackups::read_redo_source(
            &f.store.app_data,
            &redo,
            &lease,
        )
        .unwrap();
        let skill_dir = f.path.parent().unwrap();
        let scope = crate::skill_scope::SkillReadScope::bind(&[skill_dir.to_path_buf()]).unwrap();
        let hash = crate::skill_discovery::live_skill_content_hash(&scope, skill_dir).unwrap();
        let document = fs::read(&f.path).unwrap();
        let registry_bytes = fs::read(&registry_path).unwrap();
        let mut later_registry: serde_json::Value =
            serde_json::from_slice(&registry_bytes).unwrap();
        later_registry["after_undo"] = serde_json::json!("preserve");
        let later_registry = serde_json::to_vec(&later_registry).unwrap();
        let plan = crate::skill_copy_repair::CopyRepairRedoPlan::from_observed(
            &redo,
            &backups,
            &document,
            &later_registry,
            &hash,
        )
        .unwrap();
        assert_eq!(
            plan.document_change(),
            (document.as_slice(), document_before.as_slice())
        );
        assert_eq!(plan.registry_change().0, later_registry);
        let registry_after: serde_json::Value =
            serde_json::from_slice(plan.registry_change().1).unwrap();
        assert_eq!(registry_after["after_undo"], "preserve");
        assert_eq!(registry_after["later_preference"], "keep");
        assert_eq!(
            plan.registry_change().1,
            redo.intent()
                .transition
                .apply_document(&later_registry)
                .unwrap()
        );
        for (doc, registry, folder) in [
            (
                b"edited".as_slice(),
                later_registry.as_slice(),
                hash.as_str(),
            ),
            (
                document.as_slice(),
                later_registry.as_slice(),
                "incorrect hash",
            ),
            (document.as_slice(), b"{}".as_slice(), hash.as_str()),
            (
                document.as_slice(),
                registry_before.as_slice(),
                hash.as_str(),
            ),
        ] {
            assert!(crate::skill_copy_repair::CopyRepairRedoPlan::from_observed(
                &redo, &backups, doc, registry, folder,
            )
            .is_err());
        }
        backups.revalidate(&lease).unwrap();
        assert_eq!(fs::read(&f.path).unwrap(), document);
        assert_eq!(fs::read(&registry_path).unwrap(), registry_bytes);
        drop(backups);
        drop(lease);
        let prepared = f
            .service
            .prepare_copy_repair_redo(
                &source_row,
                &undo_row,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        prepared.revalidate().unwrap();
        assert_eq!(prepared.plan().document_change().1, document_before);
        let changed_resource = skill_dir.join("redo-resource-drift");
        fs::write(&changed_resource, b"new resource").unwrap();
        assert!(prepared.revalidate().is_err());
        drop(prepared);
        assert!(f
            .service
            .prepare_copy_repair_redo(
                &source_row,
                &undo_row,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .is_err());
    }

    fn interrupted_copy_undo_fixture() -> Fixture {
        let mut f = copy_fixture();
        let prepared = f
            .service
            .prepare_copy_repair_selection(
                &f.request,
                std::slice::from_ref(&f.store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        execute_copy_repair(prepared, &f.store, "recovery-source").unwrap();
        let source = f.store.get("recovery-source").unwrap().unwrap();
        let cancel = CancellationToken::default();
        let prepared = f
            .service
            .prepare_copy_repair_undo(
                &source,
                &f.store,
                Some(Duration::from_secs(10)),
                cancel.clone(),
            )
            .unwrap();
        execute_copy_repair_undo_with(prepared, &f.store, "recover-undo", |at| {
            if at == RepairExecutionStage::Document {
                cancel.cancel();
            }
        })
        .unwrap_err();
        f
    }

    fn interrupted_copy_redo_fixture(stage: RepairExecutionStage) -> Fixture {
        let mut f = interrupted_copy_undo_fixture();
        recover_next_repair(
            &mut f.service,
            &f.store,
            Some(Duration::from_secs(10)),
            CancellationToken::default(),
        )
        .unwrap();
        let source = f.store.get("recovery-source").unwrap().unwrap();
        let undo = f.store.get("recover-undo").unwrap().unwrap();
        let cancel = CancellationToken::default();
        let prepared = f
            .service
            .prepare_copy_repair_redo(
                &source,
                &undo,
                &f.store,
                Some(Duration::from_secs(10)),
                cancel.clone(),
            )
            .unwrap();
        execute_copy_repair_redo_with(prepared, &f.store, "recover-redo", |at| {
            if at == stage {
                cancel.cancel();
            }
        })
        .unwrap_err();
        f
    }

    #[test]
    fn repeated_copy_undo_redo_preserves_prior_links_and_recovers_each_direction() {
        let mut f = interrupted_copy_redo_fixture(RepairExecutionStage::Registry);
        recover_next_repair(
            &mut f.service,
            &f.store,
            Some(Duration::from_secs(10)),
            CancellationToken::default(),
        )
        .unwrap();
        let mut current = "recover-redo".to_owned();
        let mut links = vec![
            ("recovery-source".to_owned(), "recover-undo".to_owned()),
            ("recover-undo".to_owned(), current.clone()),
        ];
        for cycle in 0..2 {
            let source = f.store.get(&current).unwrap().unwrap();
            let source_intent: crate::skill_copy_repair::CopyRepairRedoIntent =
                serde_json::from_value(source.payload.clone()).unwrap();
            for bad in ["self", "same", "path", "pending"] {
                let mut row = source.clone();
                match bad {
                    "self" => row.payload["source_event"] = serde_json::json!(row.id),
                    "same" => row.payload["undo_event"] = row.payload["source_event"].clone(),
                    "path" => row.payload["source_event"] = serde_json::json!("../outside"),
                    "pending" => row.status = "pending".into(),
                    _ => unreachable!(),
                }
                assert!(
                    crate::skill_repair_recovery_event::CopyRepairUndoSource::from_row(&row)
                        .is_err()
                );
            }
            let registry_path = &source_intent.repair.registry_path;
            let mut registry: serde_json::Value =
                serde_json::from_slice(&fs::read(registry_path).unwrap()).unwrap();
            registry[format!("cycle_{cycle}")] = serde_json::json!("keep");
            fs::write(registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
            let undo_id = format!("cycle-{cycle}-undo");
            let cancel = CancellationToken::default();
            let prepared = f
                .service
                .prepare_copy_repair_undo(
                    &source,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    cancel.clone(),
                )
                .unwrap();
            execute_copy_repair_undo_with(prepared, &f.store, &undo_id, |at| {
                if at == RepairExecutionStage::Document {
                    cancel.cancel();
                }
            })
            .unwrap_err();
            recover_next_repair(
                &mut f.service,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
            assert_eq!(fs::read(&f.path).unwrap(), f.original);
            links.push((current.clone(), undo_id.clone()));
            let source = f.store.get(&current).unwrap().unwrap();
            let undo = f.store.get(&undo_id).unwrap().unwrap();
            let redo_id = format!("cycle-{cycle}-redo");
            let cancel = CancellationToken::default();
            let prepared = f
                .service
                .prepare_copy_repair_redo(
                    &source,
                    &undo,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    cancel.clone(),
                )
                .unwrap();
            execute_copy_repair_redo_with(prepared, &f.store, &redo_id, |at| {
                if at == RepairExecutionStage::Document {
                    cancel.cancel();
                }
            })
            .unwrap_err();
            // The single-repair recovery constructor must not bypass redo claim resolution.
            assert!(
                crate::skill_repair_recovery_event::CopyRepairRecoveryEvent::from_row(
                    &f.store.get(&redo_id).unwrap().unwrap()
                )
                .is_err()
            );
            recover_next_repair(
                &mut f.service,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
            assert_eq!(
                fs::read(&f.path).unwrap(),
                source_intent.repair.document.proposed_content.as_bytes()
            );
            links.push((undo_id, redo_id.clone()));
            current = redo_id;
            for (from, to) in &links {
                let row = f.store.get(from).unwrap().unwrap();
                assert_eq!(row.status, "done");
                assert_eq!(row.reverted_by.as_deref(), Some(to.as_str()));
            }
            assert!(f
                .store
                .get(&current)
                .unwrap()
                .unwrap()
                .reverted_by
                .is_none());
            let registry: serde_json::Value =
                serde_json::from_slice(&fs::read(registry_path).unwrap()).unwrap();
            for prior in 0..=cycle {
                assert_eq!(registry[format!("cycle_{prior}")], "keep");
            }
        }
        assert!(matches!(
            recover_next_repair(
                &mut f.service,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .unwrap(),
            RepairRecoveryStep::Idle
        ));
    }

    #[test]
    fn copy_redo_recovery_resolves_states_and_preserves_changed_history() {
        for (stage, fault) in [
            (RepairExecutionStage::Intent, "none"),
            (RepairExecutionStage::Document, "none"),
            (RepairExecutionStage::Registry, "none"),
            (RepairExecutionStage::Intent, "release"),
            (RepairExecutionStage::Document, "finish"),
            (RepairExecutionStage::Document, "source-before"),
            (RepairExecutionStage::Document, "undo-before"),
            (RepairExecutionStage::Document, "redo-before"),
            (RepairExecutionStage::Document, "source-after"),
            (RepairExecutionStage::Document, "undo-after"),
            (RepairExecutionStage::Document, "redo-after"),
        ] {
            let mut f = interrupted_copy_redo_fixture(stage);
            if stage == RepairExecutionStage::Registry {
                f.store
                    .conn
                    .execute(
                        "UPDATE events SET status = 'interrupted' WHERE id = 'recover-redo'",
                        [],
                    )
                    .unwrap();
            }
            let source = f.store.get("recovery-source").unwrap().unwrap();
            let undo = f.store.get("recover-undo").unwrap().unwrap();
            let redo = f.store.get("recover-redo").unwrap().unwrap();
            let intent: crate::skill_copy_repair::CopyRepairRedoIntent =
                serde_json::from_value(redo.payload.clone()).unwrap();
            let registry_path = &intent.repair.registry_path;
            let mut registry: serde_json::Value =
                serde_json::from_slice(&fs::read(registry_path).unwrap()).unwrap();
            registry["after_interruption"] = serde_json::json!("keep");
            fs::write(registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
            let before_document = fs::read(&f.path).unwrap();
            let before_registry = fs::read(registry_path).unwrap();
            let prepared = f
                .service
                .prepare_copy_redo_recovery(
                    &source,
                    &undo,
                    &redo,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let changed_id = match fault.split('-').next().unwrap() {
                "source" => "recovery-source",
                "undo" => "recover-undo",
                _ => "recover-redo",
            };
            if fault.ends_with("before") {
                f.store
                    .conn
                    .execute(
                        "UPDATE events SET payload = '{}' WHERE id = ?1",
                        [changed_id],
                    )
                    .unwrap();
            }
            if fault == "release" {
                f.store.conn.execute_batch("CREATE TRIGGER reject_redo_recovery BEFORE UPDATE OF reverted_by ON events WHEN NEW.id = 'recover-undo' AND NEW.reverted_by IS NULL BEGIN SELECT RAISE(FAIL, 'release failed'); END;").unwrap();
            } else if fault == "finish" {
                f.store.conn.execute_batch("CREATE TRIGGER reject_redo_recovery BEFORE UPDATE OF status ON events WHEN NEW.id = 'recover-redo' AND NEW.status = 'done' BEGIN SELECT RAISE(FAIL, 'finish failed'); END;").unwrap();
            }
            let result = if fault == "none" {
                drop(prepared);
                match recover_next_repair(
                    &mut f.service,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap()
                {
                    RepairRecoveryStep::Resolved { event_id, outcome } => {
                        assert_eq!(event_id, "recover-redo");
                        Ok(outcome)
                    }
                    _ => panic!("redo recovery was skipped"),
                }
            } else {
                recover_copy_redo_with(prepared, &f.store, || {
                    if fault.ends_with("after") {
                        f.store
                            .conn
                            .execute(
                                "UPDATE events SET payload = '{}' WHERE id = ?1",
                                [changed_id],
                            )
                            .unwrap();
                    }
                })
            };
            if fault.ends_with("before") || fault.ends_with("after") {
                assert_eq!(
                    result.unwrap_err().stage,
                    if fault.ends_with("before") {
                        RepairExecutionStage::Prepare
                    } else {
                        RepairExecutionStage::Finish
                    }
                );
                assert_eq!(fs::read(&f.path).unwrap(), before_document);
                assert_eq!(
                    fs::read(registry_path).unwrap(),
                    if fault.ends_with("before") {
                        before_registry
                    } else {
                        intent
                            .repair
                            .transition
                            .apply_document(&before_registry)
                            .unwrap()
                    }
                );
                assert_eq!(
                    f.store.get("recover-redo").unwrap().unwrap().status,
                    "pending"
                );
                assert_eq!(
                    f.store
                        .get("recovery-source")
                        .unwrap()
                        .unwrap()
                        .reverted_by
                        .as_deref(),
                    Some("recover-undo")
                );
                assert_eq!(
                    f.store
                        .get("recover-undo")
                        .unwrap()
                        .unwrap()
                        .reverted_by
                        .as_deref(),
                    Some("recover-redo")
                );
                continue;
            }
            let result = if fault != "none" {
                assert_eq!(result.unwrap_err().stage, RepairExecutionStage::Finish);
                assert_eq!(
                    f.store.get("recover-redo").unwrap().unwrap().status,
                    "pending"
                );
                assert_eq!(
                    f.store
                        .get("recover-undo")
                        .unwrap()
                        .unwrap()
                        .reverted_by
                        .as_deref(),
                    Some("recover-redo")
                );
                f.store
                    .conn
                    .execute_batch("DROP TRIGGER reject_redo_recovery;")
                    .unwrap();
                let prepared = f
                    .service
                    .prepare_copy_redo_recovery(
                        &source,
                        &undo,
                        &redo,
                        &f.store,
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                recover_copy_redo(prepared, &f.store)
            } else {
                result
            };
            let applied = stage != RepairExecutionStage::Intent;
            assert_eq!(
                result.unwrap(),
                if applied {
                    RepairRecoveryOutcome::Applied
                } else {
                    RepairRecoveryOutcome::NotApplied
                }
            );
            assert_eq!(
                fs::read(&f.path).unwrap(),
                if applied {
                    intent.repair.document.proposed_content.as_bytes().to_vec()
                } else {
                    f.original.clone()
                }
            );
            assert_eq!(
                fs::read(registry_path).unwrap(),
                if applied {
                    intent
                        .repair
                        .transition
                        .apply_document(&before_registry)
                        .unwrap()
                } else {
                    before_registry
                }
            );
            assert_eq!(
                f.store.get("recover-redo").unwrap().unwrap().status,
                if applied { "done" } else { "failed" }
            );
            assert_eq!(
                f.store
                    .get("recovery-source")
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .as_deref(),
                Some("recover-undo")
            );
            assert_eq!(
                f.store
                    .get("recover-undo")
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .as_deref(),
                if applied { Some("recover-redo") } else { None }
            );
            assert!(matches!(
                recover_next_repair(
                    &mut f.service,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default()
                )
                .unwrap(),
                RepairRecoveryStep::Idle
            ));
        }
    }

    #[test]
    fn copy_redo_process_exit_preserves_three_event_recovery_evidence() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        use std::process::{Command, Stdio};
        for stage in ["intent", "document", "registry"] {
            let mut f = interrupted_copy_undo_fixture();
            recover_next_repair(
                &mut f.service,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
            let source = f.store.get("recovery-source").unwrap().unwrap();
            let original_intent: crate::skill_copy_repair::CopyRepairIntent =
                serde_json::from_value(source.payload.clone()).unwrap();
            let registry_path = &original_intent.registry_path;
            let mut registry: serde_json::Value =
                serde_json::from_slice(&fs::read(registry_path).unwrap()).unwrap();
            registry["before_redo"] = serde_json::json!("keep");
            fs::write(registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
            let original_document = fs::read(&f.path).unwrap();
            let original_registry = fs::read(registry_path).unwrap();
            let repaired_registry = original_intent
                .transition
                .apply_document(&original_registry)
                .unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "skill_repair_execution::tests::copy_redo_crash_child",
                    "--ignored",
                ])
                .env("SKILL_STUDIO_REPAIR_CRASH_ROOT", f._temp.path())
                .env("SKILL_STUDIO_REPAIR_CRASH_STAGE", stage)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("redo child timed out");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(status.code(), Some(82));
            let source = f.store.get("recovery-source").unwrap().unwrap();
            let undo = f.store.get("recover-undo").unwrap().unwrap();
            let redo = f.store.get("redo-crashed").unwrap().unwrap();
            assert_eq!(redo.status, "pending");
            assert_eq!(source.reverted_by.as_deref(), Some("recover-undo"));
            assert_eq!(undo.reverted_by.as_deref(), Some("redo-crashed"));
            let event = crate::skill_repair_recovery_event::CopyRedoRecoveryEvent::from_rows(
                &source, &undo, &redo,
            )
            .unwrap();
            assert_eq!(
                fs::read(&f.path).unwrap(),
                if stage == "intent" {
                    original_document.clone()
                } else {
                    original_intent
                        .document
                        .proposed_content
                        .as_bytes()
                        .to_vec()
                }
            );
            assert_eq!(
                fs::read(registry_path).unwrap(),
                if stage == "registry" {
                    repaired_registry
                } else {
                    original_registry.clone()
                }
            );
            let skill_dir = f.path.parent().unwrap();
            let scope =
                crate::skill_scope::SkillReadScope::bind(&[skill_dir.to_path_buf()]).unwrap();
            let hash = crate::skill_discovery::live_skill_content_hash(&scope, skill_dir).unwrap();
            let observed = event
                .intent()
                .repair
                .classify_observed(
                    &fs::read(&f.path).unwrap(),
                    &fs::read(registry_path).unwrap(),
                    &hash,
                )
                .unwrap();
            use crate::skill_copy_repair::CopyRepairObservedState;
            assert_eq!(
                observed,
                match stage {
                    "intent" => CopyRepairObservedState::Original,
                    "document" => CopyRepairObservedState::DocumentApplied,
                    _ => CopyRepairObservedState::Applied,
                }
            );
            let prepared = f
                .service
                .prepare_copy_redo_recovery(
                    &source,
                    &undo,
                    &redo,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            assert_eq!(prepared.state(), observed);
            prepared.revalidate().unwrap();
            let added_resource = skill_dir.join("after-redo-recovery-preparation");
            fs::write(&added_resource, b"changed resource set").unwrap();
            assert!(prepared.revalidate().is_err());
            drop(prepared);
            assert!(f
                .service
                .prepare_copy_redo_recovery(
                    &source,
                    &undo,
                    &redo,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default()
                )
                .is_err());
            fs::remove_file(added_resource).unwrap();
            for change in [
                "source-claim",
                "undo-claim",
                "redo-claim",
                "status",
                "intent",
                "backup",
                "source-status",
                "undo-status",
            ] {
                let mut source = source.clone();
                let mut undo = undo.clone();
                let mut redo = redo.clone();
                match change {
                    "source-claim" => source.reverted_by = None,
                    "undo-claim" => undo.reverted_by = None,
                    "redo-claim" => redo.reverted_by = Some("other".into()),
                    "status" => redo.status = "done".into(),
                    "intent" => redo.payload["undo_event"] = serde_json::json!("other"),
                    "backup" => redo.backup_dir = Some("backups/other".into()),
                    "source-status" => source.status = "failed".into(),
                    "undo-status" => undo.status = "pending".into(),
                    _ => unreachable!(),
                }
                assert!(
                    crate::skill_repair_recovery_event::CopyRedoRecoveryEvent::from_rows(
                        &source, &undo, &redo
                    )
                    .is_err(),
                    "{change}"
                );
            }
            let state_scope =
                crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&f.store.app_data))
                    .unwrap();
            let lease = CoordinationPlan::new(
                vec![DirectoryEffect::tree(
                    &f.store.app_data,
                    CoordinationMode::Exclusive,
                )],
                Some(Duration::from_secs(10)),
            )
            .unwrap()
            .acquire()
            .unwrap()
            .finalize_write(&state_scope, &[])
            .unwrap();
            let backups = crate::skill_repair_backup::VerifiedCopyRedoBackups::read(
                &f.store.app_data,
                &event,
                &lease,
            )
            .unwrap();
            assert_eq!(
                backups.originals(),
                (original_document.as_slice(), original_registry.as_slice())
            );
            backups.revalidate(&lease).unwrap();
            let corrupt = match stage {
                "intent" => "recovery-source",
                "document" => "recover-undo",
                _ => "redo-crashed",
            };
            fs::write(
                f.store
                    .app_data
                    .join("backups")
                    .join(corrupt)
                    .join("0-SKILL.md"),
                b"changed backup",
            )
            .unwrap();
            assert!(backups.revalidate(&lease).is_err());
            assert!(crate::skill_repair_backup::VerifiedCopyRedoBackups::read(
                &f.store.app_data,
                &event,
                &lease
            )
            .is_err());
        }
    }

    #[test]
    #[ignore = "explicit child for copy redo process exit"]
    fn copy_redo_crash_child() {
        let root = PathBuf::from(std::env::var_os("SKILL_STUDIO_REPAIR_CRASH_ROOT").unwrap());
        let stage = std::env::var("SKILL_STUDIO_REPAIR_CRASH_STAGE").unwrap();
        let store = EventStore::open(&root.join("state")).unwrap();
        store
            .conn
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        let mut service = ScopedSkillService::bind(SkillScope {
            home: root.join("home"),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        })
        .unwrap();
        let source = store.get("recovery-source").unwrap().unwrap();
        let undo = store.get("recover-undo").unwrap().unwrap();
        let prepared = service
            .prepare_copy_repair_redo(
                &source,
                &undo,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        execute_copy_repair_redo_with(prepared, &store, "redo-crashed", |at| {
            if matches!(
                (stage.as_str(), at),
                ("intent", RepairExecutionStage::Intent)
                    | ("document", RepairExecutionStage::Document)
                    | ("registry", RepairExecutionStage::Registry)
            ) {
                std::process::exit(82);
            }
        })
        .unwrap();
        panic!("redo checkpoint not reached");
    }

    #[test]
    fn copy_redo_execution_restores_repair_and_preserves_linked_history() {
        let mut f = interrupted_copy_undo_fixture();
        recover_next_repair(
            &mut f.service,
            &f.store,
            Some(Duration::from_secs(10)),
            CancellationToken::default(),
        )
        .unwrap();
        let source = f.store.get("recovery-source").unwrap().unwrap();
        let undo = f.store.get("recover-undo").unwrap().unwrap();
        let intent: crate::skill_copy_repair::CopyRepairIntent =
            serde_json::from_value(source.payload.clone()).unwrap();
        let mut registry: serde_json::Value =
            serde_json::from_slice(&fs::read(&intent.registry_path).unwrap()).unwrap();
        registry["after_undo"] = serde_json::json!("keep");
        fs::write(
            &intent.registry_path,
            serde_json::to_vec(&registry).unwrap(),
        )
        .unwrap();
        let before_document = fs::read(&f.path).unwrap();
        let before_registry = fs::read(&intent.registry_path).unwrap();
        let prepared = f
            .service
            .prepare_copy_repair_redo(
                &source,
                &undo,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        execute_copy_repair_redo(prepared, &f.store, "redo-result").unwrap();
        assert_eq!(
            fs::read(&f.path).unwrap(),
            intent.document.proposed_content.as_bytes()
        );
        assert_eq!(
            fs::read(&intent.registry_path).unwrap(),
            intent.transition.apply_document(&before_registry).unwrap()
        );
        let row = f.store.get("redo-result").unwrap().unwrap();
        assert_eq!(row.status, "done");
        assert!(!row.restorable);
        assert!(row.inverse.is_none());
        assert_eq!(
            f.store
                .get("recovery-source")
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some("recover-undo")
        );
        assert_eq!(
            f.store
                .get("recover-undo")
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some("redo-result")
        );
        let backup = f.store.app_data.join("backups/redo-result");
        assert_eq!(
            fs::read(backup.join("0-SKILL.md")).unwrap(),
            before_document
        );
        assert_eq!(
            fs::read(backup.join("1-skill-studio.json")).unwrap(),
            before_registry
        );
        let saved: crate::skill_copy_repair::CopyRepairRedoIntent =
            serde_json::from_value(row.payload).unwrap();
        saved
            .repair
            .validate_registry_original(&before_registry)
            .unwrap();
        let inventory = f
            .service
            .scan(
                Some(&BTreeSet::from(["alpha".into()])),
                Some(Duration::from_secs(10)),
            )
            .unwrap();
        assert_eq!(
            inventory.skills[0].deployments[0].owner_kind,
            crate::skill_ownership::LifecycleOwnerKind::Copy
        );
    }

    #[test]
    fn copy_redo_history_claim_and_completion_are_conditional() {
        use crate::skill_copy_repair::CopyRepairRedoIntent;
        for fault in [
            "none",
            "insert",
            "claim",
            "source-drift",
            "undo-drift",
            "finish-source",
            "finish-undo",
            "finish-redo",
            "finish-sql",
        ] {
            let mut f = interrupted_copy_undo_fixture();
            recover_next_repair(
                &mut f.service,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
            let source = f.store.get("recovery-source").unwrap().unwrap();
            let undo = f.store.get("recover-undo").unwrap().unwrap();
            let registry_path = f._temp.path().join("home/.agents/skill-studio.json");
            let mut registry: serde_json::Value =
                serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
            registry["after_undo"] = serde_json::json!("keep");
            fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
            let before_document = fs::read(&f.path).unwrap();
            let before_registry = fs::read(&registry_path).unwrap();
            let prepared = f
                .service
                .prepare_copy_repair_redo(
                    &source,
                    &undo,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let intent = CopyRepairRedoIntent::from_prepared(&prepared).unwrap();
            intent
                .repair
                .validate_registry_original(&before_registry)
                .unwrap();
            assert_ne!(
                intent.repair.registry_before_fingerprint,
                prepared.source.intent().registry_before_fingerprint
            );
            let events = GuardedEventStore::bind(&f.store, &prepared.lease).unwrap();
            for id in ["../invalid", "recovery-source", "recover-undo"] {
                assert!(events
                    .record_copy_redo(&prepared.lease, &prepared.source, &intent, id)
                    .is_err());
            }
            let mut invalid = intent.clone();
            invalid.undo_event = "other".into();
            assert!(events
                .record_copy_redo(&prepared.lease, &prepared.source, &invalid, "redo-result")
                .is_err());
            match fault {
                "insert" => f.store.conn.execute_batch("CREATE TRIGGER reject_redo BEFORE INSERT ON events WHEN NEW.kind = 'redo_copy_frontmatter' BEGIN SELECT RAISE(FAIL, 'insert failed'); END;").unwrap(),
                "claim" => f.store.conn.execute_batch("CREATE TRIGGER reject_redo BEFORE UPDATE OF reverted_by ON events WHEN NEW.id = 'recover-undo' BEGIN SELECT RAISE(FAIL, 'claim failed'); END;").unwrap(),
                "source-drift" | "undo-drift" => {
                    let id = if fault == "source-drift" { "recovery-source" } else { "recover-undo" };
                    f.store.conn.execute("UPDATE events SET payload = '{}' WHERE id = ?1", [id]).unwrap();
                }
                _ => {}
            }
            let result =
                events.record_copy_redo(&prepared.lease, &prepared.source, &intent, "redo-result");
            if ["insert", "claim", "source-drift", "undo-drift"].contains(&fault) {
                assert!(result.is_err(), "{fault}");
                assert!(f.store.get("redo-result").unwrap().is_none());
                assert!(f
                    .store
                    .get("recover-undo")
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .is_none());
            } else {
                let recorded = result.unwrap();
                assert_eq!(
                    f.store
                        .get("recover-undo")
                        .unwrap()
                        .unwrap()
                        .reverted_by
                        .as_deref(),
                    Some("redo-result")
                );
                assert_eq!(
                    f.store.get("redo-result").unwrap().unwrap().payload,
                    serde_json::to_value(&intent).unwrap()
                );
                assert!(events
                    .record_copy_redo(&prepared.lease, &prepared.source, &intent, "second-redo")
                    .is_err());
                match fault {
                    "finish-source" | "finish-undo" | "finish-redo" => {
                        let id = match fault { "finish-source" => "recovery-source", "finish-undo" => "recover-undo", _ => "redo-result" };
                        f.store.conn.execute("UPDATE events SET payload = '{}' WHERE id = ?1", [id]).unwrap();
                    }
                    "finish-sql" => f.store.conn.execute_batch("CREATE TRIGGER reject_redo BEFORE UPDATE OF status ON events WHEN NEW.id = 'redo-result' BEGIN SELECT RAISE(FAIL, 'finish failed'); END;").unwrap(),
                    _ => {}
                }
                let finished = events.finish_copy_redo(&prepared.lease, &recorded);
                if fault == "none" {
                    finished.unwrap();
                    assert!(events.finish_copy_redo(&prepared.lease, &recorded).is_err());
                    assert_eq!(f.store.get("redo-result").unwrap().unwrap().status, "done");
                } else {
                    assert!(finished.is_err(), "{fault}");
                    assert_eq!(
                        f.store.get("redo-result").unwrap().unwrap().status,
                        "pending"
                    );
                }
            }
            assert!(f.store.conn.is_autocommit());
            assert_eq!(
                f.store
                    .get("recovery-source")
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .as_deref(),
                Some("recover-undo")
            );
            assert_eq!(fs::read(&f.path).unwrap(), before_document);
            assert_eq!(fs::read(&registry_path).unwrap(), before_registry);
        }
    }

    #[test]
    fn copy_undo_recovery_retries_after_process_exit() {
        use std::os::unix::fs::MetadataExt;
        use std::process::{Command, Stdio};
        let mut f = interrupted_copy_undo_fixture();
        let source = f.store.get("recovery-source").unwrap().unwrap();
        let intent: crate::skill_copy_repair::CopyRepairIntent =
            serde_json::from_value(source.payload.clone()).unwrap();
        let registry_before = fs::read(&intent.registry_path).unwrap();
        let restored_registry = intent
            .transition
            .roll_back_document(&registry_before)
            .unwrap();
        let backup_paths = ["recovery-source", "recover-undo"]
            .into_iter()
            .flat_map(|id| {
                ["0-SKILL.md", "1-skill-studio.json", "manifest.json"]
                    .map(|name| f.store.app_data.join("backups").join(id).join(name))
            })
            .collect::<Vec<_>>();
        let backups_before = backup_paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "skill_repair_execution::tests::copy_undo_recovery_crash_child",
                "--ignored",
            ])
            .env("SKILL_STUDIO_REPAIR_CRASH_ROOT", f._temp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("undo recovery child timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(81));
        assert_eq!(fs::read(&f.path).unwrap(), f.original);
        assert_eq!(fs::read(&intent.registry_path).unwrap(), restored_registry);
        assert_eq!(
            f.store.get("recover-undo").unwrap().unwrap().status,
            "pending"
        );
        assert_eq!(
            f.store
                .get("recovery-source")
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some("recover-undo")
        );
        let identity = |path: &std::path::Path| {
            let m = fs::metadata(path).unwrap();
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        let document_identity = identity(&f.path);
        let registry_identity = identity(&intent.registry_path);
        assert!(matches!(
            recover_next_repair(
                &mut f.service,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .unwrap(),
            RepairRecoveryStep::Resolved {
                outcome: RepairRecoveryOutcome::Applied,
                ..
            }
        ));
        assert_eq!(f.store.get("recover-undo").unwrap().unwrap().status, "done");
        assert_eq!(
            f.store
                .get("recovery-source")
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some("recover-undo")
        );
        assert_eq!(identity(&f.path), document_identity);
        assert_eq!(identity(&intent.registry_path), registry_identity);
        assert_eq!(fs::read(&f.path).unwrap(), f.original);
        assert_eq!(fs::read(&intent.registry_path).unwrap(), restored_registry);
        for (path, before) in backup_paths.iter().zip(backups_before) {
            assert_eq!(fs::read(path).unwrap(), before);
        }
    }

    #[test]
    #[ignore = "explicit child for copy undo recovery process exit"]
    fn copy_undo_recovery_crash_child() {
        let root = PathBuf::from(std::env::var_os("SKILL_STUDIO_REPAIR_CRASH_ROOT").unwrap());
        let store = EventStore::open(&root.join("state")).unwrap();
        store
            .conn
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        let mut service = ScopedSkillService::bind(SkillScope {
            home: root.join("home"),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        })
        .unwrap();
        let source = store.get("recovery-source").unwrap().unwrap();
        let undo = store.get("recover-undo").unwrap().unwrap();
        let prepared = service
            .prepare_copy_undo_recovery(
                &source,
                &undo,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        recover_copy_undo_with(prepared, &store, || std::process::exit(81)).unwrap();
        panic!("undo recovery registry checkpoint not reached");
    }

    #[test]
    fn copy_redo_recovery_retries_after_process_exit() {
        use std::os::unix::fs::MetadataExt;
        use std::process::{Command, Stdio};
        let mut f = interrupted_copy_redo_fixture(RepairExecutionStage::Document);
        let source = f.store.get("recovery-source").unwrap().unwrap();
        let intent: crate::skill_copy_repair::CopyRepairIntent =
            serde_json::from_value(source.payload.clone()).unwrap();
        let registry_before = fs::read(&intent.registry_path).unwrap();
        let repaired_registry = intent.transition.apply_document(&registry_before).unwrap();
        let backup_paths = ["recovery-source", "recover-undo", "recover-redo"]
            .into_iter()
            .flat_map(|id| {
                ["0-SKILL.md", "1-skill-studio.json", "manifest.json"]
                    .map(|name| f.store.app_data.join("backups").join(id).join(name))
            })
            .collect::<Vec<_>>();
        let backups_before = backup_paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "skill_repair_execution::tests::copy_redo_recovery_crash_child",
                "--ignored",
            ])
            .env("SKILL_STUDIO_REPAIR_CRASH_ROOT", f._temp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("redo recovery child timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(83));
        assert_eq!(
            fs::read(&f.path).unwrap(),
            intent.document.proposed_content.as_bytes()
        );
        assert_eq!(fs::read(&intent.registry_path).unwrap(), repaired_registry);
        assert_eq!(
            f.store.get("recover-redo").unwrap().unwrap().status,
            "pending"
        );
        assert_eq!(
            f.store
                .get("recovery-source")
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some("recover-undo")
        );
        assert_eq!(
            f.store
                .get("recover-undo")
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some("recover-redo")
        );
        let identity = |path: &std::path::Path| {
            let m = fs::metadata(path).unwrap();
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        let document_identity = identity(&f.path);
        let registry_identity = identity(&intent.registry_path);
        assert!(matches!(
            recover_next_repair(
                &mut f.service,
                &f.store,
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .unwrap(),
            RepairRecoveryStep::Resolved {
                outcome: RepairRecoveryOutcome::Applied,
                ..
            }
        ));
        assert_eq!(f.store.get("recover-redo").unwrap().unwrap().status, "done");
        assert_eq!(
            f.store
                .get("recovery-source")
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some("recover-undo")
        );
        assert_eq!(
            f.store
                .get("recover-undo")
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some("recover-redo")
        );
        assert_eq!(identity(&f.path), document_identity);
        assert_eq!(identity(&intent.registry_path), registry_identity);
        assert_eq!(
            fs::read(&f.path).unwrap(),
            intent.document.proposed_content.as_bytes()
        );
        assert_eq!(fs::read(&intent.registry_path).unwrap(), repaired_registry);
        for (path, before) in backup_paths.iter().zip(backups_before) {
            assert_eq!(fs::read(path).unwrap(), before);
        }
    }

    #[test]
    #[ignore = "explicit child for copy redo recovery process exit"]
    fn copy_redo_recovery_crash_child() {
        let root = PathBuf::from(std::env::var_os("SKILL_STUDIO_REPAIR_CRASH_ROOT").unwrap());
        let store = EventStore::open(&root.join("state")).unwrap();
        store
            .conn
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        let mut service = ScopedSkillService::bind(SkillScope {
            home: root.join("home"),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        })
        .unwrap();
        let source = store.get("recovery-source").unwrap().unwrap();
        let undo = store.get("recover-undo").unwrap().unwrap();
        let redo = store.get("recover-redo").unwrap().unwrap();
        let prepared = service
            .prepare_copy_redo_recovery(
                &source,
                &undo,
                &redo,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        recover_copy_redo_with(prepared, &store, || std::process::exit(83)).unwrap();
        panic!("redo recovery registry checkpoint not reached");
    }

    #[test]
    fn copy_undo_recovery_refuses_changed_evidence() {
        for change in [
            "source",
            "undo",
            "document",
            "registry",
            "resource",
            "source-backup",
            "undo-backup",
            "source-after-write",
            "undo-after-write",
        ] {
            let mut f = interrupted_copy_undo_fixture();
            let source = f.store.get("recovery-source").unwrap().unwrap();
            let undo = f.store.get("recover-undo").unwrap().unwrap();
            let prepared = f
                .service
                .prepare_copy_undo_recovery(
                    &source,
                    &undo,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let registry_path = f._temp.path().join("home/.agents/skill-studio.json");
            match change {
                "source" | "undo" => {
                    let id = if change == "source" {
                        "recovery-source"
                    } else {
                        "recover-undo"
                    };
                    f.store
                        .conn
                        .execute("UPDATE events SET payload = '{}' WHERE id = ?1", [id])
                        .unwrap();
                }
                "document" => fs::write(&f.path, b"changed document").unwrap(),
                "registry" => fs::write(&registry_path, b"{}").unwrap(),
                "resource" => {
                    fs::write(f.path.parent().unwrap().join("new-resource"), b"added").unwrap()
                }
                "source-backup" | "undo-backup" => {
                    let id = if change == "source-backup" {
                        "recovery-source"
                    } else {
                        "recover-undo"
                    };
                    fs::write(
                        f.store.app_data.join("backups").join(id).join("0-SKILL.md"),
                        b"changed backup",
                    )
                    .unwrap();
                }
                _ => {}
            }
            let before_document = fs::read(&f.path).unwrap();
            let before_registry = fs::read(&registry_path).unwrap();
            let failure = recover_copy_undo_with(prepared, &f.store, || {
                if change.ends_with("after-write") {
                    let id = if change == "source-after-write" {
                        "recovery-source"
                    } else {
                        "recover-undo"
                    };
                    f.store
                        .conn
                        .execute("UPDATE events SET payload = '{}' WHERE id = ?1", [id])
                        .unwrap();
                }
            })
            .unwrap_err();
            assert_eq!(
                failure.stage,
                if change.ends_with("after-write") {
                    RepairExecutionStage::Finish
                } else {
                    RepairExecutionStage::Prepare
                },
                "{change}"
            );
            assert_eq!(fs::read(&f.path).unwrap(), before_document);
            if change.ends_with("after-write") {
                let intent: crate::skill_copy_repair::CopyRepairIntent =
                    serde_json::from_value(source.payload.clone()).unwrap();
                assert_eq!(
                    fs::read(&registry_path).unwrap(),
                    intent
                        .transition
                        .roll_back_document(&before_registry)
                        .unwrap()
                );
            } else {
                assert_eq!(fs::read(&registry_path).unwrap(), before_registry);
            }
            assert_eq!(
                f.store.get("recover-undo").unwrap().unwrap().status,
                "pending"
            );
            assert_eq!(
                f.store
                    .get("recovery-source")
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .as_deref(),
                Some("recover-undo")
            );
        }
    }

    #[test]
    fn copy_undo_recovery_resolves_states_and_retries_atomic_completion() {
        for (stage, fault) in [
            (RepairExecutionStage::Intent, "none"),
            (RepairExecutionStage::Document, "none"),
            (RepairExecutionStage::Registry, "none"),
            (RepairExecutionStage::Intent, "release"),
            (RepairExecutionStage::Document, "finish"),
            (RepairExecutionStage::Document, "claim"),
        ] {
            let mut f = copy_fixture();
            let prepared = f
                .service
                .prepare_copy_repair_selection(
                    &f.request,
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            execute_copy_repair(prepared, &f.store, "recovery-source").unwrap();
            let source = f.store.get("recovery-source").unwrap().unwrap();
            let repaired = fs::read(&f.path).unwrap();
            let cancel = CancellationToken::default();
            let prepared = f
                .service
                .prepare_copy_repair_undo(
                    &source,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    cancel.clone(),
                )
                .unwrap();
            execute_copy_repair_undo_with(prepared, &f.store, "recover-undo", |at| {
                if at == stage {
                    cancel.cancel();
                }
            })
            .unwrap_err();
            if stage == RepairExecutionStage::Registry {
                f.store
                    .conn
                    .execute(
                        "UPDATE events SET status = 'interrupted' WHERE id = 'recover-undo'",
                        [],
                    )
                    .unwrap();
            }
            let source = f.store.get("recovery-source").unwrap().unwrap();
            let undo = f.store.get("recover-undo").unwrap().unwrap();
            let registry_path = f._temp.path().join("home/.agents/skill-studio.json");
            let mut registry: serde_json::Value =
                serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
            registry["later_preference"] = serde_json::json!("keep");
            fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
            let prepared = f
                .service
                .prepare_copy_undo_recovery(
                    &source,
                    &undo,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let before_document = fs::read(&f.path).unwrap();
            let before_registry = fs::read(&registry_path).unwrap();
            match fault {
                "release" => f.store.conn.execute_batch("CREATE TRIGGER reject_recovery BEFORE UPDATE OF reverted_by ON events WHEN NEW.reverted_by IS NULL BEGIN SELECT RAISE(FAIL, 'release rejected'); END;").unwrap(),
                "finish" => f.store.conn.execute_batch("CREATE TRIGGER reject_recovery BEFORE UPDATE OF status ON events WHEN NEW.status = 'done' BEGIN SELECT RAISE(FAIL, 'finish rejected'); END;").unwrap(),
                "claim" => { f.store.conn.execute("UPDATE events SET reverted_by = 'other' WHERE id = 'recovery-source'", []).unwrap(); }
                _ => {}
            }
            let result = if fault == "none" {
                drop(prepared);
                match recover_next_repair(
                    &mut f.service,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap()
                {
                    RepairRecoveryStep::Resolved { event_id, outcome } => {
                        assert_eq!(event_id, "recover-undo");
                        Ok(outcome)
                    }
                    RepairRecoveryStep::Idle => panic!("Undo recovery was skipped"),
                }
            } else {
                recover_copy_undo(prepared, &f.store)
            };
            if fault == "claim" {
                assert_eq!(result.unwrap_err().stage, RepairExecutionStage::Prepare);
                assert_eq!(fs::read(&f.path).unwrap(), before_document);
                assert_eq!(fs::read(&registry_path).unwrap(), before_registry);
                assert_eq!(
                    f.store
                        .get("recovery-source")
                        .unwrap()
                        .unwrap()
                        .reverted_by
                        .as_deref(),
                    Some("other")
                );
                assert_eq!(
                    f.store.get("recover-undo").unwrap().unwrap().status,
                    "pending"
                );
                continue;
            }
            let result = if fault != "none" {
                assert_eq!(result.unwrap_err().stage, RepairExecutionStage::Finish);
                assert_eq!(
                    f.store.get("recover-undo").unwrap().unwrap().status,
                    "pending"
                );
                assert_eq!(
                    f.store
                        .get("recovery-source")
                        .unwrap()
                        .unwrap()
                        .reverted_by
                        .as_deref(),
                    Some("recover-undo")
                );
                f.store
                    .conn
                    .execute_batch("DROP TRIGGER reject_recovery;")
                    .unwrap();
                let prepared = f
                    .service
                    .prepare_copy_undo_recovery(
                        &source,
                        &undo,
                        &f.store,
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                recover_copy_undo(prepared, &f.store)
            } else {
                result
            };
            let applied = stage != RepairExecutionStage::Intent;
            assert_eq!(
                result.unwrap(),
                if applied {
                    RepairRecoveryOutcome::Applied
                } else {
                    RepairRecoveryOutcome::NotApplied
                }
            );
            assert_eq!(
                fs::read(&f.path).unwrap(),
                if applied {
                    f.original.clone()
                } else {
                    repaired
                }
            );
            let registry: serde_json::Value =
                serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
            assert_eq!(registry["later_preference"], "keep");
            assert_eq!(
                f.store.get("recover-undo").unwrap().unwrap().status,
                if applied { "done" } else { "failed" }
            );
            assert_eq!(
                f.store
                    .get("recovery-source")
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .as_deref(),
                if applied { Some("recover-undo") } else { None }
            );
            assert!(matches!(
                recover_next_repair(
                    &mut f.service,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default()
                )
                .unwrap(),
                RepairRecoveryStep::Idle
            ));
            assert!(f
                .service
                .prepare_copy_undo_recovery(
                    &f.store.get("recovery-source").unwrap().unwrap(),
                    &f.store.get("recover-undo").unwrap().unwrap(),
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .is_err());
        }
    }

    #[test]
    fn copy_undo_process_exit_preserves_claim_and_both_backups() {
        use std::process::{Command, Stdio};
        for stage in ["intent", "document", "registry"] {
            let mut f = copy_fixture();
            let prepared = f
                .service
                .prepare_copy_repair_selection(
                    &f.request,
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            execute_copy_repair(prepared, &f.store, "undo-crash-source").unwrap();
            let row = f.store.get("undo-crash-source").unwrap().unwrap();
            let intent: crate::skill_copy_repair::CopyRepairIntent =
                serde_json::from_value(row.payload).unwrap();
            let repaired_document = fs::read(&f.path).unwrap();
            let repaired_registry = fs::read(&intent.registry_path).unwrap();
            let restored_registry = intent
                .transition
                .roll_back_document(&repaired_registry)
                .unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "skill_repair_execution::tests::copy_undo_crash_child",
                    "--ignored",
                ])
                .env("SKILL_STUDIO_REPAIR_CRASH_ROOT", f._temp.path())
                .env("SKILL_STUDIO_REPAIR_CRASH_STAGE", stage)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("undo child timed out");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(status.code(), Some(80));
            assert_eq!(
                f.store.get("undo-crashed").unwrap().unwrap().status,
                "pending"
            );
            assert_eq!(
                f.store
                    .get("undo-crash-source")
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .as_deref(),
                Some("undo-crashed")
            );
            assert_eq!(
                fs::read(&f.path).unwrap(),
                if stage == "intent" {
                    repaired_document.clone()
                } else {
                    f.original.clone()
                }
            );
            assert_eq!(
                fs::read(&intent.registry_path).unwrap(),
                if stage == "registry" {
                    restored_registry
                } else {
                    repaired_registry.clone()
                }
            );
            let source_row = f.store.get("undo-crash-source").unwrap().unwrap();
            let undo_row = f.store.get("undo-crashed").unwrap().unwrap();
            let recovery = crate::skill_repair_recovery_event::CopyUndoRecoveryEvent::from_rows(
                &source_row,
                &undo_row,
            )
            .unwrap();
            let skill_dir = f.path.parent().unwrap();
            let scope =
                crate::skill_scope::SkillReadScope::bind(&[skill_dir.to_path_buf()]).unwrap();
            let hash = crate::skill_discovery::live_skill_content_hash(&scope, skill_dir).unwrap();
            let observed = recovery
                .intent()
                .classify_undo_observed(
                    &fs::read(&f.path).unwrap(),
                    &fs::read(&intent.registry_path).unwrap(),
                    &hash,
                )
                .unwrap();
            use crate::skill_copy_repair::CopyUndoObservedState;
            assert_eq!(
                observed,
                match stage {
                    "intent" => CopyUndoObservedState::Unchanged,
                    "document" => CopyUndoObservedState::DocumentRestored,
                    "registry" => CopyUndoObservedState::Restored,
                    _ => unreachable!(),
                }
            );
            let prepared = f
                .service
                .prepare_copy_undo_recovery(
                    &source_row,
                    &undo_row,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            assert_eq!(prepared.state(), observed);
            prepared.revalidate().unwrap();
            let added_resource = skill_dir.join("after-undo-preparation.txt");
            fs::write(&added_resource, b"unexpected resource").unwrap();
            assert!(prepared.revalidate().is_err());
            drop(prepared);
            assert!(f
                .service
                .prepare_copy_undo_recovery(
                    &source_row,
                    &undo_row,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .is_err());
            fs::remove_file(added_resource).unwrap();
            for change in ["claim", "target", "repair", "status", "backup"] {
                let mut source = source_row.clone();
                let mut undo = undo_row.clone();
                match change {
                    "claim" => source.reverted_by = Some("another".into()),
                    "target" => undo.payload["target_event"] = serde_json::json!("other"),
                    "repair" => {
                        undo.payload["repair"]["document"]["name"] = serde_json::json!("other")
                    }
                    "status" => undo.status = "done".into(),
                    "backup" => undo.backup_dir = Some("backups/other".into()),
                    _ => unreachable!(),
                }
                assert!(
                    crate::skill_repair_recovery_event::CopyUndoRecoveryEvent::from_rows(
                        &source, &undo
                    )
                    .is_err(),
                    "{change}"
                );
            }
            use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
            let state_scope =
                crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&f.store.app_data))
                    .unwrap();
            let lease = CoordinationPlan::new(
                vec![DirectoryEffect::tree(
                    &f.store.app_data,
                    CoordinationMode::Exclusive,
                )],
                Some(Duration::from_secs(10)),
            )
            .unwrap()
            .acquire()
            .unwrap()
            .finalize_write(&state_scope, &[])
            .unwrap();
            let verified = crate::skill_repair_backup::VerifiedCopyUndoBackups::read(
                &f.store.app_data,
                &recovery,
                &lease,
            )
            .unwrap();
            assert_eq!(verified.repair_originals().0, f.original);
            assert_eq!(
                verified.undo_originals(),
                (repaired_document.as_slice(), repaired_registry.as_slice())
            );
            verified.revalidate(&lease).unwrap();
            let backup = f.store.app_data.join("backups/undo-crashed");
            assert_eq!(
                fs::read(backup.join("0-SKILL.md")).unwrap(),
                repaired_document
            );
            assert_eq!(
                fs::read(backup.join("1-skill-studio.json")).unwrap(),
                repaired_registry
            );
            match stage {
                "intent" => {
                    fs::write(backup.join("0-SKILL.md"), &f.original).unwrap();
                    let manifest_path = backup.join("manifest.json");
                    let mut manifest: crate::skill_event::BackupManifest =
                        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
                    manifest
                        .entries
                        .get_mut(f.path.to_str().unwrap())
                        .unwrap()
                        .fingerprint = fingerprint_regular_bytes(&f.original);
                    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
                }
                "document" => {
                    fs::hard_link(backup.join("1-skill-studio.json"), backup.join("alias")).unwrap()
                }
                "registry" => fs::write(
                    f.store
                        .app_data
                        .join("backups/undo-crash-source/0-SKILL.md"),
                    b"changed",
                )
                .unwrap(),
                _ => unreachable!(),
            }
            assert!(verified.revalidate(&lease).is_err());
            assert!(crate::skill_repair_backup::VerifiedCopyUndoBackups::read(
                &f.store.app_data,
                &recovery,
                &lease
            )
            .is_err());
        }
    }

    #[test]
    #[ignore = "explicit child for copy undo process exit"]
    fn copy_undo_crash_child() {
        let root = PathBuf::from(std::env::var_os("SKILL_STUDIO_REPAIR_CRASH_ROOT").unwrap());
        let stage = std::env::var("SKILL_STUDIO_REPAIR_CRASH_STAGE").unwrap();
        let store = EventStore::open(&root.join("state")).unwrap();
        store
            .conn
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        let mut service = ScopedSkillService::bind(SkillScope {
            home: root.join("home"),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        })
        .unwrap();
        let row = store.get("undo-crash-source").unwrap().unwrap();
        let prepared = service
            .prepare_copy_repair_undo(
                &row,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        execute_copy_repair_undo_with(prepared, &store, "undo-crashed", |checkpoint| {
            if matches!(
                (stage.as_str(), checkpoint),
                ("intent", RepairExecutionStage::Intent)
                    | ("document", RepairExecutionStage::Document)
                    | ("registry", RepairExecutionStage::Registry)
            ) {
                std::process::exit(80);
            }
        })
        .unwrap();
        panic!("undo checkpoint not reached");
    }

    fn interrupted_copy_fixture() -> Fixture {
        let mut f = copy_fixture();
        let cancel = CancellationToken::default();
        let prepared = f
            .service
            .prepare_copy_repair_selection(
                &f.request,
                std::slice::from_ref(&f.store.app_data),
                Some(Duration::from_secs(10)),
                cancel.clone(),
            )
            .unwrap();
        assert!(
            execute_copy_repair_with(prepared, &f.store, "recover-copy", |stage| {
                if stage == RepairExecutionStage::Document {
                    cancel.cancel();
                }
            })
            .is_err()
        );
        f
    }

    #[test]
    fn copy_recovery_retry_finishes_after_process_exit_or_sql_failure() {
        use std::process::{Command, Stdio};
        for failure in ["process-exit", "sql"] {
            let mut f = interrupted_copy_fixture();
            let row = f.store.get("recover-copy").unwrap().unwrap();
            let intent: crate::skill_copy_repair::CopyRepairIntent =
                serde_json::from_value(row.payload.clone()).unwrap();
            let original_registry = fs::read(&intent.registry_path).unwrap();
            let proposed_registry = intent
                .transition
                .apply_document(&original_registry)
                .unwrap();
            if failure == "process-exit" {
                let mut child = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "skill_repair_execution::tests::copy_recovery_crash_child",
                        "--ignored",
                    ])
                    .env("SKILL_STUDIO_REPAIR_CRASH_ROOT", f._temp.path())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                let status = loop {
                    if let Some(status) = child.try_wait().unwrap() {
                        break status;
                    }
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!("recovery child timed out");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                };
                assert_eq!(status.code(), Some(79));
            } else {
                f.store.conn.execute_batch("CREATE TRIGGER reject_recovery BEFORE UPDATE OF status ON events WHEN NEW.status = 'done' BEGIN SELECT RAISE(FAIL, 'injected recovery completion failure'); END;").unwrap();
                let prepared = f
                    .service
                    .prepare_copy_repair_recovery(
                        &row,
                        &f.store,
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                assert_eq!(
                    recover_copy_repair(prepared, &f.store).unwrap_err().stage,
                    RepairExecutionStage::Finish
                );
                f.store
                    .conn
                    .execute_batch("DROP TRIGGER reject_recovery")
                    .unwrap();
            }
            assert_eq!(fs::read(&intent.registry_path).unwrap(), proposed_registry);
            assert_eq!(
                fs::read(&f.path).unwrap(),
                intent.document.proposed_content.as_bytes()
            );
            assert_eq!(
                f.store.get("recover-copy").unwrap().unwrap().status,
                "pending"
            );
            assert!(matches!(
                recover_next_repair(
                    &mut f.service,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default()
                )
                .unwrap(),
                RepairRecoveryStep::Resolved {
                    outcome: RepairRecoveryOutcome::Applied,
                    ..
                }
            ));
            assert_eq!(f.store.get("recover-copy").unwrap().unwrap().status, "done");
            assert_eq!(fs::read(&intent.registry_path).unwrap(), proposed_registry);
        }
    }

    #[test]
    #[ignore = "explicit child for copy recovery process exit"]
    fn copy_recovery_crash_child() {
        let root = PathBuf::from(std::env::var_os("SKILL_STUDIO_REPAIR_CRASH_ROOT").unwrap());
        let store = EventStore::open(&root.join("state")).unwrap();
        store
            .conn
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        let mut service = ScopedSkillService::bind(SkillScope {
            home: root.join("home"),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        })
        .unwrap();
        let row = store.get("recover-copy").unwrap().unwrap();
        let prepared = service
            .prepare_copy_repair_recovery(
                &row,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        recover_copy_repair_with(prepared, &store, || std::process::exit(79)).unwrap();
        panic!("recovery registry checkpoint not reached");
    }

    #[test]
    fn copy_recovery_refuses_changes_after_preparation() {
        for change in ["event", "registry", "document", "resource", "backup"] {
            let mut f = copy_fixture();
            let cancel = CancellationToken::default();
            let prepared = f
                .service
                .prepare_copy_repair_selection(
                    &f.request,
                    std::slice::from_ref(&f.store.app_data),
                    Some(Duration::from_secs(10)),
                    cancel.clone(),
                )
                .unwrap();
            assert!(
                execute_copy_repair_with(prepared, &f.store, "drift-copy", |stage| {
                    if stage == RepairExecutionStage::Document {
                        cancel.cancel();
                    }
                })
                .is_err()
            );
            let row = f.store.get("drift-copy").unwrap().unwrap();
            let prepared = f
                .service
                .prepare_copy_repair_recovery(
                    &row,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let registry_path = f._temp.path().join("home/.agents/skill-studio.json");
            match change {
                "event" => {
                    f.store
                        .conn
                        .execute(
                            "UPDATE events SET payload = '{}' WHERE id = 'drift-copy'",
                            [],
                        )
                        .unwrap();
                }
                "registry" => fs::write(&registry_path, b"{}").unwrap(),
                "document" => fs::write(&f.path, b"external edit").unwrap(),
                "resource" => {
                    fs::write(f.path.parent().unwrap().join("new-resource"), b"new").unwrap()
                }
                "backup" => fs::write(
                    f.store
                        .app_data
                        .join("backups/drift-copy/1-skill-studio.json"),
                    b"changed",
                )
                .unwrap(),
                _ => unreachable!(),
            }
            let before_document = fs::read(&f.path).unwrap();
            let before_registry = fs::read(&registry_path).unwrap();
            assert!(recover_copy_repair(prepared, &f.store).is_err(), "{change}");
            assert_eq!(fs::read(&f.path).unwrap(), before_document);
            assert_eq!(fs::read(&registry_path).unwrap(), before_registry);
            assert_eq!(
                f.store.get("drift-copy").unwrap().unwrap().status,
                "pending"
            );
        }
    }

    #[test]
    fn copy_repair_process_exit_retains_each_publication_state() {
        use std::process::{Command, Stdio};
        for stage in ["intent", "document", "registry"] {
            let mut f = copy_fixture();
            fs::write(
                f._temp.path().join("request.json"),
                serde_json::to_vec(&f.request).unwrap(),
            )
            .unwrap();
            let registry_path = f._temp.path().join("home/.agents/skill-studio.json");
            let registry_original = fs::read(&registry_path).unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "skill_repair_execution::tests::copy_crash_child",
                    "--ignored",
                ])
                .env("SKILL_STUDIO_REPAIR_CRASH_ROOT", f._temp.path())
                .env("SKILL_STUDIO_REPAIR_CRASH_STAGE", stage)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("copy child timed out");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(status.code(), Some(78));
            let row = f.store.get("copy-crashed").unwrap().unwrap();
            assert_eq!(row.status, "pending");
            let intent: crate::skill_copy_repair::CopyRepairIntent =
                serde_json::from_value(row.payload.clone()).unwrap();
            let document_expected = if stage == "intent" {
                f.original.as_slice()
            } else {
                intent.document.proposed_content.as_bytes()
            };
            assert_eq!(fs::read(&f.path).unwrap(), document_expected);
            let registry_expected = if stage == "registry" {
                intent
                    .transition
                    .apply_document(&registry_original)
                    .unwrap()
            } else {
                registry_original.clone()
            };
            assert_eq!(fs::read(&registry_path).unwrap(), registry_expected);
            let skill_dir = f.path.parent().unwrap();
            let scope =
                crate::skill_scope::SkillReadScope::bind(&[skill_dir.to_path_buf()]).unwrap();
            let hash = crate::skill_discovery::live_skill_content_hash(&scope, skill_dir).unwrap();
            let observed = intent
                .classify_observed(document_expected, &registry_expected, &hash)
                .unwrap();
            use crate::skill_copy_repair::CopyRepairObservedState;
            let expected_state = match stage {
                "intent" => CopyRepairObservedState::Original,
                "document" => CopyRepairObservedState::DocumentApplied,
                "registry" => CopyRepairObservedState::Applied,
                _ => unreachable!(),
            };
            assert_eq!(observed, expected_state);
            let mut preferences: serde_json::Value =
                serde_json::from_slice(&registry_expected).unwrap();
            preferences["future_preference"] = serde_json::json!({"keep": true});
            let preferences = serde_json::to_vec(&preferences).unwrap();
            assert_eq!(
                intent
                    .classify_observed(document_expected, &preferences, &hash)
                    .unwrap(),
                expected_state
            );
            assert!(intent
                .classify_observed(b"external edit", &registry_expected, &hash)
                .is_err());
            assert!(intent
                .classify_observed(document_expected, &registry_expected, &"f".repeat(64))
                .is_err());
            let mut changed: serde_json::Value =
                serde_json::from_slice(&registry_expected).unwrap();
            changed["copies"][&intent.document.deployment_id]["disabled"] = serde_json::json!(true);
            assert!(intent
                .classify_observed(
                    document_expected,
                    &serde_json::to_vec(&changed).unwrap(),
                    &hash
                )
                .is_err());
            if stage == "intent" {
                let ahead = intent
                    .transition
                    .apply_document(&registry_original)
                    .unwrap();
                assert!(intent
                    .classify_observed(document_expected, &ahead, &hash)
                    .is_err());
            }
            let backup = f.store.app_data.join("backups/copy-crashed");
            let manifest: crate::skill_event::BackupManifest =
                serde_json::from_slice(&fs::read(backup.join("manifest.json")).unwrap()).unwrap();
            assert_eq!(manifest.entries.len(), 2);
            use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
            let prepared = f
                .service
                .prepare_copy_repair_recovery(
                    &row,
                    &f.store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            assert_eq!(prepared.state(), expected_state);
            prepared.revalidate().unwrap();
            drop(prepared);
            let state_scope =
                crate::skill_scope::SkillReadScope::bind(std::slice::from_ref(&f.store.app_data))
                    .unwrap();
            let lease = CoordinationPlan::new(
                vec![DirectoryEffect::tree(
                    &f.store.app_data,
                    CoordinationMode::Exclusive,
                )],
                Some(Duration::from_secs(10)),
            )
            .unwrap()
            .acquire()
            .unwrap()
            .finalize_write(&state_scope, &[])
            .unwrap();
            let event = crate::skill_repair_recovery_event::CopyRepairRecoveryEvent::from_row(&row)
                .unwrap();
            for change in [
                "status",
                "kind",
                "skill",
                "backup",
                "claim",
                "inverse",
                "restorable",
            ] {
                let mut altered = row.clone();
                match change {
                    "status" => altered.status = "done".into(),
                    "kind" => altered.kind = "repair_skill_frontmatter".into(),
                    "skill" => altered.skill = "other".into(),
                    "backup" => altered.backup_dir = Some("backups/other".into()),
                    "claim" => altered.reverted_by = Some("other".into()),
                    "inverse" => altered.inverse = Some(serde_json::json!({})),
                    "restorable" => altered.restorable = true,
                    _ => unreachable!(),
                }
                assert!(
                    crate::skill_repair_recovery_event::CopyRepairRecoveryEvent::from_row(&altered)
                        .is_err(),
                    "{change}"
                );
            }
            let verified = crate::skill_repair_backup::VerifiedCopyRepairBackup::read(
                &f.store.app_data,
                &event,
                &lease,
            )
            .unwrap();
            assert_eq!(
                verified.originals(),
                (f.original.as_slice(), registry_original.as_slice())
            );
            verified.revalidate(&lease).unwrap();

            for (path, original) in [(&f.path, &f.original), (&registry_path, &registry_original)] {
                let entry = &manifest.entries[path.to_str().unwrap()];
                assert_eq!(
                    fs::read(backup.join(&entry.relative_path)).unwrap(),
                    *original
                );
            }
            let events = GuardedEventStore::bind(&f.store, &lease).unwrap();
            if stage == "document" {
                f.store
                    .conn
                    .execute(
                        "UPDATE events SET status = 'interrupted' WHERE id = 'copy-crashed'",
                        [],
                    )
                    .unwrap();
                assert!(events
                    .finish_copy_recovery(&lease, &event, EventStatus::Done)
                    .is_err());
                assert_eq!(
                    f.store.get("copy-crashed").unwrap().unwrap().status,
                    "interrupted"
                );
            } else {
                let status = if stage == "intent" {
                    EventStatus::Failed
                } else {
                    EventStatus::Done
                };
                events.finish_copy_recovery(&lease, &event, status).unwrap();
                assert!(events
                    .finish_copy_recovery(&lease, &event, EventStatus::Failed)
                    .is_err());
                assert_eq!(
                    f.store.get("copy-crashed").unwrap().unwrap().status,
                    if stage == "intent" { "failed" } else { "done" }
                );
            }
            let registry_backup = backup.join("1-skill-studio.json");
            match stage {
                "intent" => fs::write(&registry_backup, b"changed").unwrap(),
                "document" => {
                    fs::hard_link(&registry_backup, backup.join("alias")).unwrap();
                }
                "registry" => {
                    fs::rename(&registry_backup, backup.join("saved-registry")).unwrap();
                    std::os::unix::fs::symlink("saved-registry", &registry_backup).unwrap();
                }
                _ => unreachable!(),
            }
            assert!(verified.revalidate(&lease).is_err());
            assert!(crate::skill_repair_backup::VerifiedCopyRepairBackup::read(
                &f.store.app_data,
                &event,
                &lease
            )
            .is_err());
        }
    }

    #[test]
    #[ignore = "explicit child for copy abrupt-exit fixture"]
    fn copy_crash_child() {
        let root = PathBuf::from(std::env::var_os("SKILL_STUDIO_REPAIR_CRASH_ROOT").unwrap());
        let stage = std::env::var("SKILL_STUDIO_REPAIR_CRASH_STAGE").unwrap();
        let request =
            serde_json::from_slice(&fs::read(root.join("request.json")).unwrap()).unwrap();
        let state = std::env::var_os("SKILL_STUDIO_REPAIR_CRASH_STATE")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("state"));
        let store = EventStore::open(&state).unwrap();
        store
            .conn
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        let mut service = ScopedSkillService::bind(SkillScope {
            home: root.join("home"),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        })
        .unwrap();
        let prepared = service
            .prepare_copy_repair_selection(
                &request,
                std::slice::from_ref(&store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        execute_copy_repair_with(prepared, &store, "copy-crashed", |checkpoint| {
            if matches!(
                (stage.as_str(), checkpoint),
                ("intent", RepairExecutionStage::Intent)
                    | ("document", RepairExecutionStage::Document)
                    | ("registry", RepairExecutionStage::Registry)
            ) {
                std::process::exit(78);
            }
        })
        .unwrap();
        panic!("copy checkpoint not reached");
    }

    #[test]
    #[ignore = "explicit child for abrupt-exit fixture"]
    fn repair_crash_child() {
        let root = PathBuf::from(
            std::env::var_os("SKILL_STUDIO_REPAIR_CRASH_ROOT").expect("fixture root"),
        );
        let stage = std::env::var("SKILL_STUDIO_REPAIR_CRASH_STAGE").expect("fixture checkpoint");
        assert!(matches!(stage.as_str(), "intent" | "document"));
        let request =
            serde_json::from_slice(&fs::read(root.join("request.json")).unwrap()).unwrap();
        let store = EventStore::open(&root.join("state")).unwrap();
        store
            .conn
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        let mut service = ScopedSkillService::bind(SkillScope {
            home: root.join("home"),
            projects: vec![],
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        })
        .unwrap();
        let selection = service
            .prepare_repair_selection(
                &request,
                std::slice::from_ref(&store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        execute_direct_repair_with(selection, &store, "crashed", |checkpoint| {
            if (stage == "intent" && checkpoint == RepairExecutionStage::Intent)
                || (stage == "document" && checkpoint == RepairExecutionStage::Document)
            {
                std::process::exit(77);
            }
        })
        .unwrap();
        panic!("repair child skipped checkpoint");
    }

    #[test]
    fn edit_after_intent_preserves_edit_backup_and_pending_event() {
        let mut f = Fixture::new(false);
        let selection = f
            .service
            .prepare_repair_selection(
                &f.request,
                std::slice::from_ref(&f.store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        let failure = execute_direct_repair_with(selection, &f.store, "repair", |_| {
            fs::write(&f.path, b"external edit").unwrap();
        })
        .unwrap_err();
        assert_eq!(failure.stage, RepairExecutionStage::Document);
        assert_eq!(fs::read(&f.path).unwrap(), b"external edit");
        assert_eq!(f.store.get("repair").unwrap().unwrap().status, "pending");
        assert_eq!(
            fs::read(f.store.app_data.join("backups/repair/0-SKILL.md")).unwrap(),
            f.original
        );
    }
}
