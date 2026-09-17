//! Owned pending fork phase. No provider execution or completed-write authority.
use crate::{
    skill_backup_reservation::BackupCopyLimits,
    skill_coordination::{CancellationToken, CoordinationFailure},
    skill_dotagents_ledger::DotagentsDetachState,
    skill_event_operations::EventWriteFailure,
    skill_event_store::EventStore,
    skill_fork_repair_intent::DotagentsForkRecoveryEvent,
    skill_fork_snapshot::ForkSnapshotReference,
    skill_provider_observation::ProviderEffectBaseline,
    skill_service::PreparedDotagentsForkSelection,
};

pub struct PendingDotagentsFork<'scope> {
    prepared: PreparedDotagentsForkSelection<'scope>,
    baseline: ProviderEffectBaseline,
    event: DotagentsForkRecoveryEvent,
    event_store_path: std::path::PathBuf,
}

pub struct ForkPreparationFailure<'scope> {
    prepared: PreparedDotagentsForkSelection<'scope>,
    operation_id: String,
    failure: EventWriteFailure,
}

impl ForkPreparationFailure<'_> {
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn failure(&self) -> &EventWriteFailure {
        &self.failure
    }
    pub fn revalidate_selection(&self) -> Result<(), CoordinationFailure> {
        self.prepared.revalidate()
    }

    pub fn verify_operation_absent(&self, store: &EventStore) -> Result<(), String> {
        self.prepared
            .verify_operation_absent(store, &self.operation_id)
    }
}

impl<'scope> PendingDotagentsFork<'scope> {
    pub(crate) fn begin(
        prepared: PreparedDotagentsForkSelection<'scope>,
        store: &EventStore,
        snapshots: ForkSnapshotReference,
        forked_at: chrono::DateTime<chrono::Utc>,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, Box<ForkPreparationFailure<'scope>>> {
        let operation_id = snapshots.operation_id().to_owned();
        let result = (|| {
            if cancellation.is_cancelled() {
                return Err(EventWriteFailure::BeforeWrite(
                    "Fork preparation cancelled".into(),
                ));
            }
            let baseline =
                ProviderEffectBaseline::bind(&prepared).map_err(EventWriteFailure::BeforeWrite)?;
            let event = prepared.record_fork(store, snapshots, forked_at, limits, cancellation)?;
            Ok((baseline, event))
        })();
        match result {
            Ok((baseline, event)) => Ok(Self {
                prepared,
                baseline,
                event,
                event_store_path: store.app_data.clone(),
            }),
            Err(failure) => Err(Box::new(ForkPreparationFailure {
                prepared,
                operation_id,
                failure,
            })),
        }
    }

    pub fn event(&self) -> &DotagentsForkRecoveryEvent {
        &self.event
    }
    pub fn revalidate_selection(&self) -> Result<(), CoordinationFailure> {
        self.prepared.revalidate()
    }
    /// Rechecks durable intent and backups before execution. This is a discrete
    /// validation, not a transferable permission or a post-effect lease transition.
    pub fn validate_before_execution(
        &self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        if store.app_data != self.event_store_path {
            return Err("Pending fork belongs to a different event store".into());
        }
        if self.observe_provider(cancellation)? != DotagentsDetachState::Attached {
            return Err("Provider changed before fork execution".into());
        }
        self.prepared
            .validate_pending_fork(store, &self.event, limits, cancellation)
    }

    /// Publishes only provider documents and leaves the event pending. Registry,
    /// repair publication and completion are later phases. Errors retain this lease.
    pub fn publish_detach_documents(
        &mut self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), crate::skill_document_write::DocumentWriteFailure> {
        self.publish_detach_documents_with(store, limits, cancellation, || {})
    }

    pub(crate) fn publish_detach_documents_with(
        &mut self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
        after_manifest: impl FnOnce(),
    ) -> Result<(), crate::skill_document_write::DocumentWriteFailure> {
        self.validate_before_execution(store, limits, cancellation)
            .map_err(crate::skill_document_write::DocumentWriteFailure::BeforeReplace)?;
        self.prepared
            .publish_provider_documents(&self.event, cancellation, after_manifest)
    }

    /// Publishes fork ownership after native detach; the repair event stays pending.
    pub fn publish_fork_registry(
        &mut self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), crate::skill_document_write::DocumentWriteFailure> {
        use crate::skill_document_write::DocumentWriteFailure;
        if store.app_data != self.event_store_path
            || self.event.intent().provider_documents().is_none()
        {
            return Err(DocumentWriteFailure::BeforeReplace(
                "Invalid native fork store or intent".into(),
            ));
        }
        if self
            .observe_provider(cancellation)
            .map_err(DocumentWriteFailure::BeforeReplace)?
            != DotagentsDetachState::Detached
        {
            return Err(DocumentWriteFailure::BeforeReplace(
                "Provider documents are not detached".into(),
            ));
        }
        self.prepared
            .validate_pending_fork(store, &self.event, limits, cancellation)
            .map_err(DocumentWriteFailure::BeforeReplace)?;
        self.prepared.publish_fork_registry(&self.event)
    }

    pub fn publish_repair(
        &mut self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), crate::skill_document_write::DocumentWriteFailure> {
        use crate::skill_document_write::DocumentWriteFailure;
        if store.app_data != self.event_store_path {
            return Err(DocumentWriteFailure::BeforeReplace(
                "Fork belongs to a different event store".into(),
            ));
        }
        self.prepared
            .validate_pending_fork(store, &self.event, limits, cancellation)
            .map_err(DocumentWriteFailure::BeforeReplace)?;
        self.prepared.publish_fork_repair(&self.event)
    }

    /// Completes only this unchanged pending event after all four publication receipts match.
    pub fn complete(
        &self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), EventWriteFailure> {
        if store.app_data != self.event_store_path {
            return Err(EventWriteFailure::BeforeWrite(
                "Fork belongs to a different event store".into(),
            ));
        }
        self.prepared
            .validate_pending_fork(store, &self.event, limits, cancellation)
            .map_err(EventWriteFailure::BeforeWrite)?;
        self.prepared.complete_fork(store, &self.event)
    }

    pub fn observe_provider(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<DotagentsDetachState, String> {
        self.baseline.observe(&self.prepared, cancellation)
    }
}
