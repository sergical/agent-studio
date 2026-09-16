use super::*;
use crate::{
    skill_backup_reservation::{valid_id, BackupCopyLimits, BackupStateRoot},
    skill_backup_source::BackupSourceRoot,
    skill_document_target::{CodexInvocationTarget, SkillDocumentTarget, SkillRegistryTarget},
    skill_event::{EventDraft, EventRow, EventStatus},
    skill_event_operations::GuardedEventStore,
    skill_event_store::{fingerprint_regular_bytes, EventStore},
    skill_ownership::LifecycleOwnerKind,
    skill_repair_backup::VerifiedCopyRepairBackup,
};
use std::ffi::OsStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentEditStage {
    Prepare,
    Backup,
    Intent,
    Document,
    Sidecar,
    Registry,
    Finish,
    Recover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentEditPublication {
    NotPublished,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentEditCause {
    Cancelled,
    Failure,
}

#[derive(Debug)]
pub struct DocumentEditError {
    pub event_id: String,
    pub cause: DocumentEditCause,
    pub stage: DocumentEditStage,
    pub publication: DocumentEditPublication,
    pub message: String,
}
impl std::fmt::Display for DocumentEditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Edit {} at {:?}: {}",
            self.event_id, self.stage, self.message
        )
    }
}
impl std::error::Error for DocumentEditError {}

#[derive(Debug)]
pub struct DocumentEditReceipt {
    pub event_id: String,
    pub deployment_id: String,
}

pub fn execute_copy_document_edit(
    prepared: PreparedCopyDocumentEdit<'_>,
    store: &EventStore,
    event_id: &str,
) -> Result<DocumentEditReceipt, DocumentEditError> {
    execute_with(prepared, store, event_id, |_| Ok(()))
}

fn execute_with(
    prepared: PreparedCopyDocumentEdit<'_>,
    store: &EventStore,
    event_id: &str,
    checkpoint: impl FnMut(DocumentEditStage) -> Result<(), String>,
) -> Result<DocumentEditReceipt, DocumentEditError> {
    execute_change(prepared, store, event_id, None, checkpoint)
}

fn execute_change(
    prepared: PreparedCopyDocumentEdit<'_>,
    store: &EventStore,
    event_id: &str,
    source: Option<CopyDocumentEditSource>,
    mut checkpoint: impl FnMut(DocumentEditStage) -> Result<(), String>,
) -> Result<DocumentEditReceipt, DocumentEditError> {
    let failure = |stage, publication, message: String| DocumentEditError {
        event_id: event_id.into(),
        cause: DocumentEditCause::Failure,
        stage,
        publication,
        message,
    };
    let before = |stage, message| failure(stage, DocumentEditPublication::NotPublished, message);
    let pending =
        |stage, message| failure(stage, DocumentEditPublication::RecoveryRequired, message);
    if !valid_id(event_id) {
        return Err(before(
            DocumentEditStage::Prepare,
            "Invalid edit event ID".into(),
        ));
    }
    let before_content = |stage, error: PreparedContentError| DocumentEditError {
        event_id: event_id.into(),
        cause: if error.is_cancelled() {
            DocumentEditCause::Cancelled
        } else {
            DocumentEditCause::Failure
        },
        stage,
        publication: DocumentEditPublication::NotPublished,
        message: error.to_string(),
    };
    prepared
        .revalidate_content()
        .map_err(|error| before_content(DocumentEditStage::Prepare, error))?;
    let events = GuardedEventStore::bind_prepared(store, &prepared.lease)
        .map_err(|error| before_content(DocumentEditStage::Prepare, error))?;
    events
        .require_recovered_prepared(&prepared.lease)
        .map_err(|error| before_content(DocumentEditStage::Prepare, error))?;
    if let Some(source) = &source {
        events
            .validate_copy_document_source(&prepared.lease, source)
            .map_err(|e| before(DocumentEditStage::Prepare, e))?;
    }
    let PreparedCopyDocumentEdit {
        intent,
        original,
        registry_original,
        sidecar,
        mut lease,
        ..
    } = prepared;
    let record = intent.transition().before();
    let document_path = record.path.join("SKILL.md");
    let registry_parent = intent
        .registry_path()
        .parent()
        .ok_or_else(|| before(DocumentEditStage::Prepare, "Missing registry parent".into()))?;
    let document = SkillDocumentTarget::bind(&record.path)
        .map_err(|e| before(DocumentEditStage::Prepare, e))?;
    let registry = SkillRegistryTarget::bind(registry_parent)
        .map_err(|e| before(DocumentEditStage::Prepare, e))?;
    let registry_after = intent
        .transition()
        .apply_document(&registry_original)
        .map_err(|e| before(DocumentEditStage::Prepare, e))?;
    let mut sources = Vec::new();
    for (parent, name) in [
        (&*record.path, "SKILL.md"),
        (registry_parent, "skill-studio.json"),
    ] {
        sources.push(
            BackupSourceRoot::bind(parent)
                .and_then(|root| root.select(OsStr::new(name)))
                .map_err(|e| before(DocumentEditStage::Backup, e.to_string()))?,
        );
    }
    let sidecar_path = record.path.join("agents/openai.yaml");
    let absent_sidecar = sidecar
        .as_ref()
        .and_then(|edit| edit.original().is_none().then_some(sidecar_path.as_path()));
    if sidecar
        .as_ref()
        .is_some_and(|edit| edit.original().is_some())
    {
        sources.push(
            BackupSourceRoot::bind(&record.path.join("agents"))
                .and_then(|root| root.select(OsStr::new("openai.yaml")))
                .map_err(|e| before(DocumentEditStage::Backup, e.to_string()))?,
        );
    }
    let state = BackupStateRoot::bind(&store.app_data)
        .map_err(|e| before(DocumentEditStage::Backup, e.to_string()))?;
    checkpoint(DocumentEditStage::Prepare)
        .map_err(|error| before(DocumentEditStage::Prepare, error))?;
    let manifest = lease
        .backup_documents_with_absent_sidecar_prepared(
            &state,
            event_id,
            sources,
            BackupCopyLimits {
                max_bytes: (original.len()
                    + registry_original.len()
                    + sidecar
                        .as_ref()
                        .and_then(|edit| edit.original())
                        .map_or(0, str::len)) as u64,
                max_entries: 2 + u64::from(sidecar.is_some()),
                max_depth: 0,
            },
            absent_sidecar,
        )
        .map_err(|error| before_content(DocumentEditStage::Backup, error))?;
    for (path, bytes) in [
        (&*document_path, original.as_slice()),
        (intent.registry_path(), registry_original.as_slice()),
    ] {
        if manifest
            .entries
            .get(path.to_string_lossy().as_ref())
            .is_none_or(|entry| entry.fingerprint != fingerprint_regular_bytes(bytes))
        {
            return Err(before(
                DocumentEditStage::Backup,
                "Copy edit backup changed".into(),
            ));
        }
    }
    checkpoint(DocumentEditStage::Backup).map_err(|e| before(DocumentEditStage::Backup, e))?;
    let selected = crate::skill_deployment::parse_deployment_id(&intent.request().deployment_id)
        .ok_or_else(|| before(DocumentEditStage::Intent, "Invalid edit deployment".into()))?;
    let draft = match &source {
        Some(source) => source
            .reversal_draft(event_id, &intent)
            .map_err(|e| before(DocumentEditStage::Intent, e))?,
        None => EventDraft {
            kind: "edit_copy_document".into(),
            skill: record.name.clone(),
            harness: None,
            scope: Some(selected.scope),
            project_path: selected.project_path,
            payload: serde_json::to_value(&intent)
                .map_err(|e| before(DocumentEditStage::Intent, e.to_string()))?,
            inverse: None,
            backup_dir: Some(format!("backups/{event_id}")),
            restorable: false,
        },
    };
    let expected_payload = draft.payload.clone();
    match &source {
        Some(source) => {
            drop(draft);
            events.record_copy_document_reversal(&lease, source, event_id, &intent)
        }
        None => events.record_pending(&lease, event_id, draft),
    }
    .map_err(|error| match error {
        crate::skill_event_operations::EventWriteFailure::CancelledBeforeWrite => {
            DocumentEditError {
                event_id: event_id.into(),
                cause: DocumentEditCause::Cancelled,
                stage: DocumentEditStage::Intent,
                publication: DocumentEditPublication::NotPublished,
                message: error.to_string(),
            }
        }
        other => pending(DocumentEditStage::Intent, other.to_string()),
    })?;
    let row = events
        .next_recovery_event(&lease)
        .map_err(|e| pending(DocumentEditStage::Intent, e))?
        .ok_or_else(|| pending(DocumentEditStage::Intent, "Recorded edit is missing".into()))?;
    if row.id != event_id || row.payload != expected_payload {
        return Err(pending(
            DocumentEditStage::Intent,
            "Recorded edit intent changed".into(),
        ));
    }
    let event = CopyDocumentEditRecoveryEvent::from_row(&row)
        .map_err(|e| pending(DocumentEditStage::Intent, e))?;
    checkpoint(DocumentEditStage::Intent).map_err(|e| pending(DocumentEditStage::Intent, e))?;
    events
        .validate_copy_document_edit_recovery(&lease, &event)
        .map_err(|e| pending(DocumentEditStage::Document, e))?;
    document
        .replace(
            &mut lease,
            &original,
            intent.request().proposed_content.as_bytes(),
        )
        .map_err(|e| pending(DocumentEditStage::Document, e.to_string()))?;
    checkpoint(DocumentEditStage::Document).map_err(|e| pending(DocumentEditStage::Document, e))?;
    if let Some(sidecar) = sidecar {
        match (sidecar.original(), sidecar.proposed()) {
            (None, Some(proposed)) => {
                CodexInvocationTarget::create(&record.path, &mut lease, proposed.as_bytes())
            }
            (Some(expected), Some(proposed)) => CodexInvocationTarget::bind(&record.path)
                .map_err(crate::skill_document_write::DocumentWriteFailure::BeforeReplace)
                .and_then(|target| {
                    target.replace(&mut lease, expected.as_bytes(), proposed.as_bytes())
                }),
            (Some(expected), None) => CodexInvocationTarget::bind(&record.path)
                .map_err(crate::skill_document_write::DocumentWriteFailure::BeforeReplace)
                .and_then(|target| target.remove(&mut lease, expected.as_bytes())),
            // The selected Codex reader still participates in the event, even
            // when its sidecar is absent at both endpoints. This preserves the
            // proof that no external sidecar appeared between planning and
            // ownership publication.
            (None, None) => Ok(()),
        }
        .map_err(|e| pending(DocumentEditStage::Sidecar, e.to_string()))?;
        checkpoint(DocumentEditStage::Sidecar)
            .map_err(|e| pending(DocumentEditStage::Sidecar, e))?;
    }
    events
        .validate_copy_document_edit_recovery(&lease, &event)
        .map_err(|e| pending(DocumentEditStage::Registry, e))?;
    registry
        .replace(&mut lease, &registry_original, &registry_after)
        .map_err(|e| pending(DocumentEditStage::Registry, e.to_string()))?;
    checkpoint(DocumentEditStage::Registry).map_err(|e| pending(DocumentEditStage::Registry, e))?;
    events
        .finish_copy_document_edit_recovery(&lease, &event, EventStatus::Done)
        .map_err(|e| pending(DocumentEditStage::Finish, e.to_string()))?;
    Ok(DocumentEditReceipt {
        event_id: event_id.into(),
        deployment_id: intent.request().deployment_id.clone(),
    })
}

pub struct CopyDocumentEditRecoveryEvent {
    snapshot: EventRow,
    intent: CopyDocumentEditIntent,
    source: Option<CopyDocumentEditSourceReference>,
}
impl CopyDocumentEditRecoveryEvent {
    pub fn from_row(row: &EventRow) -> Result<Self, String> {
        if !matches!(row.status.as_str(), "pending" | "interrupted") || row.reverted_by.is_some() {
            return Err("Event is not an eligible interrupted Copy document change".into());
        }
        let (intent, source) = decode_edit_event(row)?;
        Ok(Self {
            snapshot: row.clone(),
            intent,
            source,
        })
    }
    pub(crate) fn source(&self) -> Option<&CopyDocumentEditSourceReference> {
        self.source.as_ref()
    }
    pub(crate) fn intent(&self) -> &CopyDocumentEditIntent {
        &self.intent
    }
    pub(crate) fn snapshot(&self) -> &EventRow {
        &self.snapshot
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyDocumentEditObservedState {
    Original,
    DocumentApplied,
    SidecarApplied,
    Applied,
}

impl CopyDocumentEditIntent {
    pub fn classify_observed(
        &self,
        document: &[u8],
        registry: &[u8],
        sidecar: Option<Option<&[u8]>>,
        sidecar_original: Option<&[u8]>,
        backward_hash: &str,
        forward_hash: &str,
    ) -> Result<CopyDocumentEditObservedState, String> {
        self.validate_record()?;
        if document.len() > MAX_COPY_DOCUMENT_EDIT_BYTES || registry.len() > 8 * 1024 * 1024 {
            return Err("Copy edit observation exceeds its limit".into());
        }
        if backward_hash != self.transition.before().content_hash
            || forward_hash != self.transition.after().content_hash
        {
            return Err("Copy edit resources conflict with its intent".into());
        }
        let registry: crate::skill_fork_registry::ForkRegistry =
            serde_json::from_slice(registry).map_err(|e| e.to_string())?;
        let current = registry
            .copies
            .get(&self.request.deployment_id)
            .ok_or("Copy edit owner is missing")?;
        let document_before =
            content_fingerprint(document) == self.request.expected_content_fingerprint;
        let document_after = content_fingerprint(document) == self.proposed_content_fingerprint;
        let registry_before = current == self.transition.before();
        let registry_after = current == self.transition.after();
        let (sidecar_before, sidecar_after) = match (self.sidecar(), sidecar) {
            (None, None) => (true, true),
            (Some(intent), Some(current)) => (
                current == sidecar_original,
                current == intent.proposed_content().map(str::as_bytes),
            ),
            _ => return Err("Copy edit sidecar participant changed".into()),
        };
        if !document_before && !document_after || !registry_before && !registry_after {
            return Err("Copy edit ownership conflicts with its intent".into());
        }
        if self.sidecar().is_none() {
            return match (
                document_before,
                document_after,
                registry_before,
                registry_after,
            ) {
                (true, _, true, _) => Ok(CopyDocumentEditObservedState::Original),
                (_, true, true, _) => Ok(CopyDocumentEditObservedState::DocumentApplied),
                (_, true, _, true) => Ok(CopyDocumentEditObservedState::Applied),
                _ => Err("Copy edit publication order changed".into()),
            };
        }
        // Check older prefixes first because equal endpoints carry no
        // publication information (for example, an absent sidecar).
        match (
            document_before,
            document_after,
            sidecar_before,
            sidecar_after,
            registry_before,
            registry_after,
        ) {
            (true, _, true, _, true, _) => Ok(CopyDocumentEditObservedState::Original),
            (_, true, _, true, _, true) => Ok(CopyDocumentEditObservedState::Applied),
            (_, true, _, true, true, _) => Ok(CopyDocumentEditObservedState::SidecarApplied),
            (_, true, true, _, true, _) => Ok(CopyDocumentEditObservedState::DocumentApplied),
            _ => Err("Copy edit publication order changed".into()),
        }
    }
}

pub struct PreparedCopyDocumentEditRecovery<'scope> {
    event: CopyDocumentEditRecoveryEvent,
    backup: VerifiedCopyRepairBackup,
    document: Vec<u8>,
    registry: Vec<u8>,
    sidecar: Option<Option<Vec<u8>>>,
    content: PreparedCopyRepairContent,
    content_scope: SkillReadScope,
    state: CopyDocumentEditObservedState,
    lease: FinalizedWriteLease<'scope>,
}
impl PreparedCopyDocumentEditRecovery<'_> {
    fn observe(&self) -> Result<CopyDocumentEditObservedState, String> {
        self.backup.revalidate(&self.lease)?;
        let (original_document, _, original_sidecar) = self.backup.edit_originals();
        let current_sidecar = self
            .sidecar
            .as_ref()
            .map(|current| {
                current
                    .as_ref()
                    .map(|bytes| String::from_utf8(bytes.clone()))
                    .transpose()
                    .map_err(|_| "Copy sidecar is not UTF-8")
            })
            .transpose()?;
        let original_sidecar_text = original_sidecar
            .map(std::str::from_utf8)
            .transpose()
            .map_err(|_| "Copy sidecar backup is not UTF-8")?
            .map(str::to_owned);
        let backward_sidecar = self.event.intent.sidecar().map(|_| {
            crate::skill_invocation_edit::CodexInvocationEdit::from_endpoints(
                current_sidecar.clone().flatten(),
                original_sidecar_text.clone(),
            )
        });
        let forward_sidecar = self.event.intent.sidecar().map(|intent| {
            crate::skill_invocation_edit::CodexInvocationEdit::from_endpoints(
                current_sidecar.clone().flatten(),
                intent.proposed_content().map(str::to_owned),
            )
        });
        let (_, backward_hash) = self
            .content
            .hashes_with_document_and_sidecar_limit(
                &self.content_scope,
                &self.lease,
                (&self.document, original_document),
                backward_sidecar.as_ref(),
                MAX_COPY_DOCUMENT_EDIT_BYTES,
            )
            .map_err(|error| error.to_string())?;
        let (_, forward_hash) = self
            .content
            .hashes_with_document_and_sidecar_limit(
                &self.content_scope,
                &self.lease,
                (
                    &self.document,
                    self.event.intent.request().proposed_content.as_bytes(),
                ),
                forward_sidecar.as_ref(),
                MAX_COPY_DOCUMENT_EDIT_BYTES,
            )
            .map_err(|error| error.to_string())?;
        let state = self.event.intent.classify_observed(
            &self.document,
            &self.registry,
            self.sidecar.as_ref().map(|value| value.as_deref()),
            original_sidecar,
            &backward_hash,
            &forward_hash,
        )?;
        self.lease.revalidate().map_err(|e| e.to_string())?;
        Ok(state)
    }
    pub fn state(&self) -> CopyDocumentEditObservedState {
        self.state
    }
}

impl ScopedSkillService {
    pub fn prepare_copy_document_edit_recovery(
        &mut self,
        row: &EventRow,
        store: &EventStore,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedCopyDocumentEditRecovery<'_>, DocumentEditPreparationError> {
        let invalid = DocumentEditPreparationError::InvalidEdit;
        let event = CopyDocumentEditRecoveryEvent::from_row(row).map_err(invalid)?;
        let record = event.intent.transition().before();
        let names = BTreeSet::from([record.name.clone()]);
        let (inventory, mut lease) = self
            .prepare_write_inventory(
                Some(&names),
                std::slice::from_ref(&store.app_data),
                timeout,
                cancellation.clone(),
            )
            .map_err(DocumentEditPreparationError::Inventory)?;
        if event.intent.registry_path() != inventory.scope.home.join(".agents/skill-studio.json") {
            return Err(invalid(
                "Copy edit registry is outside the selected home".into(),
            ));
        }
        let mut matches = inventory
            .skills
            .iter()
            .flat_map(|s| &s.deployments)
            .filter(|d| d.id == record.deployment_id);
        let deployment = matches
            .next()
            .ok_or_else(|| invalid("Copy edit recovery deployment is absent".into()))?;
        if matches.next().is_some()
            || std::path::Path::new(&deployment.path) != record.path
            || deployment.is_symlink
            || deployment.plugin.is_some()
            || deployment.disabled != event.intent.transition().is_disabled()
            || !matches!(
                deployment.owner_kind,
                LifecycleOwnerKind::Copy
                    | LifecycleOwnerKind::Unknown
                    | LifecycleOwnerKind::Manual
                    | LifecycleOwnerKind::InRepo
            )
        {
            return Err(invalid("Copy edit recovery target changed".into()));
        }
        let backup =
            VerifiedCopyRepairBackup::read_edit(&store.app_data, &row.id, &event.intent, &lease)
                .map_err(invalid)?;
        let content_scope = SkillReadScope::bind(std::slice::from_ref(&record.path))
            .map_err(|e| invalid(e.to_string()))?;
        let sidecar_path = record.path.join("agents/openai.yaml");
        let sidecar = if event.intent.sidecar().is_some() {
            let current = match content_scope.observe_entry(&record.path, OsStr::new("agents")) {
                Err(crate::skill_scope::ScopedReadError::Missing { .. }) => None,
                Ok(entry)
                    if entry.metadata.is_dir() && !entry.metadata.file_type().is_symlink() =>
                {
                    match content_scope
                        .observe_entry(&record.path.join("agents"), OsStr::new("openai.yaml"))
                    {
                        Err(crate::skill_scope::ScopedReadError::Missing { .. }) => None,
                        Ok(entry)
                            if entry.metadata.is_file()
                                && !entry.metadata.file_type().is_symlink() =>
                        {
                            Some(
                                lease
                                    .read(&sidecar_path, MAX_COPY_DOCUMENT_EDIT_BYTES)
                                    .map_err(|e| invalid(e.to_string()))?,
                            )
                        }
                        _ => {
                            return Err(invalid(
                                "Copy sidecar must be a regular scoped file".into(),
                            ))
                        }
                    }
                }
                _ => {
                    return Err(invalid(
                        "Copy sidecar parent must be a regular scoped directory".into(),
                    ))
                }
            };
            if current.is_none() {
                lease = lease
                    .retain_absent_invocation_sidecar(&sidecar_path)
                    .map_err(invalid)?;
            }
            Some(current)
        } else {
            None
        };
        let content = PreparedCopyRepairContent::enumerate_cancellable(
            &content_scope,
            &record.path,
            &cancellation,
        )
        .map_err(invalid)?;
        let document = lease
            .read(&record.path.join("SKILL.md"), MAX_COPY_DOCUMENT_EDIT_BYTES)
            .map_err(|e| invalid(e.to_string()))?;
        let registry = lease
            .read(event.intent.registry_path(), 8 * 1024 * 1024)
            .map_err(|e| invalid(e.to_string()))?;
        let mut prepared = PreparedCopyDocumentEditRecovery {
            event,
            backup,
            document,
            registry,
            sidecar,
            content,
            content_scope,
            state: CopyDocumentEditObservedState::Original,
            lease,
        };
        prepared.state = prepared.observe().map_err(invalid)?;
        GuardedEventStore::bind(store, &prepared.lease)
            .and_then(|events| {
                events.validate_copy_document_edit_recovery(&prepared.lease, &prepared.event)
            })
            .map_err(invalid)?;
        Ok(prepared)
    }
}

pub fn recover_copy_document_edit(
    mut prepared: PreparedCopyDocumentEditRecovery<'_>,
    store: &EventStore,
) -> Result<CopyDocumentEditObservedState, DocumentEditError> {
    let error = |message: String| DocumentEditError {
        event_id: prepared.event.snapshot.id.clone(),
        cause: DocumentEditCause::Failure,
        stage: DocumentEditStage::Recover,
        publication: DocumentEditPublication::RecoveryRequired,
        message,
    };
    let state = prepared.observe().map_err(&error)?;
    if state != prepared.state {
        return Err(error("Copy edit recovery state changed".into()));
    }
    let events = GuardedEventStore::bind(store, &prepared.lease).map_err(&error)?;
    events
        .validate_copy_document_edit_recovery(&prepared.lease, &prepared.event)
        .map_err(&error)?;
    if state == CopyDocumentEditObservedState::DocumentApplied {
        if let Some(sidecar) = prepared.event.intent.sidecar() {
            let skill_path = &prepared.event.intent.transition().before().path;
            match (
                prepared
                    .sidecar
                    .as_ref()
                    .and_then(|current| current.as_deref()),
                sidecar.proposed_content(),
            ) {
                (None, Some(proposed)) => CodexInvocationTarget::create(
                    skill_path,
                    &mut prepared.lease,
                    proposed.as_bytes(),
                ),
                (Some(expected), Some(proposed)) => CodexInvocationTarget::bind(skill_path)
                    .map_err(crate::skill_document_write::DocumentWriteFailure::BeforeReplace)
                    .and_then(|target| {
                        target.replace(&mut prepared.lease, expected, proposed.as_bytes())
                    }),
                (Some(expected), None) => CodexInvocationTarget::bind(skill_path)
                    .map_err(crate::skill_document_write::DocumentWriteFailure::BeforeReplace)
                    .and_then(|target| target.remove(&mut prepared.lease, expected)),
                (None, None) => Ok(()),
            }
            .map_err(|e| error(e.to_string()))?;
            prepared.sidecar = Some(
                sidecar
                    .proposed_content()
                    .map(|content| content.as_bytes().to_vec()),
            );
            prepared
                .lease
                .validate_invocation_output(
                    &skill_path.join("agents/openai.yaml"),
                    sidecar.proposed_content().map(str::as_bytes),
                )
                .map_err(&error)?;
        }
    }
    if matches!(
        state,
        CopyDocumentEditObservedState::DocumentApplied
            | CopyDocumentEditObservedState::SidecarApplied
    ) {
        let intent = &prepared.event.intent;
        let parent = intent
            .registry_path()
            .parent()
            .ok_or_else(|| error("Missing registry parent".into()))?;
        let target = SkillRegistryTarget::bind(parent).map_err(&error)?;
        let after = intent
            .transition()
            .apply_document(&prepared.registry)
            .map_err(&error)?;
        target
            .replace(&mut prepared.lease, &prepared.registry, &after)
            .map_err(|e| error(e.to_string()))?;
        // The lease validates the replacement receipt; the original read binding is stale.
        prepared.registry = after;
    }
    let expected = if state == CopyDocumentEditObservedState::Original {
        CopyDocumentEditObservedState::Original
    } else {
        CopyDocumentEditObservedState::Applied
    };
    let status = if expected == CopyDocumentEditObservedState::Original {
        EventStatus::Failed
    } else {
        EventStatus::Done
    };
    prepared
        .lease
        .revalidate()
        .map_err(|failure| error(failure.to_string()))?;
    events
        .finish_copy_document_edit_recovery(&prepared.lease, &prepared.event, status)
        .map_err(|e| error(e.to_string()))?;
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use super::super::preparation_tests::Fixture;
    use super::*;
    use std::fs;

    const ID: &str = "01M2C600000000000000000001";

    fn prepare<'a>(
        fixture: &Fixture,
        service: &'a mut ScopedSkillService,
        store: &EventStore,
    ) -> PreparedCopyDocumentEdit<'a> {
        let CopyDocumentEditPreparation::Ready(prepared) = service
            .prepare_copy_document_edit(
                &fixture.request,
                std::slice::from_ref(&store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap()
        else {
            panic!("expected changed edit")
        };
        *prepared
    }

    fn interrupted(
        fixture: &Fixture,
        service: &mut ScopedSkillService,
        store: &EventStore,
        stage: DocumentEditStage,
    ) -> DocumentEditError {
        execute_with(prepare(fixture, service, store), store, ID, |current| {
            if current == stage {
                Err("injected interruption".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err()
    }

    #[test]
    fn copy_invocation_publishes_an_absent_codex_sidecar_with_the_document_and_registry() {
        use crate::skill_document::InvocationPolicy;

        let fixture = Fixture::new(false, true, false);
        let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let CopyDocumentEditPreparation::Ready(prepared) = service
            .prepare_copy_invocation(
                &fixture.request.deployment_id,
                InvocationPolicy::UserOnly,
                std::slice::from_ref(&store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap()
        else {
            panic!("expected invocation edit")
        };
        execute_copy_document_edit(*prepared, &store, ID).unwrap();
        assert!(fs::read_to_string(fixture.skill.join("SKILL.md"))
            .unwrap()
            .contains("disable-model-invocation: true"));
        assert!(fs::read_to_string(fixture.skill.join("agents/openai.yaml"))
            .unwrap()
            .contains("allow_implicit_invocation: false"));
        assert_eq!(store.get(ID).unwrap().unwrap().status, "done");
    }

    #[test]
    fn copy_invocation_recovers_after_document_and_sidecar_publication() {
        use crate::skill_document::InvocationPolicy;

        for stop_at in [DocumentEditStage::Document, DocumentEditStage::Sidecar] {
            let fixture = Fixture::new(false, true, false);
            let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            let CopyDocumentEditPreparation::Ready(prepared) = service
                .prepare_copy_invocation(
                    &fixture.request.deployment_id,
                    InvocationPolicy::UserOnly,
                    std::slice::from_ref(&store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap()
            else {
                panic!("expected invocation edit")
            };
            let error = execute_with(*prepared, &store, ID, |stage| {
                (stage != stop_at)
                    .then_some(())
                    .ok_or_else(|| "interrupted".into())
            })
            .unwrap_err();
            assert_eq!(error.stage, stop_at);
            let row = store.get(ID).unwrap().unwrap();
            let prepared = service
                .prepare_copy_document_edit_recovery(
                    &row,
                    &store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            assert_eq!(
                prepared.state(),
                if stop_at == DocumentEditStage::Document {
                    CopyDocumentEditObservedState::DocumentApplied
                } else {
                    CopyDocumentEditObservedState::SidecarApplied
                }
            );
            assert_eq!(
                recover_copy_document_edit(prepared, &store).unwrap(),
                CopyDocumentEditObservedState::Applied
            );
            assert_eq!(store.get(ID).unwrap().unwrap().status, "done");
        }
    }

    #[test]
    fn copy_edit_cancellation_before_intent_preserves_its_typed_cause() {
        for stage in [DocumentEditStage::Prepare, DocumentEditStage::Backup] {
            for cancel in [true, false] {
                let fixture = Fixture::new(false, false, false);
                let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
                let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                let token = CancellationToken::default();
                let original = fs::read(fixture.skill.join("SKILL.md")).unwrap();
                let registry = fs::read(&fixture.registry).unwrap();
                let CopyDocumentEditPreparation::Ready(prepared) = service
                    .prepare_copy_document_edit(
                        &fixture.request,
                        std::slice::from_ref(&store.app_data),
                        Some(Duration::from_secs(10)),
                        token.clone(),
                    )
                    .unwrap()
                else {
                    panic!("expected changed edit")
                };
                let error = execute_with(*prepared, &store, ID, |current| {
                    if current == stage {
                        if cancel {
                            token.cancel();
                        } else {
                            return Err("message mentions cancelled without cancellation".into());
                        }
                    }
                    Ok(())
                })
                .unwrap_err();
                assert_eq!(
                    error.cause,
                    if cancel {
                        DocumentEditCause::Cancelled
                    } else {
                        DocumentEditCause::Failure
                    }
                );
                assert_eq!(error.publication, DocumentEditPublication::NotPublished);
                assert!(store.get(ID).unwrap().is_none());
                assert_eq!(fs::read(fixture.skill.join("SKILL.md")).unwrap(), original);
                assert_eq!(fs::read(&fixture.registry).unwrap(), registry);
            }
        }
    }

    #[test]
    fn copy_edit_execution_publishes_document_owner_and_history() {
        for project in [false, true] {
            for per_harness in [false, true] {
                let mut fixture = Fixture::new(project, per_harness, project && per_harness);
                fixture.request.proposed_content = fixture
                    .request
                    .proposed_content
                    .replace("name: sample", "name: different");
                let mut value: serde_json::Value =
                    serde_json::from_slice(&fs::read(&fixture.registry).unwrap()).unwrap();
                value["future_setting"] = serde_json::json!({"keep": true});
                value["copies"][&fixture.request.deployment_id]["future_record"] =
                    serde_json::json!([1, 2]);
                fs::write(&fixture.registry, serde_json::to_vec(&value).unwrap()).unwrap();
                let sibling = fs::read(&fixture.sibling).unwrap();
                let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
                let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                let receipt =
                    execute_copy_document_edit(prepare(&fixture, &mut service, &store), &store, ID)
                        .unwrap();
                assert_eq!(receipt.event_id, ID);
                assert_eq!(receipt.deployment_id, fixture.request.deployment_id);
                let row = store.get(ID).unwrap().unwrap();
                assert_eq!(row.status, "done");
                assert_eq!(row.kind, "edit_copy_document");
                assert!(!row.restorable);
                assert_eq!(
                    fs::read_to_string(fixture.skill.join("SKILL.md")).unwrap(),
                    fixture.request.proposed_content
                );
                let current: serde_json::Value =
                    serde_json::from_slice(&fs::read(&fixture.registry).unwrap()).unwrap();
                assert_eq!(current["future_setting"], value["future_setting"]);
                assert_eq!(
                    current["copies"][&fixture.request.deployment_id]["future_record"],
                    serde_json::json!([1, 2])
                );
                let inventory = service.scan(None, Some(Duration::from_secs(10))).unwrap();
                let deployment = inventory
                    .skills
                    .iter()
                    .flat_map(|s| &s.deployments)
                    .find(|d| d.id == fixture.request.deployment_id)
                    .unwrap();
                assert_eq!(deployment.owner_kind, LifecycleOwnerKind::Copy);
                assert_eq!(
                    deployment.content_hash,
                    current["copies"][&fixture.request.deployment_id]["content_hash"]
                        .as_str()
                        .unwrap()
                );
                assert_eq!(fs::read(&fixture.sibling).unwrap(), sibling);
            }
        }
    }

    #[test]
    fn copy_edit_round_trip_preserves_global_and_project_reader_links() {
        for project in [false, true] {
            let fixture = Fixture::new(project, false, false);
            let root = if project {
                &fixture.scope.projects[0]
            } else {
                &fixture.scope.home
            };
            let links = [
                root.join(".claude/skills/sample"),
                root.join(".codex/skills/sample"),
            ];
            let target = PathBuf::from("../../.agents/skills/sample");
            for link in &links {
                fs::create_dir_all(link.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(&target, link).unwrap();
            }
            let original = fixture.bytes();
            let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            execute_copy_document_edit(prepare(&fixture, &mut service, &store), &store, ID)
                .unwrap();
            let applied = fixture.bytes();
            assert_ne!(applied.0, original.0);
            assert_ne!(applied.1, original.1);
            let mut source_id = ID;
            for (id, expected) in [
                (ID, &applied),
                ("undo-linked-copy", &original),
                ("redo-linked-copy", &applied),
            ] {
                if id != ID {
                    let source = store.get(source_id).unwrap().unwrap();
                    let prepared = service
                        .prepare_copy_document_reversal(
                            &source,
                            &store,
                            Some(Duration::from_secs(10)),
                            CancellationToken::default(),
                        )
                        .unwrap();
                    execute_copy_document_reversal(prepared, &store, id).unwrap();
                    assert_eq!(
                        store
                            .get(source_id)
                            .unwrap()
                            .unwrap()
                            .reverted_by
                            .as_deref(),
                        Some(id)
                    );
                }
                assert_eq!(store.get(id).unwrap().unwrap().status, "done");
                let current = fixture.bytes();
                assert_eq!(current.0, expected.0);
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&current.1).unwrap(),
                    serde_json::from_slice::<serde_json::Value>(&expected.1).unwrap()
                );
                assert_eq!(current.2, original.2);
                for link in &links {
                    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
                    assert_eq!(fs::read_link(link).unwrap(), target);
                    assert_eq!(fs::read(link.join("SKILL.md")).unwrap(), expected.0);
                }
                source_id = id;
            }
        }
    }

    #[test]
    fn copy_edit_refuses_reader_retarget_after_document_publication() {
        for project in [false, true] {
            let fixture = Fixture::new(project, false, false);
            let root = if project {
                &fixture.scope.projects[0]
            } else {
                &fixture.scope.home
            };
            let link = root.join(".claude/skills/sample");
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&fixture.skill, &link).unwrap();
            let original = fixture.bytes();
            let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            let result = execute_with(
                prepare(&fixture, &mut service, &store),
                &store,
                ID,
                |stage| {
                    if stage == DocumentEditStage::Document {
                        fs::remove_file(&link).unwrap();
                        std::os::unix::fs::symlink(fixture.sibling.parent().unwrap(), &link)
                            .unwrap();
                    }
                    Ok(())
                },
            )
            .unwrap_err();
            assert_eq!(result.stage, DocumentEditStage::Registry);
            assert_eq!(
                result.publication,
                DocumentEditPublication::RecoveryRequired
            );
            assert_eq!(store.get(ID).unwrap().unwrap().status, "pending");
            let current = fixture.bytes();
            assert_eq!(current.0, fixture.request.proposed_content.as_bytes());
            assert_eq!(current.1, original.1);
            assert_eq!(current.2, original.2);
            assert_eq!(fs::read(link.join("SKILL.md")).unwrap(), original.2);
        }
    }

    #[derive(Serialize, Deserialize)]
    struct KillFixtureInput {
        scope: crate::skill_service::SkillScope,
        request: CopyDocumentEditRequest,
        ready: PathBuf,
        stage: String,
        conflict: bool,
    }

    #[test]
    #[ignore = "subprocess helper for actual Copy publication interruption"]
    fn copy_edit_kill_process_helper() {
        let input: KillFixtureInput = serde_json::from_slice(
            &fs::read(std::env::var_os("SKILL_STUDIO_COPY_KILL_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let store = EventStore::open(&input.scope.home.join("state")).unwrap();
        let mut service = ScopedSkillService::bind(input.scope).unwrap();
        if std::env::var("SKILL_STUDIO_COPY_KILL_MODE").unwrap() == "write" {
            let CopyDocumentEditPreparation::Ready(prepared) = service
                .prepare_copy_document_edit(
                    &input.request,
                    std::slice::from_ref(&store.app_data),
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap()
            else {
                panic!("expected edit");
            };
            execute_with(*prepared, &store, ID, |stage| {
                if format!("{stage:?}") == input.stage {
                    fs::write(&input.ready, b"published").unwrap();
                    loop {
                        std::thread::park_timeout(Duration::from_secs(1));
                    }
                }
                Ok(())
            })
            .unwrap();
            panic!("publication checkpoint was not reached");
        }
        assert_eq!(store.reconcile_at_startup().unwrap().len(), 1);
        let row = store.get(ID).unwrap().unwrap();
        let recovery = service.prepare_copy_document_edit_recovery(
            &row,
            &store,
            Some(Duration::from_secs(10)),
            CancellationToken::default(),
        );
        if input.conflict {
            assert!(
                recovery.is_err(),
                "external edit must prevent recovery publication"
            );
            assert_ne!(store.get(ID).unwrap().unwrap().status, "done");
        } else {
            let recovery = recovery.unwrap();
            assert_eq!(
                recovery.state(),
                if input.stage == "Document" {
                    CopyDocumentEditObservedState::DocumentApplied
                } else {
                    CopyDocumentEditObservedState::Applied
                }
            );
            assert_eq!(
                recover_copy_document_edit(recovery, &store).unwrap(),
                CopyDocumentEditObservedState::Applied
            );
            assert_eq!(store.get(ID).unwrap().unwrap().status, "done");
            assert!(store.reconcile_at_startup().unwrap().is_empty());
        }
    }

    #[test]
    fn copy_edit_survives_sigkill_after_live_publication_and_refuses_conflicts() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::{Child, Command, Stdio};
        use std::time::Instant;
        struct ChildGuard(Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        const HELPER: &str =
            "skill_copy_document_edit::execution::tests::copy_edit_kill_process_helper";
        for project in [false, true] {
            for per_harness in [false, true] {
                for (stage, conflict) in
                    [("Document", false), ("Registry", false), ("Document", true)]
                {
                    let fixture = Fixture::new(project, per_harness, false);
                    let original = fixture.bytes();
                    let control = tempfile::tempdir().unwrap();
                    let input_path = control.path().join("input.json");
                    let ready = control.path().join("ready");
                    fs::write(
                        &input_path,
                        serde_json::to_vec(&KillFixtureInput {
                            scope: fixture.scope.clone(),
                            request: fixture.request.clone(),
                            ready: ready.clone(),
                            stage: stage.into(),
                            conflict,
                        })
                        .unwrap(),
                    )
                    .unwrap();
                    let command = |mode| {
                        let mut cmd = Command::new(std::env::current_exe().unwrap());
                        cmd.args(["--exact", HELPER, "--ignored", "--nocapture"])
                            .env("SKILL_STUDIO_COPY_KILL_INPUT", &input_path)
                            .env("SKILL_STUDIO_COPY_KILL_MODE", mode)
                            .stdin(Stdio::null());
                        cmd
                    };
                    let writer_log = control.path().join("writer.log");
                    let mut writer = ChildGuard(
                        command("write")
                            .stdout(fs::File::create(&writer_log).unwrap())
                            .stderr(Stdio::null())
                            .spawn()
                            .unwrap(),
                    );
                    let deadline = Instant::now() + Duration::from_secs(20);
                    while !ready.exists() {
                        assert!(
                            writer.0.try_wait().unwrap().is_none(),
                            "writer exited before checkpoint: {}",
                            fs::read_to_string(&writer_log).unwrap()
                        );
                        assert!(Instant::now() < deadline, "writer checkpoint timeout");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    assert_eq!(
                        fs::read(fixture.skill.join("SKILL.md")).unwrap(),
                        fixture.request.proposed_content.as_bytes()
                    );
                    if stage == "Document" {
                        assert_eq!(fs::read(&fixture.registry).unwrap(), original.1);
                    }
                    writer.0.kill().unwrap();
                    assert_eq!(writer.0.wait().unwrap().signal(), Some(libc::SIGKILL));
                    drop(writer);
                    {
                        let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
                        assert_eq!(store.get(ID).unwrap().unwrap().status, "pending");
                    }
                    if conflict {
                        fs::write(
                            fixture.skill.join("SKILL.md"),
                            b"external change after kill",
                        )
                        .unwrap();
                    }
                    let before_recovery = fixture.bytes();
                    let output = command("recover").output().unwrap();
                    assert!(
                        output.status.success(),
                        "{} {}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );
                    let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
                    if conflict {
                        assert_eq!(fixture.bytes(), before_recovery);
                        assert_ne!(store.get(ID).unwrap().unwrap().status, "done");
                    } else {
                        let row = store.get(ID).unwrap().unwrap();
                        assert_eq!(row.kind, "edit_copy_document");
                        assert_eq!(row.status, "done");
                        assert_eq!(
                            fs::read(fixture.skill.join("SKILL.md")).unwrap(),
                            fixture.request.proposed_content.as_bytes()
                        );
                        let registry: serde_json::Value =
                            serde_json::from_slice(&fs::read(&fixture.registry).unwrap()).unwrap();
                        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                        let inventory = service.scan(None, Some(Duration::from_secs(10))).unwrap();
                        let deployment = inventory
                            .skills
                            .iter()
                            .flat_map(|skill| &skill.deployments)
                            .find(|deployment| deployment.id == fixture.request.deployment_id)
                            .unwrap();
                        assert_eq!(deployment.owner_kind, LifecycleOwnerKind::Copy);
                        assert_eq!(
                            deployment.content_hash,
                            registry["copies"][&fixture.request.deployment_id]["content_hash"]
                                .as_str()
                                .unwrap()
                        );
                        drop(inventory);
                        assert!(store.reconcile_at_startup().unwrap().is_empty());
                        let applied = fixture.bytes();
                        let mut source_id = ID;
                        for (id, kind, expected) in [
                            ("undo-after-kill", "undo_copy_document", &original),
                            ("redo-after-kill", "redo_copy_document", &applied),
                        ] {
                            let source = store.get(source_id).unwrap().unwrap();
                            let prepared = service
                                .prepare_copy_document_reversal(
                                    &source,
                                    &store,
                                    Some(Duration::from_secs(10)),
                                    CancellationToken::default(),
                                )
                                .unwrap();
                            execute_copy_document_reversal(prepared, &store, id).unwrap();
                            let reversed = store.get(id).unwrap().unwrap();
                            assert_eq!(reversed.kind, kind);
                            assert_eq!(reversed.status, "done");
                            assert_eq!(
                                store
                                    .get(source_id)
                                    .unwrap()
                                    .unwrap()
                                    .reverted_by
                                    .as_deref(),
                                Some(id)
                            );
                            let current = fixture.bytes();
                            assert_eq!(current.0, expected.0);
                            assert_eq!(
                                serde_json::from_slice::<serde_json::Value>(&current.1).unwrap(),
                                serde_json::from_slice::<serde_json::Value>(&expected.1).unwrap()
                            );
                            assert_eq!(current.2, original.2);
                            source_id = id;
                        }
                        assert!(store.reconcile_at_startup().unwrap().is_empty());
                    }
                    assert_eq!(fs::read(&fixture.sibling).unwrap(), original.2);
                    println!(
                        "COPY_KILL_CASE {}",
                        serde_json::json!({"project": project, "per_harness": per_harness, "stage": stage, "external_conflict": conflict, "signal": "SIGKILL", "fresh_process_recovery": true, "undo_redo_verified": !conflict, "status": "ok"})
                    );
                }
            }
        }
    }

    #[test]
    fn copy_edit_execution_recovers_each_publication_boundary() {
        for stage in [
            DocumentEditStage::Backup,
            DocumentEditStage::Intent,
            DocumentEditStage::Document,
            DocumentEditStage::Registry,
        ] {
            let fixture = Fixture::new(true, true, false);
            let before = fixture.bytes();
            let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            let error = interrupted(&fixture, &mut service, &store, stage);
            assert_eq!(error.stage, stage);
            if stage == DocumentEditStage::Backup {
                assert_eq!(error.publication, DocumentEditPublication::NotPublished);
                assert!(store.get(ID).unwrap().is_none());
                assert_eq!(fixture.bytes(), before);
                continue;
            }
            assert_eq!(error.publication, DocumentEditPublication::RecoveryRequired);
            assert_eq!(store.get(ID).unwrap().unwrap().status, "pending");
            assert_eq!(store.reconcile_at_startup().unwrap().len(), 1);
            let row = store.get(ID).unwrap().unwrap();
            let prepared = service
                .prepare_copy_document_edit_recovery(
                    &row,
                    &store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let observed = match stage {
                DocumentEditStage::Intent => CopyDocumentEditObservedState::Original,
                DocumentEditStage::Document => CopyDocumentEditObservedState::DocumentApplied,
                _ => CopyDocumentEditObservedState::Applied,
            };
            assert_eq!(prepared.state(), observed);
            let result = recover_copy_document_edit(prepared, &store).unwrap();
            if stage == DocumentEditStage::Intent {
                assert_eq!(result, CopyDocumentEditObservedState::Original);
                assert_eq!(store.get(ID).unwrap().unwrap().status, "failed");
                assert_eq!(fixture.bytes(), before);
            } else {
                assert_eq!(result, CopyDocumentEditObservedState::Applied);
                assert_eq!(store.get(ID).unwrap().unwrap().status, "done");
                assert_eq!(
                    fs::read_to_string(fixture.skill.join("SKILL.md")).unwrap(),
                    fixture.request.proposed_content
                );
            }
            assert_eq!(fs::read(&fixture.sibling).unwrap(), before.2);
            assert!(store.reconcile_at_startup().unwrap().is_empty());
            let settled = fixture.bytes();
            assert!(service
                .prepare_copy_document_edit_recovery(
                    &row,
                    &store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default()
                )
                .is_err());
            assert_eq!(fixture.bytes(), settled);
        }
    }

    #[test]
    fn copy_edit_execution_recovers_failed_sql_completion_and_preserves_unrelated_registry() {
        let fixture = Fixture::new(false, false, true);
        let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        store.conn.execute_batch("CREATE TRIGGER fail_completion BEFORE UPDATE OF status ON events WHEN NEW.status = 'done' BEGIN SELECT RAISE(ABORT, 'injected completion failure'); END;").unwrap();
        let error = execute_copy_document_edit(prepare(&fixture, &mut service, &store), &store, ID)
            .unwrap_err();
        assert_eq!(error.stage, DocumentEditStage::Finish);
        assert_eq!(error.publication, DocumentEditPublication::RecoveryRequired);
        let row = store.get(ID).unwrap().unwrap();
        let prepared = service
            .prepare_copy_document_edit_recovery(
                &row,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        assert!(recover_copy_document_edit(prepared, &store).is_err());
        assert_eq!(store.get(ID).unwrap().unwrap().status, "pending");
        store
            .conn
            .execute_batch("DROP TRIGGER fail_completion")
            .unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&fixture.registry).unwrap()).unwrap();
        value["unrelated"] = serde_json::json!({"preserve": 42});
        fs::write(&fixture.registry, serde_json::to_vec(&value).unwrap()).unwrap();
        let prepared = service
            .prepare_copy_document_edit_recovery(
                &row,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(
            recover_copy_document_edit(prepared, &store).unwrap(),
            CopyDocumentEditObservedState::Applied
        );
        let current: serde_json::Value =
            serde_json::from_slice(&fs::read(&fixture.registry).unwrap()).unwrap();
        assert_eq!(current["unrelated"], value["unrelated"]);
    }

    #[test]
    fn copy_edit_recovery_refuses_changed_files_backup_or_event() {
        for changed in ["document", "resource", "registry", "backup", "event"] {
            let fixture = Fixture::new(false, false, false);
            let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            interrupted(&fixture, &mut service, &store, DocumentEditStage::Document);
            let row = store.get(ID).unwrap().unwrap();
            let prepared = service
                .prepare_copy_document_edit_recovery(
                    &row,
                    &store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            match changed {
                "document" => {
                    fs::write(fixture.skill.join("SKILL.md"), "external document").unwrap()
                }
                "resource" => {
                    fs::write(fixture.skill.join("resource.txt"), "external resource").unwrap()
                }
                "registry" => fs::write(&fixture.registry, "{}").unwrap(),
                "backup" => fs::write(
                    store.app_data.join(format!("backups/{ID}/0-SKILL.md")),
                    "changed backup",
                )
                .unwrap(),
                _ => {
                    store
                        .conn
                        .execute("UPDATE events SET skill = 'changed' WHERE id = ?1", [ID])
                        .unwrap();
                }
            }
            let changed_bytes = fixture.bytes();
            assert!(
                recover_copy_document_edit(prepared, &store).is_err(),
                "{changed}"
            );
            assert_eq!(fixture.bytes(), changed_bytes);
            assert_eq!(store.get(ID).unwrap().unwrap().status, "pending");
        }
    }

    #[test]
    fn copy_edit_cancellation_after_intent_recovers_from_observed_state() {
        for stop_at in [
            DocumentEditStage::Intent,
            DocumentEditStage::Document,
            DocumentEditStage::Registry,
        ] {
            let fixture = Fixture::new(false, false, false);
            let before = fixture.bytes();
            let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            let cancellation = CancellationToken::default();
            let CopyDocumentEditPreparation::Ready(prepared) = service
                .prepare_copy_document_edit(
                    &fixture.request,
                    std::slice::from_ref(&store.app_data),
                    Some(Duration::from_secs(10)),
                    cancellation.clone(),
                )
                .unwrap()
            else {
                panic!("expected changed edit")
            };
            let error = execute_with(*prepared, &store, ID, |stage| {
                if stage == stop_at {
                    cancellation.cancel();
                }
                Ok(())
            })
            .unwrap_err();
            assert_eq!(error.publication, DocumentEditPublication::RecoveryRequired);
            let row = store.get(ID).unwrap().unwrap();
            let prepared = service
                .prepare_copy_document_edit_recovery(
                    &row,
                    &store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            let result = recover_copy_document_edit(prepared, &store).unwrap();
            if stop_at == DocumentEditStage::Intent {
                assert_eq!(result, CopyDocumentEditObservedState::Original);
                assert_eq!(fixture.bytes(), before);
            } else {
                assert_eq!(result, CopyDocumentEditObservedState::Applied);
                assert_eq!(
                    fs::read_to_string(fixture.skill.join("SKILL.md")).unwrap(),
                    fixture.request.proposed_content
                );
                assert_eq!(fs::read(&fixture.sibling).unwrap(), before.2);
            }
        }
    }

    #[test]
    fn copy_edit_execution_refuses_event_change_before_document_publication() {
        let fixture = Fixture::new(false, false, false);
        let before = fixture.bytes();
        let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let error = execute_with(
            prepare(&fixture, &mut service, &store),
            &store,
            ID,
            |stage| {
                if stage == DocumentEditStage::Intent {
                    store
                        .conn
                        .execute("UPDATE events SET skill = 'changed' WHERE id = ?1", [ID])
                        .unwrap();
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.stage, DocumentEditStage::Document);
        assert_eq!(error.publication, DocumentEditPublication::RecoveryRequired);
        assert_eq!(fixture.bytes(), before);
        assert_eq!(store.get(ID).unwrap().unwrap().status, "pending");
    }

    #[test]
    fn copy_edit_recovery_rejects_forged_event_metadata() {
        let fixture = Fixture::new(false, false, false);
        let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        interrupted(&fixture, &mut service, &store, DocumentEditStage::Intent);
        let row = store.get(ID).unwrap().unwrap();
        for field in ["scope", "kind", "backup", "payload", "reverted"] {
            let mut changed = row.clone();
            match field {
                "scope" => changed.scope = Some("project".into()),
                "kind" => changed.kind = "repair_copy_frontmatter".into(),
                "backup" => changed.backup_dir = Some("../../outside".into()),
                "payload" => changed.payload["unexpected"] = serde_json::json!(true),
                _ => changed.reverted_by = Some("another-event".into()),
            }
            assert!(
                CopyDocumentEditRecoveryEvent::from_row(&changed).is_err(),
                "{field}"
            );
        }
    }
}

#[path = "skill_copy_document_history.rs"]
mod history;
use history::decode_edit_event;
pub use history::*;
