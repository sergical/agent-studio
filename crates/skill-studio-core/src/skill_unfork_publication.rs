#[path = "skill_unfork_recovery.rs"]
mod recovery;
use super::*;
use crate::{
    skill_dotagents_ledger::{
        DotagentsDetachIntent, DotagentsDetachState, DotagentsReattachProposal,
    },
    skill_fork_transition::UnforkRegistryState,
};
pub use recovery::PreparedUnforkPublication;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnforkPublicationState {
    Before,
    TreeReplaced,
    ProviderPartial,
    ProviderAttached,
    Complete,
    Diverged,
}

pub struct UnforkPublicationPlan {
    before_tree: String,
    after_tree: String,
    selected_rows: ProviderRows,
    registry: UnforkRegistryTransition,
}

pub struct UnforkPublicationDocuments {
    pub provider: DotagentsReattachProposal,
    pub registry: Vec<u8>,
}

impl UnforkPublicationPlan {
    /// Reconstructs evidence only; callers still need fresh scope and write admission.
    pub fn read_saved(
        store: &EventStore,
        event: &PendingDotagentsUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, String> {
        let check_event = || -> Result<(), String> {
            if cancellation.is_cancelled() {
                return Err("Unfork publication read cancelled".into());
            }
            let current = store
                .get(&event.row.id)?
                .ok_or("Unfork event disappeared")?;
            if serde_json::to_value(&current).map_err(|error| error.to_string())?
                != serde_json::to_value(&event.row).map_err(|error| error.to_string())?
            {
                return Err("Unfork publication event changed".into());
            }
            PendingDotagentsUnforkEvent::from_row(&current)?;
            Ok(())
        };
        check_event()?;
        let staged = event
            .intent
            .provider
            .staged_source()
            .ok_or("Unfork source is not verified")?;
        let root = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
        let backup = root
            .open_existing(&event.row.id)
            .map_err(|error| error.to_string())?;
        let before =
            UnforkSnapshotReceipt::read(&backup, &event.intent.before, limits, cancellation)?;
        if before.source_event_id() != event.intent.source_event_id
            || serde_json::to_value(before.selection()).map_err(|error| error.to_string())?
                != serde_json::to_value(&event.intent.registry)
                    .map_err(|error| error.to_string())?
        {
            return Err("Unfork publication intent differs from its before snapshot".into());
        }
        let cache = root
            .open_managed_source(staged.cache(), limits, cancellation)
            .map_err(|error| error.to_string())?;
        let request = event
            .intent
            .v2_request()
            .ok_or("Missing V2 Unfork request")?;
        let receipt = cache
            .read_dotagents_stage_v2(staged, request, limits, cancellation)
            .map_err(|error| error.to_string())?;
        let (after_tree, selected_rows) = (
            receipt.installed_identity().to_owned(),
            receipt.selected_rows().map_err(|error| error.to_string())?,
        );
        let plan = Self {
            before_tree: before.live_identity().into(),
            after_tree,
            selected_rows: ProviderRows::Dotagents(selected_rows),
            registry: before.selection().clone(),
        };
        check_event()?;
        Ok(plan)
    }

    pub fn observe(
        &self,
        live_identity: &str,
        lock: &str,
        manifest: &str,
        registry: &[u8],
    ) -> Result<UnforkPublicationState, String> {
        use UnforkPublicationState::*;
        let ProviderRows::Dotagents(rows) = &self.selected_rows else {
            return Err("Dotagents documents supplied for skills.sh publication".into());
        };
        let provider = rows.observe(Some(lock), Some(manifest))?;
        let owner = self.registry.observe_document(registry)?;
        if live_identity == self.after_tree {
            return Ok(match (provider, owner) {
                (DotagentsDetachState::Detached, UnforkRegistryState::Before) => TreeReplaced,
                (DotagentsDetachState::Partial, UnforkRegistryState::Before) => ProviderPartial,
                (DotagentsDetachState::Attached, UnforkRegistryState::Before) => ProviderAttached,
                (DotagentsDetachState::Attached, UnforkRegistryState::After) => Complete,
                _ => Diverged,
            });
        }
        if live_identity == self.before_tree
            && provider == DotagentsDetachState::Detached
            && owner == UnforkRegistryState::Before
        {
            return Ok(Before);
        }
        Ok(Diverged)
    }

    pub fn project_documents(
        &self,
        live_identity: &str,
        lock: &str,
        manifest: &str,
        registry: &[u8],
    ) -> Result<UnforkPublicationDocuments, String> {
        if self.observe(live_identity, lock, manifest, registry)?
            == UnforkPublicationState::Diverged
        {
            return Err("Unfork publication differs from its saved selected effects".into());
        }
        let ProviderRows::Dotagents(rows) = &self.selected_rows else {
            return Err("Dotagents documents supplied for skills.sh publication".into());
        };
        Ok(UnforkPublicationDocuments {
            provider: rows.propose_document_reattach(lock, manifest)?,
            registry: self.registry.apply_document(registry)?,
        })
    }
}

impl PreparedDotagentsUnfork<'_> {
    pub fn publication_plan(
        &self,
        store: &EventStore,
        event: &PendingDotagentsUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<UnforkPublicationPlan, String> {
        self.validate_pending_snapshot(store, event, limits, cancellation)?;
        let plan = UnforkPublicationPlan::read_saved(store, event, limits, cancellation)?;
        plan.project_documents(
            &self.live_identity,
            std::str::from_utf8(&self.provider_lock).map_err(|error| error.to_string())?,
            std::str::from_utf8(&self.provider_manifest).map_err(|error| error.to_string())?,
            &self.registry,
        )?;
        self.validate_pending_snapshot(store, event, limits, cancellation)?;
        Ok(plan)
    }

    pub fn begin_publication(
        &self,
        store: &EventStore,
        expected: &PendingDotagentsUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PendingDotagentsUnforkEvent, EventWriteFailure> {
        self.publication_plan(store, expected, limits, cancellation)
            .map_err(EventWriteFailure::BeforeWrite)?;
        let staged = expected.intent.provider.staged_source().ok_or_else(|| {
            EventWriteFailure::BeforeWrite("Unfork source is not verified".into())
        })?;
        self.lease
            .validate_state_tree(&store.app_data)
            .map_err(EventWriteFailure::BeforeWrite)?;
        let root = BackupStateRoot::bind(&store.app_data)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        let source = root
            .open_managed_source(staged.cache(), limits, cancellation)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        {
            source
                .prepare_dotagents_publication_candidate_v2(
                    staged,
                    &self.reinstall,
                    limits,
                    cancellation,
                )
                .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        }
        self.validate_pending_snapshot(store, expected, limits, cancellation)
            .map_err(EventWriteFailure::BeforeWrite)?;
        if matches!(
            expected.intent.provider,
            UnforkProviderState::Publishing { .. }
        ) {
            return Ok(expected.clone());
        }
        let guarded =
            GuardedEventStore::bind(store, &self.lease).map_err(EventWriteFailure::BeforeWrite)?;
        let event = guarded.advance_dotagents_unfork_provider(
            &self.lease,
            expected,
            UnforkProviderState::Publishing {
                staged: staged.clone(),
            },
        )?;
        self.validate_pending_snapshot(store, &event, limits, cancellation)
            .map_err(EventWriteFailure::MayHaveWritten)?;
        Ok(event)
    }
}

enum ProviderRows {
    Dotagents(DotagentsDetachIntent),
    SkillsSh(crate::skill_skills_sh_lock_transition::SkillsShLockTransition),
}

impl UnforkPublicationPlan {
    fn read_saved_skills_sh(
        store: &EventStore,
        event: &PendingSkillsShUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, String> {
        let current = store
            .get(&event.row.id)?
            .ok_or("skills.sh Unfork event disappeared")?;
        if serde_json::to_value(&current).map_err(|e| e.to_string())?
            != serde_json::to_value(&event.row).map_err(|e| e.to_string())?
        {
            return Err("skills.sh Unfork event changed".into());
        }
        PendingSkillsShUnforkEvent::from_row(&current)?;
        let root = BackupStateRoot::bind(&store.app_data).map_err(|e| e.to_string())?;
        let backup = root
            .open_existing(&event.row.id)
            .map_err(|e| e.to_string())?;
        let before = snapshot::SkillsShUnforkSnapshotReceipt::read(
            &backup,
            &event.intent.before,
            limits,
            cancellation,
        )?;
        if before.request != event.intent.request
            || serde_json::to_value(&before.selection).map_err(|e| e.to_string())?
                != serde_json::to_value(&event.intent.registry).map_err(|e| e.to_string())?
        {
            return Err("skills.sh Unfork snapshot differs from event".into());
        }
        let staged = event
            .intent
            .provider
            .staged_source()
            .ok_or("skills.sh Unfork source is not verified")?;
        let source = root
            .open_managed_source(staged.cache(), limits, cancellation)
            .map_err(|e| e.to_string())?;
        let receipt = source
            .read_skills_sh_stage(staged, &event.intent.request, limits, cancellation)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            before_tree: before.live_identity,
            after_tree: receipt.installed_identity().into(),
            selected_rows: ProviderRows::SkillsSh(receipt.selected_row().clone()),
            registry: before.selection,
        })
    }

    fn document_names(&self) -> &'static [&'static str] {
        match self.selected_rows {
            ProviderRows::Dotagents(_) => &["agents.lock", "agents.toml", "skill-studio.json"],
            ProviderRows::SkillsSh(_) => &[".skill-lock.json", "skill-studio.json"],
        }
    }

    fn observe_documents(
        &self,
        live: &str,
        documents: &[Vec<u8>],
    ) -> Result<UnforkPublicationState, String> {
        use UnforkPublicationState::*;
        if documents.len() != self.document_names().len() {
            return Err("Unfork provider document count differs".into());
        }
        match &self.selected_rows {
            ProviderRows::Dotagents(_) => self.observe(
                live,
                std::str::from_utf8(&documents[0]).map_err(|e| e.to_string())?,
                std::str::from_utf8(&documents[1]).map_err(|e| e.to_string())?,
                &documents[2],
            ),
            ProviderRows::SkillsSh(rows) => {
                use crate::skill_skills_sh_lock_transition::SkillsShLockState as Provider;
                let provider = rows.observe(&documents[0])?;
                let owner = self.registry.observe_document(&documents[1])?;
                if live == self.after_tree {
                    return Ok(match (provider, owner) {
                        (Provider::Detached, UnforkRegistryState::Before) => TreeReplaced,
                        (Provider::Attached, UnforkRegistryState::Before) => ProviderAttached,
                        (Provider::Attached, UnforkRegistryState::After) => Complete,
                        _ => Diverged,
                    });
                }
                Ok(
                    if live == self.before_tree
                        && provider == Provider::Detached
                        && owner == UnforkRegistryState::Before
                    {
                        Before
                    } else {
                        Diverged
                    },
                )
            }
        }
    }

    fn project_all(&self, documents: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, String> {
        if self.observe_documents(&self.after_tree, documents)? == UnforkPublicationState::Diverged
        {
            return Err("Unfork documents diverged".into());
        }
        match &self.selected_rows {
            ProviderRows::Dotagents(_) => {
                let projected = self.project_documents(
                    &self.after_tree,
                    std::str::from_utf8(&documents[0]).map_err(|e| e.to_string())?,
                    std::str::from_utf8(&documents[1]).map_err(|e| e.to_string())?,
                    &documents[2],
                )?;
                Ok(vec![
                    projected.provider.lock().as_bytes().to_vec(),
                    projected.provider.manifest().as_bytes().to_vec(),
                    projected.registry,
                ])
            }
            ProviderRows::SkillsSh(rows) => Ok(vec![
                rows.apply_document(&documents[0])?,
                self.registry.apply_document(&documents[1])?,
            ]),
        }
    }
}

impl PreparedSkillsShUnfork<'_> {
    pub fn begin_skills_sh_publication(
        &self,
        store: &EventStore,
        expected: &PendingSkillsShUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PendingSkillsShUnforkEvent, EventWriteFailure> {
        let failure = EventWriteFailure::BeforeWrite;
        self.validate_pending_snapshot(store, expected, limits, cancellation)
            .map_err(failure)?;
        let plan =
            UnforkPublicationPlan::read_saved_skills_sh(store, expected, limits, cancellation)
                .map_err(failure)?;
        if serde_json::to_value(&self.selection).map_err(|e| failure(e.to_string()))?
            != serde_json::to_value(&plan.registry).map_err(|e| failure(e.to_string()))?
            || self.reinstall != expected.intent.request
        {
            return Err(failure("skills.sh Unfork selection differs".into()));
        }
        if plan
            .observe_documents(
                &self.live_identity,
                &[self.provider_lock.clone(), self.registry.clone()],
            )
            .map_err(failure)?
            != UnforkPublicationState::Before
        {
            return Err(failure("skills.sh Unfork before state differs".into()));
        }
        let staged = expected
            .intent
            .provider
            .staged_source()
            .ok_or_else(|| failure("skills.sh source is not verified".into()))?;
        self.lease
            .validate_state_tree(&store.app_data)
            .map_err(failure)?;
        let root = BackupStateRoot::bind(&store.app_data).map_err(|e| failure(e.to_string()))?;
        root.open_managed_source(staged.cache(), limits, cancellation)
            .map_err(|e| failure(e.to_string()))?
            .prepare_skills_sh_publication_candidate(staged, &self.reinstall, limits, cancellation)
            .map_err(|e| failure(e.to_string()))?;
        self.validate_pending_snapshot(store, expected, limits, cancellation)
            .map_err(failure)?;
        if matches!(
            expected.intent.provider,
            SkillsShUnforkProviderState::Publishing { .. }
        ) {
            return Ok(expected.clone());
        }
        GuardedEventStore::bind(store, &self.lease)
            .map_err(failure)?
            .advance_skills_sh_unfork_provider(
                &self.lease,
                expected,
                SkillsShUnforkProviderState::Publishing {
                    staged: staged.clone(),
                },
            )
    }
}
