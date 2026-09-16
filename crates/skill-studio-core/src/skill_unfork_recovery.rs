use super::*;
use crate::skill_document_target::{ProviderDocument, ProviderDocumentTarget, SkillRegistryTarget};

pub struct PreparedUnforkPublication<'scope> {
    event: PublicationEvent,
    plan: UnforkPublicationPlan,
    live: BackupSource,
    candidate: BackupSource,
    documents: Vec<Vec<u8>>,
    published: Vec<bool>,
    state_path: PathBuf,
    lease: FinalizedWriteLease<'scope>,
}

impl ScopedSkillService {
    pub fn resume_unfork_publication(
        &self,
        event: &PendingDotagentsUnforkEvent,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<(), String> {
        self.resume_provider_publication(
            &PublicationEvent::Dotagents(event.clone()),
            store,
            limits,
            timeout,
            cancellation,
        )
    }
    pub fn resume_skills_sh_unfork_publication(
        &self,
        event: &PendingSkillsShUnforkEvent,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<(), String> {
        self.resume_provider_publication(
            &PublicationEvent::SkillsSh(event.clone()),
            store,
            limits,
            timeout,
            cancellation,
        )
    }
    pub fn prepare_unfork_publication(
        &mut self,
        event: &PendingDotagentsUnforkEvent,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedUnforkPublication<'_>, WritePreparationError> {
        self.prepare_provider_publication(
            &PublicationEvent::Dotagents(event.clone()),
            store,
            limits,
            timeout,
            cancellation,
        )
    }
    pub fn prepare_skills_sh_unfork_publication(
        &mut self,
        event: &PendingSkillsShUnforkEvent,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedUnforkPublication<'_>, WritePreparationError> {
        self.prepare_provider_publication(
            &PublicationEvent::SkillsSh(event.clone()),
            store,
            limits,
            timeout,
            cancellation,
        )
    }
    /// Resumes only a persisted publication; never launches the provider.
    fn resume_provider_publication(
        &self,
        event: &PublicationEvent,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<(), String> {
        if !event.is_publishing() {
            return Err(
                "Unfork provider stage requires explicit recovery before publication".into(),
            );
        }
        event.read_plan(store, limits, &cancellation)?;
        let root = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
        let cache_reference = event.cache()?;
        let sealed = root
            .open_managed_source(cache_reference, limits, &cancellation)
            .map_err(|error| error.to_string())?;
        let cache = sealed
            .cache_path(limits, &cancellation)
            .map_err(|error| error.to_string())?;
        let mut scope = self.scope().clone();
        if !scope.backing_roots.contains(&cache) {
            scope.backing_roots.push(cache);
        }
        let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
        let prepared = service
            .prepare_provider_publication(event, store, limits, timeout, cancellation.clone())
            .map_err(|error| format!("Unfork publication preparation: {error}"))?;
        if prepared.revalidate(store, limits, &cancellation)? == UnforkPublicationState::Before {
            prepared
                .exchange_tree(store, limits, &cancellation)
                .map_err(|error| format!("Unfork exchange requires recovery: {error:?}"))?;
            service
                .prepare_provider_publication(event, store, limits, timeout, cancellation.clone())
                .map_err(|error| format!("Unfork preparation after exchange: {error}"))?
                .publish_documents(store, limits, &cancellation)
                .map_err(|error| format!("Unfork documents after exchange: {error}"))
        } else {
            prepared
                .publish_documents(store, limits, &cancellation)
                .map_err(|error| error.to_string())
        }
    }

    fn prepare_provider_publication(
        &mut self,
        event: &PublicationEvent,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedUnforkPublication<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        if !event.is_publishing() {
            return Err(invalid("Unfork has not entered publication".into()));
        }
        let record = event.selection().record();
        let agents = self.scope().home.join(".agents");
        if record.skill_dir != agents.join("skills").join(event.selection().name()) {
            return Err(invalid(
                "Unfork publication is outside the selected home".into(),
            ));
        }
        let plan = event
            .read_plan(store, limits, &cancellation)
            .map_err(invalid)?;
        let root =
            BackupStateRoot::bind(&store.app_data).map_err(|error| invalid(error.to_string()))?;
        let cache_reference = event.cache().map_err(invalid)?;
        let sealed = root
            .open_managed_source(cache_reference, limits, &cancellation)
            .map_err(|error| invalid(error.to_string()))?;
        let candidate = event
            .candidate(&sealed, limits, &cancellation)
            .map_err(invalid)?;
        let names = BTreeSet::from([event.selection().name().to_owned()]);
        let (inventory, lease) = self.prepare_write_inventory(
            Some(&names),
            &[
                store.app_data.clone(),
                record.skill_dir.clone(),
                candidate.original_path.clone(),
            ],
            timeout,
            cancellation.clone(),
        )?;
        let deployment = exact_repair_deployment(&inventory, &record.deployment_id)?;
        if std::path::Path::new(&deployment.path) != record.skill_dir
            || deployment.is_symlink
            || deployment.plugin.is_some()
            || deployment.disabled
            || deployment.scope != "global"
            || deployment.destination != crate::skill_deployment::SkillDestination::Universal
            || !matches!(
                deployment.backing,
                crate::skill_deployment::BackingRelationship::Canonical
            )
            || !match deployment.owner_kind {
                LifecycleOwnerKind::Fork => {
                    deployment.owner_revision.as_deref()
                        == RegistryOwnerRecord::Fork(event.selection().recorded_provenance())
                            .revision()
                            .as_deref()
                }
                owner if owner == event.provider_owner() => true,
                _ => false,
            }
        {
            return Err(invalid(
                "Unfork publication owner or location changed".into(),
            ));
        }
        let read = |name: &str| {
            lease
                .read(&agents.join(name), 8 * 1024 * 1024)
                .map_err(|error| invalid(error.to_string()))
        };
        let documents = plan
            .document_names()
            .iter()
            .map(|name| read(name))
            .collect::<Result<Vec<_>, _>>()?;
        let published = vec![false; documents.len()];
        let live = BackupSourceRoot::bind(
            record
                .skill_dir
                .parent()
                .ok_or_else(|| invalid("Missing live parent".into()))?,
        )
        .map_err(|error| invalid(error.to_string()))?
        .select(
            record
                .skill_dir
                .file_name()
                .ok_or_else(|| invalid("Missing live name".into()))?,
        )
        .map_err(|error| invalid(error.to_string()))?;
        let prepared = PreparedUnforkPublication {
            event: event.clone(),
            plan,
            live,
            candidate,
            documents,
            published,
            state_path: store.app_data.clone(),
            lease,
        };
        prepared
            .revalidate(store, limits, &cancellation)
            .map_err(invalid)?;
        Ok(prepared)
    }
}

impl PreparedUnforkPublication<'_> {
    pub fn revalidate(
        &self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<UnforkPublicationState, String> {
        if store.app_data != self.state_path || cancellation.is_cancelled() {
            return Err("Unfork publication store changed or operation cancelled".into());
        }
        let guarded = GuardedEventStore::bind(store, &self.lease)?;
        let next = guarded
            .next_recovery_event(&self.lease)?
            .ok_or("Unfork event is no longer pending")?;
        if serde_json::to_value(&next).map_err(|error| error.to_string())?
            != serde_json::to_value(self.event.row()).map_err(|error| error.to_string())?
        {
            return Err("Unfork publication event or provenance changed".into());
        }
        self.event.read_plan(store, limits, cancellation)?;
        self.live.revalidate().map_err(|error| error.to_string())?;
        self.candidate
            .revalidate()
            .map_err(|error| error.to_string())?;
        let live = inspect_entry(&self.live.directory, &self.live.name, limits, cancellation)
            .map_err(|error| error.to_string())?
            .tree_identity;
        let candidate = inspect_entry(
            &self.candidate.directory,
            &self.candidate.name,
            limits,
            cancellation,
        )
        .map_err(|error| error.to_string())?
        .tree_identity;
        let agents = self
            .event
            .selection()
            .record()
            .skill_dir
            .parent()
            .and_then(|path| path.parent())
            .ok_or("Missing agents root")?;
        for (index, name) in self.plan.document_names().iter().enumerate() {
            let path = agents.join(name);
            if self.published[index] {
                self.lease
                    .validate_published_document(&path, &self.documents[index])?;
            } else if self
                .lease
                .read(&path, 8 * 1024 * 1024)
                .map_err(|error| error.to_string())?
                != self.documents[index]
            {
                return Err("Prepared publication document changed".into());
            }
        }
        let state = self.plan.observe_documents(&live, &self.documents)?;
        let expected_candidate = if state == UnforkPublicationState::Before {
            &self.plan.after_tree
        } else {
            &self.plan.before_tree
        };
        if state == UnforkPublicationState::Diverged || &candidate != expected_candidate {
            return Err(
                "Unfork live or retained candidate differs from its publication plan".into(),
            );
        }
        self.lease.revalidate().map_err(|error| error.to_string())?;
        Ok(state)
    }

    pub fn exchange_tree(
        self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), crate::skill_tree_exchange::TreeExchangeFailure> {
        use crate::skill_tree_exchange::TreeExchangeFailure::BeforeExchange;
        if self
            .revalidate(store, limits, cancellation)
            .map_err(BeforeExchange)?
            != UnforkPublicationState::Before
        {
            return Err(BeforeExchange("Unfork tree is already published".into()));
        }
        self.candidate.exchange_verified_tree(
            &self.live,
            &self.plan.after_tree,
            &self.plan.before_tree,
            &self.lease,
            limits,
            cancellation,
        )
    }

    pub fn publish_documents(
        self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), EventWriteFailure> {
        self.publish_documents_with_checkpoint(store, limits, cancellation, |_| Ok(()))
    }

    pub(crate) fn publish_documents_with_checkpoint(
        mut self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
        mut checkpoint: impl FnMut(usize) -> Result<(), String>,
    ) -> Result<(), EventWriteFailure> {
        let failure = EventWriteFailure::MayHaveWritten;
        if self
            .revalidate(store, limits, cancellation)
            .map_err(failure)?
            == UnforkPublicationState::Before
        {
            return Err(EventWriteFailure::BeforeWrite(
                "Unfork tree has not been published".into(),
            ));
        }
        let documents = self.plan.project_all(&self.documents).map_err(failure)?;
        let agents = self
            .event
            .selection()
            .record()
            .skill_dir
            .parent()
            .and_then(|path| path.parent())
            .ok_or_else(|| failure("Missing agents root".into()))?
            .to_path_buf();
        for (index, proposed) in documents.into_iter().enumerate() {
            self.revalidate(store, limits, cancellation)
                .map_err(failure)?;
            if proposed == self.documents[index] {
                continue;
            }
            let result = match self.plan.document_names()[index] {
                "agents.lock" | "agents.toml" => ProviderDocumentTarget::bind(
                    &agents,
                    if self.plan.document_names()[index] == "agents.lock" {
                        ProviderDocument::Lock
                    } else {
                        ProviderDocument::Manifest
                    },
                )
                .map_err(failure)?
                .replace(&mut self.lease, &self.documents[index], &proposed),
                ".skill-lock.json" => {
                    crate::skill_document_target::SkillsShLockTarget::bind(&agents)
                        .map_err(failure)?
                        .replace(&mut self.lease, &self.documents[index], &proposed)
                }
                _ => SkillRegistryTarget::bind(&agents)
                    .map_err(failure)?
                    .replace(&mut self.lease, &self.documents[index], &proposed),
            };
            result.map_err(|error| failure(error.to_string()))?;
            self.documents[index] = proposed;
            self.published[index] = true;
            checkpoint(index).map_err(failure)?;
        }
        if self
            .revalidate(store, limits, cancellation)
            .map_err(failure)?
            != UnforkPublicationState::Complete
        {
            return Err(failure("Unfork publication effects are incomplete".into()));
        }
        GuardedEventStore::bind(store, &self.lease)
            .map_err(failure)?
            .finish_recovery_snapshot(
                &self.lease,
                self.event.row(),
                crate::skill_event::EventStatus::Done,
                None,
            )
    }
}

#[derive(Clone)]
enum PublicationEvent {
    Dotagents(PendingDotagentsUnforkEvent),
    SkillsSh(PendingSkillsShUnforkEvent),
}
impl PublicationEvent {
    fn row(&self) -> &crate::skill_event::EventRow {
        match self {
            Self::Dotagents(event) => event.event(),
            Self::SkillsSh(event) => event.event(),
        }
    }
    fn selection(&self) -> &UnforkRegistryTransition {
        match self {
            Self::Dotagents(event) => &event.intent.registry,
            Self::SkillsSh(event) => &event.intent.registry,
        }
    }
    fn is_publishing(&self) -> bool {
        match self {
            Self::Dotagents(event) => matches!(
                event.intent.provider,
                UnforkProviderState::Publishing { .. }
            ),
            Self::SkillsSh(event) => matches!(
                event.intent.provider,
                SkillsShUnforkProviderState::Publishing { .. }
            ),
        }
    }
    fn provider_owner(&self) -> LifecycleOwnerKind {
        match self {
            Self::Dotagents(_) => LifecycleOwnerKind::Dotagents,
            Self::SkillsSh(_) => LifecycleOwnerKind::SkillsSh,
        }
    }
    fn cache(&self) -> Result<&crate::skill_backup_reservation::ManagedSourceReference, String> {
        match self {
            Self::Dotagents(event) => event
                .intent
                .provider
                .staged_source()
                .map(|source| source.cache()),
            Self::SkillsSh(event) => event
                .intent
                .provider
                .staged_source()
                .map(|source| source.cache()),
        }
        .ok_or_else(|| "Unfork source is not verified".into())
    }
    fn read_plan(
        &self,
        store: &EventStore,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<UnforkPublicationPlan, String> {
        match self {
            Self::Dotagents(event) => {
                UnforkPublicationPlan::read_saved(store, event, limits, cancellation)
            }
            Self::SkillsSh(event) => {
                UnforkPublicationPlan::read_saved_skills_sh(store, event, limits, cancellation)
            }
        }
    }
    fn candidate(
        &self,
        source: &crate::skill_backup_reservation::SealedManagedSource<'_>,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<BackupSource, String> {
        match self {
            Self::Dotagents(_) => source.open_dotagents_publication_candidate(limits, cancellation),
            Self::SkillsSh(_) => source.open_skills_sh_publication_candidate(limits, cancellation),
        }
        .map_err(|e| e.to_string())
    }
}
