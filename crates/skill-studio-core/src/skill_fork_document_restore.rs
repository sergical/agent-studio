//! Document-only restore retains fork ownership and provider state.
use crate::{
    skill_backup_reservation::{BackupCopyLimits, BackupStateRoot},
    skill_coordination::{CancellationToken, FinalizedWriteLease},
    skill_event::EventRow,
    skill_event_operations::GuardedEventStore,
    skill_event_store::EventStore,
    skill_fork_repair_intent::CompletedDotagentsForkEvent,
    skill_frontmatter_repair::content_fingerprint,
    skill_ownership::LifecycleOwnerKind,
    skill_service::{
        exact_repair_deployment, ScopedSkillService, WritePreparationError,
        MAX_REPAIR_DOCUMENT_BYTES,
    },
};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForkDocumentRegistryAdmission {
    version: u32,
    selected: serde_json::Value,
}

impl ForkDocumentRegistryAdmission {
    fn validate(&self, original: &crate::skill_fork_registry::ForkRecord) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported fork document registry admission version".into());
        }
        validate_admitted_registry_row(&self.selected, original)
    }
}

enum RegistryAdmissionMode<'a> {
    Strict,
    NewRestore,
    Recorded(Option<&'a ForkDocumentRegistryAdmission>),
}

fn registry_selected_row(
    lease: &FinalizedWriteLease<'_>,
    agents: &std::path::Path,
    name: &str,
) -> Result<serde_json::Value, String> {
    let bytes = lease
        .read_ownership_registry(&agents.join("skill-studio.json"), 8 * 1024 * 1024)?
        .ok_or("Fork registry is missing")?;
    let registry: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    registry
        .get("forks")
        .and_then(|forks| forks.get(name))
        .cloned()
        .ok_or_else(|| "Fork ownership record changed since the repair".into())
}

fn validate_admitted_registry_row(
    selected: &serde_json::Value,
    original: &crate::skill_fork_registry::ForkRecord,
) -> Result<(), String> {
    let selected_record: crate::skill_fork_registry::ForkRecord =
        serde_json::from_value(selected.clone()).map_err(|error| error.to_string())?;
    let mut expected = serde_json::to_value(original).map_err(|error| error.to_string())?;
    expected["base_commit"] = serde_json::Value::String(selected_record.base_commit.clone());
    if selected != &expected || !crate::skill_fork_pull::valid_commit(&selected_record.base_commit)
    {
        return Err("Fork ownership record changed since the repair".into());
    }
    Ok(())
}

fn select_registry_admission(
    lease: &FinalizedWriteLease<'_>,
    agents: &std::path::Path,
    name: &str,
    original: &crate::skill_fork_registry::ForkRecord,
    mode: RegistryAdmissionMode<'_>,
) -> Result<Option<ForkDocumentRegistryAdmission>, String> {
    let selected = registry_selected_row(lease, agents, name)?;
    match mode {
        RegistryAdmissionMode::Strict => {
            if selected != serde_json::to_value(original).map_err(|error| error.to_string())? {
                return Err("Fork ownership record changed since the repair".into());
            }
            Ok(None)
        }
        RegistryAdmissionMode::NewRestore => {
            validate_admitted_registry_row(&selected, original)?;
            if selected == serde_json::to_value(original).map_err(|error| error.to_string())? {
                Ok(None)
            } else {
                Ok(Some(ForkDocumentRegistryAdmission {
                    version: 1,
                    selected,
                }))
            }
        }
        RegistryAdmissionMode::Recorded(admission) => {
            if let Some(admission) = admission {
                admission.validate(original)?;
                if selected != admission.selected {
                    return Err("Fork ownership record changed since restore admission".into());
                }
                Ok(Some(admission.clone()))
            } else if selected
                != serde_json::to_value(original).map_err(|error| error.to_string())?
            {
                Err("Fork ownership record changed since the repair".into())
            } else {
                Ok(None)
            }
        }
    }
}

fn require_current_registry_admission(
    lease: &FinalizedWriteLease<'_>,
    agents: &std::path::Path,
    name: &str,
    original: &crate::skill_fork_registry::ForkRecord,
    admission: Option<&ForkDocumentRegistryAdmission>,
) -> Result<(), String> {
    select_registry_admission(
        lease,
        agents,
        name,
        original,
        RegistryAdmissionMode::Recorded(admission),
    )
    .map(|_| ())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkDocumentRestoreIntent {
    pub(crate) target_event: String,
    pub(crate) fork: crate::skill_fork_repair_intent::DotagentsForkRepairIntent,
    pub(crate) before: String,
    pub(crate) after: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) registry_admission: Option<ForkDocumentRegistryAdmission>,
}

impl ForkDocumentRestoreIntent {
    pub fn validate_record(&self, operation_id: &str) -> Result<(), String> {
        if !crate::skill_backup_reservation::valid_id(operation_id)
            || operation_id == self.target_event
            || !crate::skill_backup_reservation::valid_id(&self.target_event)
            || operation_id == self.fork.snapshots().operation_id()
            || [&self.before, &self.after].iter().any(|hash| {
                hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
        {
            return Err("Invalid fork document restore identity or fingerprint".into());
        }
        self.fork
            .validate_for_operation(self.fork.snapshots().operation_id())?;
        if let Some(admission) = &self.registry_admission {
            admission.validate(self.fork.registry().record())?;
        }
        Ok(())
    }
}

pub struct PendingForkDocumentRestore<'scope> {
    prepared: PreparedForkDocumentRestore<'scope>,
    source: EventRow,
    event: EventRow,
    overwritten: crate::skill_repair_backup::VerifiedRepairBackup,
}

impl PendingForkDocumentRestore<'_> {
    pub fn execute(
        mut self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<
        crate::skill_repair_execution::RepairExecutionReceipt,
        crate::skill_repair_execution::RepairExecutionError,
    > {
        use crate::skill_repair_execution::{
            RepairExecutionError, RepairExecutionReceipt, RepairExecutionStage,
        };
        let id = self.event.id.clone();
        let fail = |stage, message| RepairExecutionError {
            event_id: id.clone(),
            stage,
            message,
        };
        self.revalidate(store, limits, cancellation)
            .map_err(|message| fail(RepairExecutionStage::Prepare, message))?;
        let document = self.prepared.source.intent().repair().path.join("SKILL.md");
        let intent: ForkDocumentRestoreIntent = serde_json::from_value(self.event.payload.clone())
            .map_err(|error| fail(RepairExecutionStage::Prepare, error.to_string()))?;
        let inverse = serde_json::to_value(crate::skill_event::InverseOp::RestoreBackup {
            path: document.clone(),
            pre_fingerprint: intent.before,
            post_fingerprint: Some(intent.after),
        })
        .map_err(|error| fail(RepairExecutionStage::Prepare, error.to_string()))?;
        if cancellation.is_cancelled() {
            return Err(fail(
                RepairExecutionStage::Document,
                "Fork restore cancelled before publication".into(),
            ));
        }
        if self.prepared.current != self.prepared.original {
            crate::skill_document_target::SkillDocumentTarget::bind(
                &self.prepared.source.intent().repair().path,
            )
            .and_then(|target| {
                target
                    .replace(
                        &mut self.prepared.lease,
                        &self.prepared.current,
                        &self.prepared.original,
                    )
                    .map_err(|error| error.to_string())
            })
            .map_err(|message| fail(RepairExecutionStage::Document, message))?;
            self.prepared
                .lease
                .validate_published_document(&document, &self.prepared.original)
                .map_err(|message| fail(RepairExecutionStage::Finish, message))?;
        } else {
            let current = self
                .prepared
                .lease
                .read(&document, MAX_REPAIR_DOCUMENT_BYTES)
                .map_err(|error| fail(RepairExecutionStage::Finish, error.to_string()))?;
            if current != self.prepared.original {
                return Err(fail(
                    RepairExecutionStage::Finish,
                    "Restored document changed before completion".into(),
                ));
            }
        }
        self.revalidate(store, limits, cancellation)
            .map_err(|message| fail(RepairExecutionStage::Finish, message))?;
        GuardedEventStore::bind(store, &self.prepared.lease)
            .map_err(|message| fail(RepairExecutionStage::Finish, message))?
            .finish_fork_document_restore(&self.prepared.lease, &self.source, &self.event, inverse)
            .map_err(|error| fail(RepairExecutionStage::Finish, error.to_string()))?;
        Ok(RepairExecutionReceipt {
            event_id: id,
            deployment_id: self.prepared.source.intent().repair().deployment_id.clone(),
        })
    }

    pub fn revalidate(
        &self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        if store.app_data != self.prepared.state_path {
            return Err("Fork document restore belongs to another event store".into());
        }
        let lease = &self.prepared.lease;
        let guarded = GuardedEventStore::bind(store, lease)?;
        self.require_pending_history(store)?;
        let agents = self
            .prepared
            .source
            .intent()
            .repair()
            .path
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or("Fork restore has invalid ownership path")?;
        require_current_registry_admission(
            lease,
            agents,
            &self.prepared.source.intent().repair().name,
            self.prepared.source.intent().registry().record(),
            self.prepared.registry_admission.as_ref(),
        )?;
        guarded.verify_dotagents_fork_inputs(
            lease,
            self.prepared.source.intent(),
            limits,
            cancellation,
        )?;
        self.overwritten.revalidate(lease)?;
        if let Some(backup) = &self.prepared.target_backup {
            backup.revalidate(lease)?;
        }
        lease.revalidate().map_err(|error| error.to_string())?;
        self.require_pending_history(store)
    }

    fn require_pending_history(&self, store: &EventStore) -> Result<(), String> {
        for expected in [&self.source, &self.event]
            .into_iter()
            .chain(self.prepared.history.iter().skip(1))
        {
            let current = store
                .get(&expected.id)?
                .ok_or("Fork restore history is missing")?;
            if serde_json::to_value(current).map_err(|error| error.to_string())?
                != serde_json::to_value(expected).map_err(|error| error.to_string())?
            {
                return Err("Fork restore history changed after recording".into());
            }
        }
        let other_pending: bool = store.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE status IN ('pending', 'interrupted') AND id != ?1)",
            [&self.event.id],
            |row| row.get(0),
        ).map_err(|error| error.to_string())?;
        if other_pending {
            return Err("Another unfinished operation requires recovery".into());
        }
        Ok(())
    }

    pub fn source_event(&self) -> &EventRow {
        &self.source
    }
    pub fn event(&self) -> &EventRow {
        &self.event
    }
    pub fn current_content(&self) -> &[u8] {
        &self.prepared.current
    }
    pub fn restore_content(&self) -> &[u8] {
        &self.prepared.original
    }
}

pub struct PreparedForkDocumentRestore<'scope> {
    source: CompletedDotagentsForkEvent,
    history: Vec<EventRow>,
    target_backup: Option<crate::skill_repair_backup::VerifiedRepairBackup>,
    current: Vec<u8>,
    original: Vec<u8>,
    state_path: PathBuf,
    lease: FinalizedWriteLease<'scope>,
    registry_admission: Option<ForkDocumentRegistryAdmission>,
}

impl<'scope> PreparedForkDocumentRestore<'scope> {
    fn select_history(&mut self, history: Vec<EventRow>, store: &EventStore) -> Result<(), String> {
        require_unchanged_history(store, &history)?;
        if history.len() > 1 {
            let first_undo: ForkDocumentRestoreIntent =
                serde_json::from_value(history[history.len() - 2].payload.clone())
                    .map_err(|error| error.to_string())?;
            if first_undo.after
                != crate::skill_event_store::fingerprint_regular_bytes(&self.original)
            {
                return Err("Fork restore chain differs from original document backup".into());
            }
            let target = &history[0];
            let intent: ForkDocumentRestoreIntent = serde_json::from_value(target.payload.clone())
                .map_err(|error| error.to_string())?;
            let backup = crate::skill_repair_backup::VerifiedRepairBackup::read_document(
                &store.app_data,
                &target.id,
                &self.source.intent().repair().path.join("SKILL.md"),
                &intent.before,
                &self.lease,
            )?;
            self.original = backup.original().to_vec();
            self.target_backup = Some(backup);
        }
        self.history = history;
        Ok(())
    }

    pub fn event_id(&self) -> &str {
        &self.history[0].id
    }

    pub fn current_content(&self) -> &[u8] {
        &self.current
    }

    pub fn restore_content(&self) -> &[u8] {
        &self.original
    }

    pub fn record_intent(
        self,
        store: &EventStore,
        operation_id: &str,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PendingForkDocumentRestore<'scope>, crate::skill_event_operations::EventWriteFailure>
    {
        self.record_intent_after_backup(store, operation_id, limits, cancellation, || {})
    }

    pub(crate) fn record_intent_after_backup(
        self,
        store: &EventStore,
        operation_id: &str,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
        after_backup: impl FnOnce(),
    ) -> Result<PendingForkDocumentRestore<'scope>, crate::skill_event_operations::EventWriteFailure>
    {
        use crate::{
            skill_backup_source::BackupSourceRoot, skill_event_operations::EventWriteFailure,
            skill_event_store::fingerprint_regular_bytes,
        };
        let before_write = EventWriteFailure::BeforeWrite;
        let intent = ForkDocumentRestoreIntent {
            target_event: self.history[0].id.clone(),
            fork: self.source.intent().clone(),
            before: fingerprint_regular_bytes(&self.current),
            after: fingerprint_regular_bytes(&self.original),
            registry_admission: self.registry_admission.clone(),
        };
        intent.validate_record(operation_id).map_err(before_write)?;
        self.revalidate(store, limits, cancellation)
            .map_err(before_write)?;
        let directory = &self.source.intent().repair().path;
        let source = BackupSourceRoot::bind(directory)
            .and_then(|root| root.select(std::ffi::OsStr::new("SKILL.md")))
            .map_err(|error| before_write(error.to_string()))?;
        let state = BackupStateRoot::bind(&store.app_data)
            .map_err(|error| before_write(error.to_string()))?;
        let (manifest, backup) = self
            .lease
            .backup_documents_retained_prepared(
                &state,
                operation_id,
                vec![source],
                BackupCopyLimits {
                    max_bytes: self.current.len() as u64,
                    max_entries: 1,
                    max_depth: 0,
                },
            )
            .map_err(|error| before_write(error.to_string()))?;
        after_backup();
        if manifest
            .entries
            .get(
                directory
                    .join("SKILL.md")
                    .to_str()
                    .ok_or_else(|| before_write("Invalid fork document path".into()))?,
            )
            .is_none_or(|entry| entry.fingerprint != intent.before)
        {
            return Err(discard_unrecorded_restore_backup(
                backup,
                store,
                &self.lease,
                before_write("Fork restore backup differs from current document".into()),
            ));
        }
        if let Err(error) = self.revalidate(store, limits, cancellation) {
            return Err(discard_unrecorded_restore_backup(
                backup,
                store,
                &self.lease,
                before_write(error),
            ));
        }
        let overwritten = match crate::skill_repair_backup::VerifiedRepairBackup::read_document(
            &store.app_data,
            operation_id,
            &directory.join("SKILL.md"),
            &intent.before,
            &self.lease,
        ) {
            Ok(overwritten) => overwritten,
            Err(error) => {
                return Err(discard_unrecorded_restore_backup(
                    backup,
                    store,
                    &self.lease,
                    before_write(error),
                ));
            }
        };
        let record = GuardedEventStore::bind(store, &self.lease)
            .map_err(before_write)
            .and_then(|guarded| {
                guarded.record_fork_document_restore(
                    &self.lease,
                    &self.history[0],
                    &self.source,
                    &intent,
                    operation_id,
                )
            });
        let (source, event) = match record {
            Ok(record) => record,
            Err(crate::skill_event_operations::EventWriteFailure::BeforeWrite(error)) => {
                return Err(discard_unrecorded_restore_backup(
                    backup,
                    store,
                    &self.lease,
                    before_write(error),
                ));
            }
            Err(error) => return Err(error),
        };
        drop(backup);
        Ok(PendingForkDocumentRestore {
            prepared: self,
            source,
            event,
            overwritten,
        })
    }

    pub fn revalidate(
        &self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        if store.app_data != self.state_path {
            return Err("Fork document restore belongs to another event store".into());
        }
        let guarded = GuardedEventStore::bind(store, &self.lease)?;
        guarded.require_recovered(&self.lease)?;
        require_unchanged_history(store, &self.history)?;
        if let Some(backup) = &self.target_backup {
            backup.revalidate(&self.lease)?;
        }
        require_current_registry_admission(
            &self.lease,
            self.source
                .intent()
                .repair()
                .path
                .parent()
                .and_then(std::path::Path::parent)
                .ok_or("Fork restore has invalid ownership path")?,
            &self.source.intent().repair().name,
            self.source.intent().registry().record(),
            self.registry_admission.as_ref(),
        )?;
        guarded.verify_dotagents_fork_inputs(
            &self.lease,
            self.source.intent(),
            limits,
            cancellation,
        )?;
        require_unchanged_history(store, &self.history)?;
        if let Some(backup) = &self.target_backup {
            backup.revalidate(&self.lease)?;
        }
        self.lease.revalidate().map_err(|error| error.to_string())
    }
}

fn discard_unrecorded_restore_backup(
    backup: crate::skill_backup_reservation::ReservedBackup<'_>,
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    failure: crate::skill_event_operations::EventWriteFailure,
) -> crate::skill_event_operations::EventWriteFailure {
    let operation_id = backup.operation_id().to_owned();
    let cleanup = (|| {
        GuardedEventStore::bind(store, lease)?;
        if store.get(&operation_id)?.is_some() {
            return Err(
                "Fork restore event may have been recorded; retaining backup for recovery".into(),
            );
        }
        backup.discard().map_err(|error| error.to_string())
    })();
    match cleanup {
        Ok(()) => failure,
        Err(error) => crate::skill_event_operations::EventWriteFailure::BeforeWrite(format!(
            "{failure}; fork restore backup cleanup refused: {error}"
        )),
    }
}

fn require_unchanged_source(
    store: &EventStore,
    source: &CompletedDotagentsForkEvent,
) -> Result<(), String> {
    require_unchanged_history(store, std::slice::from_ref(source.event()))
}

fn require_unchanged_history(store: &EventStore, history: &[EventRow]) -> Result<(), String> {
    for expected in history {
        let current = store
            .get(&expected.id)?
            .ok_or("Fork restore source is missing")?;
        if serde_json::to_value(current).map_err(|error| error.to_string())?
            != serde_json::to_value(expected).map_err(|error| error.to_string())?
        {
            return Err("Fork restore source changed since selection".into());
        }
    }
    Ok(())
}

fn read_restore_chain(
    row: &EventRow,
    claim: Option<&str>,
    store: &EventStore,
) -> Result<(CompletedDotagentsForkEvent, Vec<EventRow>), String> {
    let mut current = row.clone();
    let mut expected_claim = claim.map(str::to_owned);
    let mut history = Vec::new();
    let mut seen = BTreeSet::new();
    let mut fork = None;
    let mut child_after = None;
    loop {
        if history.len() >= 1024 || !seen.insert(current.id.clone()) {
            return Err("Fork restore history is cyclic or exceeds its read limit".into());
        }
        if current.kind == "repair_dotagents_fork" {
            let origin = match expected_claim.as_deref() {
                Some(claim) => CompletedDotagentsForkEvent::from_claimed_row(&current, claim)?,
                None => CompletedDotagentsForkEvent::from_row(&current)?,
            };
            if fork
                .as_ref()
                .is_some_and(|payload| payload != &current.payload)
            {
                return Err("Fork restore history has inconsistent origin evidence".into());
            }
            history.push(current);
            return Ok((origin, history));
        }
        let intent: ForkDocumentRestoreIntent =
            serde_json::from_value(current.payload.clone()).map_err(|error| error.to_string())?;
        intent.validate_record(&current.id)?;
        let expected_inverse = serde_json::to_value(crate::skill_event::InverseOp::RestoreBackup {
            path: intent.fork.repair().path.join("SKILL.md"),
            pre_fingerprint: intent.before.clone(),
            post_fingerprint: Some(intent.after.clone()),
        })
        .map_err(|error| error.to_string())?;
        let origin = serde_json::to_value(&intent.fork).map_err(|error| error.to_string())?;
        if current.kind != "restore_fork_document"
            || current.status != "done"
            || !current.restorable
            || current.reverted_by != expected_claim
            || current.inverse.as_ref() != Some(&expected_inverse)
            || current.skill != intent.fork.repair().name
            || current.scope.as_deref() != Some("global")
            || current.harness.is_some()
            || current.project_path.is_some()
            || current.backup_dir.as_deref() != Some(format!("backups/{}", current.id).as_str())
            || serde_json::to_value(&intent).map_err(|error| error.to_string())? != current.payload
            || fork.as_ref().is_some_and(|payload| payload != &origin)
            || child_after
                .as_ref()
                .is_some_and(|hash| hash != &intent.before)
        {
            return Err("Invalid completed fork restore chain".into());
        }
        fork = Some(origin);
        child_after = Some(intent.after);
        expected_claim = Some(current.id.clone());
        history.push(current);
        current = store
            .get(&intent.target_event)?
            .ok_or("Fork restore predecessor is missing")?;
    }
}

impl ScopedSkillService {
    pub fn prepare_fork_document_restore(
        &mut self,
        row: &EventRow,
        store: &EventStore,
        force: bool,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedForkDocumentRestore<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let (source, history) = read_restore_chain(row, None, store).map_err(invalid)?;
        let mut prepared = self.prepare_fork_document_inputs(
            source,
            store,
            limits,
            timeout,
            cancellation.clone(),
            RegistryAdmissionMode::NewRestore,
        )?;
        prepared.select_history(history, store).map_err(invalid)?;
        let matches_source = if row.kind == "restore_fork_document" {
            let intent: ForkDocumentRestoreIntent = serde_json::from_value(row.payload.clone())
                .map_err(|error| invalid(error.to_string()))?;
            crate::skill_event_store::fingerprint_regular_bytes(&prepared.current) == intent.after
        } else {
            content_fingerprint(&prepared.current)
                == prepared
                    .source
                    .intent()
                    .repair()
                    .proposed_content_fingerprint
        };
        if !force && !matches_source {
            return Err(invalid("Fork document changed since its source operation; force must preserve current bytes".into()));
        }
        prepared
            .revalidate(store, limits, &cancellation)
            .map_err(invalid)?;
        Ok(prepared)
    }
    fn prepare_fork_document_inputs(
        &mut self,
        source: CompletedDotagentsForkEvent,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
        registry_mode: RegistryAdmissionMode<'_>,
    ) -> Result<PreparedForkDocumentRestore<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let repair = source.intent().repair();
        let agents = self.scope().home.join(".agents");
        if repair.path != agents.join("skills").join(&repair.name) {
            return Err(invalid(
                "Fork restore requires its canonical deployment in the selected home".into(),
            ));
        }
        let names = BTreeSet::from([repair.name.clone()]);
        let (inventory, lease) = self.prepare_write_inventory(
            Some(&names),
            std::slice::from_ref(&store.app_data),
            timeout,
            cancellation.clone(),
        )?;
        let deployment = exact_repair_deployment(&inventory, &repair.deployment_id)?;
        if deployment.owner_kind != LifecycleOwnerKind::Fork
            || deployment.plugin.is_some()
            || deployment.is_symlink
            || std::path::Path::new(&deployment.path) != repair.path
        {
            return Err(invalid(
                "Fork document restore ownership or location changed".into(),
            ));
        }
        let registry_admission = select_registry_admission(
            &lease,
            &agents,
            &repair.name,
            source.intent().registry().record(),
            registry_mode,
        )
        .map_err(invalid)?;
        let current = lease
            .read(&repair.path.join("SKILL.md"), MAX_REPAIR_DOCUMENT_BYTES)
            .map_err(|error| invalid(error.to_string()))?;
        let guarded = GuardedEventStore::bind(store, &lease).map_err(invalid)?;
        require_unchanged_source(store, &source).map_err(invalid)?;
        guarded
            .verify_dotagents_fork_inputs(&lease, source.intent(), limits, &cancellation)
            .map_err(invalid)?;
        let root =
            BackupStateRoot::bind(&store.app_data).map_err(|error| invalid(error.to_string()))?;
        let backup = root
            .open_existing(&source.event().id)
            .map_err(|error| invalid(error.to_string()))?;
        let original = backup
            .read_tree_record("live-tree", "SKILL.md", MAX_REPAIR_DOCUMENT_BYTES)
            .map_err(|error| invalid(error.to_string()))?;
        repair.validate_original(&original).map_err(invalid)?;
        let prepared = PreparedForkDocumentRestore {
            history: vec![source.event().clone()],
            target_backup: None,
            source,
            current,
            original,
            state_path: store.app_data.clone(),
            lease,
            registry_admission,
        };
        Ok(prepared)
    }

    pub fn prepare_fork_document_restore_recovery(
        &mut self,
        source: &EventRow,
        restore: &EventRow,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedForkDocumentRestoreRecovery<'_>, WritePreparationError> {
        use crate::skill_event_store::fingerprint_regular_bytes;
        let invalid = WritePreparationError::InvalidRepairSelection;
        let (origin, history) =
            read_restore_chain(source, Some(&restore.id), store).map_err(invalid)?;
        let intent: ForkDocumentRestoreIntent = serde_json::from_value(restore.payload.clone())
            .map_err(|error| invalid(error.to_string()))?;
        intent.validate_record(&restore.id).map_err(invalid)?;
        if !matches!(restore.status.as_str(), "pending" | "interrupted")
            || restore.kind != "restore_fork_document"
            || restore.reverted_by.is_some()
            || restore.restorable
            || restore.inverse.is_some()
            || restore.skill != source.skill
            || restore.scope != source.scope
            || restore.harness.is_some()
            || restore.project_path.is_some()
            || restore.backup_dir.as_deref() != Some(format!("backups/{}", restore.id).as_str())
            || intent.target_event != source.id
            || serde_json::to_value(&intent).map_err(|error| invalid(error.to_string()))?
                != restore.payload
            || serde_json::to_value(&intent.fork).map_err(|error| invalid(error.to_string()))?
                != origin.event().payload
        {
            return Err(invalid("Invalid unresolved fork document restore".into()));
        }
        let mut prepared = self.prepare_fork_document_inputs(
            origin,
            store,
            limits,
            timeout,
            cancellation.clone(),
            RegistryAdmissionMode::Recorded(intent.registry_admission.as_ref()),
        )?;
        prepared.select_history(history, store).map_err(invalid)?;
        let current = fingerprint_regular_bytes(&prepared.current);
        if fingerprint_regular_bytes(&prepared.original) != intent.after
            || (current != intent.before && current != intent.after)
        {
            return Err(invalid(
                "Fork restore recovery found unexpected document bytes".into(),
            ));
        }
        let overwritten = crate::skill_repair_backup::VerifiedRepairBackup::read_document(
            &store.app_data,
            &restore.id,
            &intent.fork.repair().path.join("SKILL.md"),
            &intent.before,
            &prepared.lease,
        )
        .map_err(invalid)?;
        let pending = PendingForkDocumentRestore {
            prepared,
            source: source.clone(),
            event: restore.clone(),
            overwritten,
        };
        pending
            .revalidate(store, limits, &cancellation)
            .map_err(invalid)?;
        Ok(PreparedForkDocumentRestoreRecovery { pending, limits })
    }
}

pub struct PreparedForkDocumentRestoreRecovery<'scope> {
    pending: PendingForkDocumentRestore<'scope>,
    limits: BackupCopyLimits,
}

impl PreparedForkDocumentRestoreRecovery<'_> {
    pub fn cancel_unapplied(
        self,
        store: &EventStore,
        cancellation: &CancellationToken,
    ) -> Result<
        crate::skill_repair_execution::RepairRecoveryOutcome,
        crate::skill_repair_execution::RepairExecutionError,
    > {
        use crate::skill_repair_execution::{
            RepairExecutionError, RepairExecutionStage, RepairRecoveryOutcome,
        };
        if self.pending.prepared.current == self.pending.prepared.original {
            return self.recover(store, cancellation);
        }
        let fail = |message| RepairExecutionError {
            event_id: self.pending.event.id.clone(),
            stage: RepairExecutionStage::Recover,
            message,
        };
        self.pending
            .revalidate(store, self.limits, cancellation)
            .map_err(fail)?;
        if self.pending.prepared.current != self.pending.overwritten.original() {
            return Err(fail(
                "Fork restore cancellation requires unchanged pre-restore bytes".into(),
            ));
        }
        GuardedEventStore::bind(store, &self.pending.prepared.lease)
            .map_err(fail)?
            .cancel_fork_document_restore(
                &self.pending.prepared.lease,
                &self.pending.source,
                &self.pending.event,
            )
            .map_err(|error| fail(error.to_string()))?;
        Ok(RepairRecoveryOutcome::NotApplied)
    }

    pub fn recover(
        self,
        store: &EventStore,
        cancellation: &CancellationToken,
    ) -> Result<
        crate::skill_repair_execution::RepairRecoveryOutcome,
        crate::skill_repair_execution::RepairExecutionError,
    > {
        self.pending.execute(store, self.limits, cancellation)?;
        Ok(crate::skill_repair_execution::RepairRecoveryOutcome::Applied)
    }
}

pub struct PreparedForkMergeBase<'scope> {
    inputs: PreparedForkDocumentRestore<'scope>,
}

impl PreparedForkMergeBase<'_> {
    pub fn publish(
        &self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PathBuf, String> {
        self.inputs.revalidate(store, limits, cancellation)?;
        let state = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
        state.publish_fork_base(
            &self.inputs.lease,
            &self.inputs.source.intent().repair().name,
            self.inputs.source.intent().snapshots(),
            limits,
            cancellation,
            || self.inputs.revalidate(store, limits, cancellation),
        )
    }

    pub fn event_id(&self) -> &str {
        &self.inputs.source.event().id
    }

    pub fn copy_to_staging(
        &self,
        store: &EventStore,
        staging: &BackupStateRoot,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<crate::skill_backup_reservation::BackupCopyReport, String> {
        self.inputs.revalidate(store, limits, cancellation)?;
        let state = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
        let backup = state
            .open_existing(self.event_id())
            .map_err(|error| error.to_string())?;
        let report = crate::skill_fork_snapshot::ForkSnapshotReceipt::copy_upstream_base(
            &backup,
            self.inputs.source.intent().snapshots(),
            staging,
            limits,
            cancellation,
        )?;
        self.inputs.revalidate(store, limits, cancellation)?;
        Ok(report)
    }
}

impl ScopedSkillService {
    pub fn prepare_fork_merge_base(
        &mut self,
        row: &EventRow,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedForkMergeBase<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        let source = match row.reverted_by.as_deref() {
            Some(claim) => CompletedDotagentsForkEvent::from_claimed_row(row, claim),
            None => CompletedDotagentsForkEvent::from_row(row),
        }
        .map_err(invalid)?;
        let inputs = self.prepare_fork_document_inputs(
            source,
            store,
            limits,
            timeout,
            cancellation.clone(),
            RegistryAdmissionMode::Strict,
        )?;
        inputs
            .revalidate(store, limits, &cancellation)
            .map_err(invalid)?;
        Ok(PreparedForkMergeBase { inputs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_fork_registry::{ForkRecord, OriginTool};

    fn record() -> ForkRecord {
        ForkRecord {
            deployment_id: "deployment".into(),
            skill_dir: "/tmp/.agents/skills/alpha".into(),
            forked_at: "2026-09-17T00:00:00Z".into(),
            origin_tool: OriginTool::Dotagents,
            origin_source: "owner/repo".into(),
            repo: "owner/repo".into(),
            path: "skills/alpha".into(),
            declared_ref: Some("main".into()),
            base_commit: "a".repeat(40),
        }
    }

    #[test]
    fn registry_admission_allows_only_a_valid_base_advance() {
        let original = record();
        let mut selected = serde_json::to_value(&original).unwrap();
        selected["base_commit"] = serde_json::json!("b".repeat(40));
        assert!(validate_admitted_registry_row(&selected, &original).is_ok());

        selected["origin_source"] = serde_json::json!("other/repo");
        assert!(validate_admitted_registry_row(&selected, &original).is_err());
    }

    #[test]
    fn registry_admission_refuses_unknown_or_invalid_selected_rows() {
        let original = record();
        let mut selected = serde_json::to_value(&original).unwrap();
        selected["base_commit"] = serde_json::json!("b".repeat(40));
        selected["unexpected"] = serde_json::json!(true);
        assert!(validate_admitted_registry_row(&selected, &original).is_err());

        selected.as_object_mut().unwrap().remove("unexpected");
        for invalid in ["not-a-commit".to_owned(), "B".repeat(40), "B".repeat(64)] {
            selected["base_commit"] = serde_json::json!(invalid);
            assert!(validate_admitted_registry_row(&selected, &original).is_err());
        }
    }
}
