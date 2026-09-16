#[path = "skill_unfork_publication.rs"]
mod publication;
#[path = "skill_dotagents_runtime.rs"]
mod runtime;
pub use publication::{
    PreparedUnforkPublication, UnforkPublicationDocuments, UnforkPublicationPlan,
    UnforkPublicationState,
};

use super::*;
use crate::skill_backup_reservation::{valid_id, DotagentsStagedSourceReference};
use crate::skill_backup_reservation::{SkillsShReinstallRequest, SkillsShStagedSourceReference};
use crate::skill_dotagents_ledger::DotagentsReinstallRequest;
use crate::skill_event::{EventDraft, EventRow};
use crate::skill_event_operations::EventWriteFailure;
use crate::skill_fork_registry::OriginTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsRuntimeRecord {
    pub provider_version: String,
    pub provider_tree_identity: String,
    pub node_version: String,
    pub node_content_digest: String,
    pub copy_contract: String,
}

impl DotagentsRuntimeRecord {
    pub fn validate(&self) -> Result<(), String> {
        let version = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
        };
        let digest = |value: &str, prefix: &str| {
            value.strip_prefix(prefix).is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        };
        if !version(&self.provider_version)
            || !version(&self.node_version)
            || !digest(&self.provider_tree_identity, "tree-v1:")
            || !digest(&self.node_content_digest, "sha256:")
            || self.copy_contract != "dotagents-3.0.1-default-node-copy"
            || self.provider_version != "3.0.1"
        {
            return Err("Unsupported or invalid Unfork provider runtime record".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnforkProviderState {
    NotStarted,
    MayHaveStarted,
    SourceVerified {
        staged: DotagentsStagedSourceReference,
    },
    Publishing {
        staged: DotagentsStagedSourceReference,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsUnforkIntentV2 {
    request: DotagentsReinstallRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_contract: Option<String>,
}

impl DotagentsUnforkIntentV2 {
    pub fn request(&self) -> &DotagentsReinstallRequest {
        &self.request
    }
    fn has_native_execution_contract(&self) -> bool {
        self.execution_contract.as_deref() == Some("macos-sandbox-exec-stage-cache-v1")
    }
}

impl UnforkProviderState {
    fn staged_source(&self) -> Option<&DotagentsStagedSourceReference> {
        match self {
            Self::SourceVerified { staged } | Self::Publishing { staged } => Some(staged),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsUnforkIntent {
    version: u32,
    source_event_id: String,
    registry: UnforkRegistryTransition,
    before: UnforkSnapshotReference,
    runtime: DotagentsRuntimeRecord,
    provider: UnforkProviderState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    v2: Option<DotagentsUnforkIntentV2>,
}

impl DotagentsUnforkIntent {
    pub fn v2(
        request: DotagentsReinstallRequest,
        registry: UnforkRegistryTransition,
        before: UnforkSnapshotReference,
        runtime: DotagentsRuntimeRecord,
        execution_contract: Option<String>,
    ) -> Result<Self, String> {
        let intent = Self {
            version: 2,
            source_event_id: String::new(),
            registry,
            before,
            runtime,
            provider: UnforkProviderState::NotStarted,
            v2: Some(DotagentsUnforkIntentV2 {
                request,
                execution_contract,
            }),
        };
        intent.before.validate()?;
        intent.registry.validate()?;
        intent.runtime.validate()?;
        Ok(intent)
    }
    pub fn before(&self) -> &UnforkSnapshotReference {
        &self.before
    }
    pub fn runtime(&self) -> &DotagentsRuntimeRecord {
        &self.runtime
    }
    pub fn provider_state(&self) -> UnforkProviderState {
        self.provider.clone()
    }
    pub fn selection(&self) -> &UnforkRegistryTransition {
        &self.registry
    }
    pub fn source_event_id(&self) -> &str {
        &self.source_event_id
    }
    pub fn v2_request(&self) -> Option<&DotagentsReinstallRequest> {
        self.v2.as_ref().map(DotagentsUnforkIntentV2::request)
    }

    pub fn validate_for_operation(&self, id: &str) -> Result<(), String> {
        self.before.validate()?;
        self.registry.validate()?;
        self.runtime.validate()?;
        if let Some(staged) = self.provider.staged_source() {
            staged
                .validate_for_unfork_version(self.version)
                .map_err(|error| error.to_string())?;
            if staged.cache().operation_id() != id {
                return Err("Unfork staged source belongs to another operation".into());
            }
        }
        let v2 = self.version == 2
            && self.v2.as_ref().is_some_and(|v2| {
                DotagentsReinstallRequest::from_fork_record(
                    self.registry.recorded_provenance(),
                    self.registry.name(),
                )
                .is_ok_and(|request| request == v2.request)
            })
            && self.registry.is_bound_current()
            && self.source_event_id.is_empty();
        if !v2
            || !valid_id(id)
            || self.before.version() != self.version
            || self.before.operation_id() != id
            || self.before.deployment_id() != self.registry.record().deployment_id
            || self.registry.record().origin_tool != OriginTool::Dotagents
        {
            return Err("Unfork intent parts do not identify one operation".into());
        }
        Ok(())
    }

    pub(crate) fn draft(&self, id: &str) -> Result<EventDraft, String> {
        self.validate_for_operation(id)?;
        if self.provider != UnforkProviderState::NotStarted {
            return Err("A new Unfork event must not have a launch marker".into());
        }
        Ok(EventDraft {
            kind: "unfork_dotagents".into(),
            skill: self.registry.name().into(),
            harness: None,
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::to_value(self).map_err(|error| error.to_string())?,
            inverse: None,
            backup_dir: Some(format!("backups/{id}")),
            restorable: false,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PendingDotagentsUnforkEvent {
    row: EventRow,
    intent: DotagentsUnforkIntent,
}

impl PendingDotagentsUnforkEvent {
    pub fn from_row(row: &EventRow) -> Result<Self, String> {
        let intent: DotagentsUnforkIntent =
            serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?;
        intent.validate_for_operation(&row.id)?;
        if row.kind != "unfork_dotagents"
            || !matches!(row.status.as_str(), "pending" | "interrupted")
            || row.skill != intent.registry.name()
            || row.harness.is_some()
            || row.scope.as_deref() != Some("global")
            || row.project_path.is_some()
            || row.inverse.is_some()
            || row.reverted_by.is_some()
            || row.restorable
            || row.backup_dir.as_deref() != Some(format!("backups/{}", row.id).as_str())
            || serde_json::to_value(&intent).map_err(|error| error.to_string())? != row.payload
        {
            return Err("Unfork event envelope differs from its pending intent".into());
        }
        Ok(Self {
            row: row.clone(),
            intent,
        })
    }

    pub fn event(&self) -> &EventRow {
        &self.row
    }
    pub fn intent(&self) -> &DotagentsUnforkIntent {
        &self.intent
    }

    pub(crate) fn advance_payload(
        &self,
        next: UnforkProviderState,
    ) -> Result<serde_json::Value, String> {
        let allowed = match (&self.intent.provider, &next) {
            (UnforkProviderState::NotStarted, UnforkProviderState::MayHaveStarted) => {
                self.row.status == "pending"
            }
            (UnforkProviderState::MayHaveStarted, UnforkProviderState::SourceVerified { .. }) => {
                matches!(self.row.status.as_str(), "pending" | "interrupted")
            }
            (
                UnforkProviderState::SourceVerified { staged: before },
                UnforkProviderState::Publishing { staged: after },
            ) => before == after && matches!(self.row.status.as_str(), "pending" | "interrupted"),
            _ => false,
        };
        if !allowed {
            return Err("Unfork provider phase cannot advance from this event".into());
        }
        let mut intent = self.intent.clone();
        intent.provider = next;
        intent.validate_for_operation(&self.row.id)?;
        serde_json::to_value(intent).map_err(|error| error.to_string())
    }
}

/// The skills.sh codec is deliberately separate from the Dotagents V2 codec.
/// Interrupted Dotagents events therefore retain their exact historical JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillsShUnforkProviderState {
    NotStarted,
    MayHaveStarted,
    SourceVerified {
        staged: SkillsShStagedSourceReference,
    },
    Publishing {
        staged: SkillsShStagedSourceReference,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsShRuntimeRecord {
    pub provider_version: String,
    pub provider_tree_identity: String,
    pub node_version: String,
    pub node_content_digest: String,
    pub copy_contract: String,
}

impl SkillsShRuntimeRecord {
    pub fn validate(&self) -> Result<(), String> {
        let version = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
        };
        let digest = |value: &str, prefix: &str| {
            value.strip_prefix(prefix).is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        };
        if self.provider_version != "1.5.25"
            || self.copy_contract != "skills-1.5.25-global-universal-copy"
            || !version(&self.provider_version)
            || !version(&self.node_version)
            || !digest(&self.provider_tree_identity, "tree-v1:")
            || !digest(&self.node_content_digest, "sha256:")
        {
            return Err("Unsupported or invalid skills.sh Unfork runtime record".into());
        }
        Ok(())
    }
}

impl SkillsShUnforkProviderState {
    pub(crate) fn staged_source(&self) -> Option<&SkillsShStagedSourceReference> {
        match self {
            Self::SourceVerified { staged } | Self::Publishing { staged } => Some(staged),
            Self::NotStarted | Self::MayHaveStarted => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsShUnforkIntent {
    version: u32,
    registry: UnforkRegistryTransition,
    request: SkillsShReinstallRequest,
    before: SkillsShUnforkSnapshotReference,
    runtime: SkillsShRuntimeRecord,
    provider: SkillsShUnforkProviderState,
}

impl SkillsShUnforkIntent {
    pub fn new(
        registry: UnforkRegistryTransition,
        request: SkillsShReinstallRequest,
        before: SkillsShUnforkSnapshotReference,
        runtime: SkillsShRuntimeRecord,
    ) -> Result<Self, String> {
        let intent = Self {
            version: 1,
            registry,
            request,
            before,
            runtime,
            provider: SkillsShUnforkProviderState::NotStarted,
        };
        intent.validate_for_operation(intent.before.operation_id())?;
        Ok(intent)
    }
    pub fn selection(&self) -> &UnforkRegistryTransition {
        &self.registry
    }
    pub fn request(&self) -> &SkillsShReinstallRequest {
        &self.request
    }
    pub fn provider_state(&self) -> SkillsShUnforkProviderState {
        self.provider.clone()
    }
    pub fn before(&self) -> &SkillsShUnforkSnapshotReference {
        &self.before
    }
    pub fn runtime(&self) -> &SkillsShRuntimeRecord {
        &self.runtime
    }
    pub fn validate_for_operation(&self, id: &str) -> Result<(), String> {
        self.registry.validate()?;
        self.before.validate()?;
        self.runtime.validate()?;
        if self.version != 1
            || self.registry.record().origin_tool != OriginTool::SkillsSh
            || !self.registry.is_bound_current()
            || self.before.operation_id() != id
            || self.before.deployment_id() != self.registry.record().deployment_id
            || SkillsShReinstallRequest::from_fork_record(
                self.registry.recorded_provenance(),
                self.registry.name(),
                self.request.resolved_commit(),
            )? != self.request
        {
            return Err("skills.sh Unfork intent parts do not identify one operation".into());
        }
        if let Some(staged) = self.provider.staged_source() {
            staged.validate().map_err(|error| error.to_string())?;
            if staged.cache().operation_id() != id {
                return Err("skills.sh staged source belongs to another operation".into());
            }
        }
        Ok(())
    }
    pub(crate) fn draft(&self, id: &str) -> Result<EventDraft, String> {
        self.validate_for_operation(id)?;
        if self.provider != SkillsShUnforkProviderState::NotStarted {
            return Err("A new skills.sh Unfork event must not have a launch marker".into());
        }
        Ok(EventDraft {
            kind: "unfork_skills_sh".into(),
            skill: self.registry.name().into(),
            harness: None,
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::to_value(self).map_err(|e| e.to_string())?,
            inverse: None,
            backup_dir: Some(format!("backups/{id}")),
            restorable: false,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PendingSkillsShUnforkEvent {
    row: EventRow,
    intent: SkillsShUnforkIntent,
}
impl PendingSkillsShUnforkEvent {
    pub fn from_row(row: &EventRow) -> Result<Self, String> {
        let intent: SkillsShUnforkIntent =
            serde_json::from_value(row.payload.clone()).map_err(|e| e.to_string())?;
        intent.validate_for_operation(&row.id)?;
        if row.kind != "unfork_skills_sh"
            || !matches!(row.status.as_str(), "pending" | "interrupted")
            || row.skill != intent.registry.name()
            || row.harness.is_some()
            || row.scope.as_deref() != Some("global")
            || row.project_path.is_some()
            || row.inverse.is_some()
            || row.reverted_by.is_some()
            || row.restorable
            || row.backup_dir.as_deref() != Some(format!("backups/{}", row.id).as_str())
            || serde_json::to_value(&intent).map_err(|e| e.to_string())? != row.payload
        {
            return Err("skills.sh Unfork event envelope differs from its pending intent".into());
        }
        Ok(Self {
            row: row.clone(),
            intent,
        })
    }
    pub fn event(&self) -> &EventRow {
        &self.row
    }
    pub fn intent(&self) -> &SkillsShUnforkIntent {
        &self.intent
    }
    pub(crate) fn advance_payload(
        &self,
        next: SkillsShUnforkProviderState,
    ) -> Result<serde_json::Value, String> {
        let allowed = match (&self.intent.provider, &next) {
            (
                SkillsShUnforkProviderState::NotStarted,
                SkillsShUnforkProviderState::MayHaveStarted,
            ) => self.row.status == "pending",
            (
                SkillsShUnforkProviderState::MayHaveStarted,
                SkillsShUnforkProviderState::SourceVerified { .. },
            ) => matches!(self.row.status.as_str(), "pending" | "interrupted"),
            (
                SkillsShUnforkProviderState::SourceVerified { staged: before },
                SkillsShUnforkProviderState::Publishing { staged: after },
            ) => before == after && matches!(self.row.status.as_str(), "pending" | "interrupted"),
            _ => false,
        };
        if !allowed {
            return Err("skills.sh Unfork provider phase cannot advance from this event".into());
        }
        let mut intent = self.intent.clone();
        intent.provider = next;
        intent.validate_for_operation(&self.row.id)?;
        serde_json::to_value(intent).map_err(|e| e.to_string())
    }
}

impl PreparedSkillsShUnfork<'_> {
    pub fn record_skills_sh_pending(
        &self,
        store: &EventStore,
        id: &str,
        runtime: SkillsShRuntimeRecord,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PendingSkillsShUnforkEvent, EventWriteFailure> {
        self.revalidate(store, limits, cancellation)
            .map_err(EventWriteFailure::BeforeWrite)?;
        let guarded =
            GuardedEventStore::bind(store, &self.lease).map_err(EventWriteFailure::BeforeWrite)?;
        guarded
            .require_recovered(&self.lease)
            .map_err(EventWriteFailure::BeforeWrite)?;
        let before = self
            .publish_skills_sh_before_snapshot(store, id, limits, cancellation)
            .map_err(EventWriteFailure::MayHaveWritten)?;
        let state = BackupStateRoot::bind(&store.app_data)
            .map_err(|e| EventWriteFailure::MayHaveWritten(e.to_string()))?;
        let reservation = state
            .reserve_managed_source(id)
            .map_err(|e| EventWriteFailure::MayHaveWritten(e.to_string()))?;
        let intent = SkillsShUnforkIntent::new(
            self.selection.clone(),
            self.reinstall.clone(),
            before,
            runtime,
        )
        .map_err(EventWriteFailure::MayHaveWritten)?;
        guarded.record_pending(
            &self.lease,
            id,
            intent
                .draft(id)
                .map_err(EventWriteFailure::MayHaveWritten)?,
        )?;
        reservation
            .revalidate()
            .map_err(|e| EventWriteFailure::MayHaveWritten(e.to_string()))?;
        let row = store
            .get(id)
            .map_err(EventWriteFailure::MayHaveWritten)?
            .ok_or_else(|| {
                EventWriteFailure::MayHaveWritten("Pending skills.sh Unfork disappeared".into())
            })?;
        PendingSkillsShUnforkEvent::from_row(&row).map_err(EventWriteFailure::MayHaveWritten)
    }
    pub fn mark_skills_sh_provider_may_have_started(
        &self,
        store: &EventStore,
        expected: &PendingSkillsShUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PendingSkillsShUnforkEvent, EventWriteFailure> {
        self.validate_pending_snapshot(store, expected, limits, cancellation)
            .map_err(EventWriteFailure::BeforeWrite)?;
        GuardedEventStore::bind(store, &self.lease)
            .map_err(EventWriteFailure::BeforeWrite)?
            .advance_skills_sh_unfork_provider(
                &self.lease,
                expected,
                SkillsShUnforkProviderState::MayHaveStarted,
            )
    }
    pub fn record_skills_sh_verified_source(
        &self,
        store: &EventStore,
        expected: &PendingSkillsShUnforkEvent,
        staged: &SkillsShStagedSourceReference,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PendingSkillsShUnforkEvent, EventWriteFailure> {
        self.validate_pending_snapshot(store, expected, limits, cancellation)
            .map_err(EventWriteFailure::BeforeWrite)?;
        if staged.cache().operation_id() != expected.row.id {
            return Err(EventWriteFailure::BeforeWrite(
                "skills.sh staged source belongs to another operation".into(),
            ));
        }
        let root = BackupStateRoot::bind(&store.app_data)
            .map_err(|e| EventWriteFailure::BeforeWrite(e.to_string()))?;
        root.open_managed_source(staged.cache(), limits, cancellation)
            .map_err(|e| EventWriteFailure::BeforeWrite(e.to_string()))?
            .read_skills_sh_stage(staged, &self.reinstall, limits, cancellation)
            .map_err(|e| EventWriteFailure::BeforeWrite(e.to_string()))?;
        GuardedEventStore::bind(store, &self.lease)
            .map_err(EventWriteFailure::BeforeWrite)?
            .advance_skills_sh_unfork_provider(
                &self.lease,
                expected,
                SkillsShUnforkProviderState::SourceVerified {
                    staged: staged.clone(),
                },
            )
    }
}

impl PreparedDotagentsUnfork<'_> {
    pub fn abandon_may_have_started(
        &self,
        store: &EventStore,
        event: &PendingDotagentsUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), EventWriteFailure> {
        if event.intent.provider != UnforkProviderState::MayHaveStarted {
            return Err(EventWriteFailure::BeforeWrite(
                "Unfork provider has a recoverable phase".into(),
            ));
        }
        if !event
            .intent
            .v2
            .as_ref()
            .is_some_and(DotagentsUnforkIntentV2::has_native_execution_contract)
        {
            return Err(EventWriteFailure::BeforeWrite(
                "Unfork uncertain provider lacks the native staging contract".into(),
            ));
        }
        self.validate_pending_snapshot(store, event, limits, cancellation)
            .map_err(EventWriteFailure::BeforeWrite)?;
        GuardedEventStore::bind(store, &self.lease)
            .map_err(EventWriteFailure::BeforeWrite)?
            .abandon_may_have_started_dotagents_unfork(&self.lease, event)
    }

    pub fn resolve_unstarted(
        &self,
        store: &EventStore,
        event: &PendingDotagentsUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), EventWriteFailure> {
        if event.intent.provider != UnforkProviderState::NotStarted {
            return Err(EventWriteFailure::BeforeWrite(
                "Unfork provider may have started".into(),
            ));
        }
        self.validate_pending_snapshot(store, event, limits, cancellation)
            .map_err(EventWriteFailure::BeforeWrite)?;
        GuardedEventStore::bind(store, &self.lease)
            .map_err(EventWriteFailure::BeforeWrite)?
            .resolve_unstarted_dotagents_unfork(&self.lease, event)
    }

    pub fn record_pending(
        &self,
        store: &EventStore,
        id: &str,
        runtime: DotagentsRuntimeRecord,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PendingDotagentsUnforkEvent, EventWriteFailure> {
        self.record_pending_with_execution_contract(store, id, runtime, None, limits, cancellation)
    }

    pub fn record_native_pending(
        &self,
        store: &EventStore,
        id: &str,
        runtime: DotagentsRuntimeRecord,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PendingDotagentsUnforkEvent, EventWriteFailure> {
        self.record_pending_with_execution_contract(
            store,
            id,
            runtime,
            Some("macos-sandbox-exec-stage-cache-v1".into()),
            limits,
            cancellation,
        )
    }

    fn record_pending_with_execution_contract(
        &self,
        store: &EventStore,
        id: &str,
        runtime: DotagentsRuntimeRecord,
        execution_contract: Option<String>,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PendingDotagentsUnforkEvent, EventWriteFailure> {
        let before = EventWriteFailure::BeforeWrite;
        let after = EventWriteFailure::MayHaveWritten;
        runtime.validate().map_err(before)?;
        self.revalidate(store, limits, cancellation)
            .map_err(before)?;
        let guarded = GuardedEventStore::bind(store, &self.lease).map_err(before)?;
        guarded.require_recovered(&self.lease).map_err(before)?;
        let reference = self
            .publish_before_snapshot(store, id, limits, cancellation)
            .map_err(after)?;
        let state =
            BackupStateRoot::bind(&store.app_data).map_err(|error| after(error.to_string()))?;
        let reservation = state
            .reserve_managed_source(id)
            .map_err(|error| after(error.to_string()))?;
        let intent = DotagentsUnforkIntent::v2(
            self.reinstall.clone(),
            self.selection.clone(),
            reference,
            runtime,
            execution_contract,
        )
        .map_err(after)?;
        self.revalidate(store, limits, cancellation)
            .map_err(after)?;
        reservation
            .revalidate()
            .map_err(|error| after(error.to_string()))?;
        guarded.record_pending(&self.lease, id, intent.draft(id).map_err(after)?)?;
        let row = store
            .get(id)
            .map_err(after)?
            .ok_or_else(|| after("Pending Unfork disappeared".into()))?;
        let event = PendingDotagentsUnforkEvent::from_row(&row).map_err(after)?;
        if row.payload != serde_json::to_value(&intent).map_err(|error| after(error.to_string()))? {
            return Err(after("Pending Unfork changed during recording".into()));
        }
        reservation
            .revalidate()
            .map_err(|error| after(error.to_string()))?;
        self.validate_pending_snapshot(store, &event, limits, cancellation)
            .map_err(after)?;
        Ok(event)
    }

    fn validate_pending_snapshot(
        &self,
        store: &EventStore,
        event: &PendingDotagentsUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        self.revalidate(store, limits, cancellation)?;
        let intent = event.intent();
        if serde_json::to_value(&intent.registry).map_err(|error| error.to_string())?
            != serde_json::to_value(&self.selection).map_err(|error| error.to_string())?
            || intent.v2_request() != self.selection.is_bound_current().then_some(&self.reinstall)
        {
            return Err("Pending Unfork differs from the prepared Fork".into());
        }
        let state = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
        let backup = state
            .open_existing(&event.row.id)
            .map_err(|error| error.to_string())?;
        let receipt = UnforkSnapshotReceipt::read(&backup, &intent.before, limits, cancellation)?;
        if receipt.source_event_id() != intent.source_event_id
            || receipt.live_identity() != self.live_identity
            || receipt.v2_request() != Some(&self.reinstall)
            || serde_json::to_value(receipt.selection()).map_err(|error| error.to_string())?
                != serde_json::to_value(&self.selection).map_err(|error| error.to_string())?
        {
            return Err("Pending Unfork backup differs from preparation".into());
        }
        if let Some(staged) = intent.provider.staged_source() {
            let sealed = state
                .open_managed_source(staged.cache(), limits, cancellation)
                .map_err(|error| error.to_string())?;
            sealed
                .read_dotagents_stage_v2(staged, &self.reinstall, limits, cancellation)
                .map_err(|error| error.to_string())?;
        }
        state
            .open_managed_source_reservation(&event.row.id)
            .map_err(|error| error.to_string())?
            .revalidate()
            .map_err(|error| error.to_string())?;
        let guarded = GuardedEventStore::bind(store, &self.lease)?;
        let current = guarded
            .next_recovery_event(&self.lease)?
            .ok_or("Pending Unfork is no longer unresolved")?;
        if serde_json::to_value(&current).map_err(|error| error.to_string())?
            != serde_json::to_value(event.event()).map_err(|error| error.to_string())?
        {
            return Err("Unfork is stale or another event requires recovery first".into());
        }
        Ok(())
    }

    /// The adapter must stop provider writers before calling this under a fresh lease.
    pub fn record_verified_source(
        &self,
        store: &EventStore,
        expected: &PendingDotagentsUnforkEvent,
        staged: &DotagentsStagedSourceReference,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PendingDotagentsUnforkEvent, EventWriteFailure> {
        let invalid = EventWriteFailure::BeforeWrite;
        self.validate_pending_snapshot(store, expected, limits, cancellation)
            .map_err(invalid)?;
        staged
            .validate_for_unfork_version(expected.intent.version)
            .map_err(|error| invalid(error.to_string()))?;
        if staged.cache().operation_id() != expected.row.id {
            return Err(invalid(
                "Unfork staged source belongs to another operation".into(),
            ));
        }
        let state =
            BackupStateRoot::bind(&store.app_data).map_err(|error| invalid(error.to_string()))?;
        let sealed = state
            .open_managed_source(staged.cache(), limits, cancellation)
            .map_err(|error| invalid(error.to_string()))?;
        sealed
            .read_dotagents_stage_v2(staged, &self.reinstall, limits, cancellation)
            .map_err(|error| invalid(error.to_string()))?;
        if let UnforkProviderState::SourceVerified { staged: saved } = &expected.intent.provider {
            if saved == staged {
                self.validate_pending_snapshot(store, expected, limits, cancellation)
                    .map_err(invalid)?;
                return Ok(expected.clone());
            }
            return Err(invalid(
                "Unfork already records a different staged source".into(),
            ));
        }
        let guarded = GuardedEventStore::bind(store, &self.lease).map_err(invalid)?;
        let recorded = guarded.advance_dotagents_unfork_provider(
            &self.lease,
            expected,
            UnforkProviderState::SourceVerified {
                staged: staged.clone(),
            },
        )?;
        self.validate_pending_snapshot(store, &recorded, limits, cancellation)
            .map_err(EventWriteFailure::MayHaveWritten)?;
        Ok(recorded)
    }

    pub fn mark_provider_may_have_started(
        &self,
        store: &EventStore,
        expected: &PendingDotagentsUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<PendingDotagentsUnforkEvent, EventWriteFailure> {
        self.validate_pending_snapshot(store, expected, limits, cancellation)
            .map_err(EventWriteFailure::BeforeWrite)?;
        let guarded =
            GuardedEventStore::bind(store, &self.lease).map_err(EventWriteFailure::BeforeWrite)?;
        guarded.advance_dotagents_unfork_provider(
            &self.lease,
            expected,
            UnforkProviderState::MayHaveStarted,
        )
    }
}

impl ScopedSkillService {
    pub fn prepare_dotagents_unfork_resume(
        &mut self,
        event: &PendingDotagentsUnforkEvent,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedDotagentsUnfork<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        if cancellation.is_cancelled() {
            return Err(invalid("Unfork resume cancelled".into()));
        }
        event
            .intent
            .validate_for_operation(&event.row.id)
            .map_err(invalid)?;
        let state =
            BackupStateRoot::bind(&store.app_data).map_err(|error| invalid(error.to_string()))?;
        let backup = state
            .open_existing(&event.row.id)
            .map_err(|error| invalid(error.to_string()))?;
        UnforkSnapshotReceipt::read(&backup, &event.intent.before, limits, &cancellation)
            .map_err(invalid)?;
        let document = backup
            .read_tree_record(
                "live-tree",
                "SKILL.md",
                crate::skill_service::MAX_REPAIR_DOCUMENT_BYTES,
            )
            .map_err(|error| invalid(error.to_string()))?;
        let request = DotagentsUnforkRequest {
            deployment_id: event.intent.registry.record().deployment_id.clone(),
            expected_owner_revision: RegistryOwnerRecord::Fork(
                event.intent.registry.recorded_provenance(),
            )
            .revision()
            .ok_or_else(|| invalid("Unfork owner revision missing".into()))?,
            expected_document_fingerprint: content_fingerprint(&document),
        };
        let prepared = self.prepare_current_dotagents_unfork(
            &request,
            store,
            limits,
            timeout,
            cancellation.clone(),
        )?;
        prepared
            .validate_pending_snapshot(store, event, limits, &cancellation)
            .map_err(invalid)?;
        Ok(prepared)
    }
}

impl PreparedSkillsShUnfork<'_> {
    fn validate_pending_snapshot(
        &self,
        store: &EventStore,
        event: &PendingSkillsShUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        self.revalidate(store, limits, cancellation)?;
        let intent = event.intent();
        if serde_json::to_value(&intent.registry).map_err(|error| error.to_string())?
            != serde_json::to_value(&self.selection).map_err(|error| error.to_string())?
            || intent.request != self.reinstall
        {
            return Err("Pending Unfork differs from the prepared Fork".into());
        }
        let state = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
        let backup = state
            .open_existing(&event.row.id)
            .map_err(|error| error.to_string())?;
        let receipt = snapshot::SkillsShUnforkSnapshotReceipt::read(
            &backup,
            &intent.before,
            limits,
            cancellation,
        )?;
        if receipt.live_identity != self.live_identity
            || receipt.request != self.reinstall
            || serde_json::to_value(&receipt.selection).map_err(|error| error.to_string())?
                != serde_json::to_value(&self.selection).map_err(|error| error.to_string())?
        {
            return Err("Pending Unfork backup differs from preparation".into());
        }
        if let Some(staged) = intent.provider.staged_source() {
            let sealed = state
                .open_managed_source(staged.cache(), limits, cancellation)
                .map_err(|error| error.to_string())?;
            sealed
                .read_skills_sh_stage(staged, &self.reinstall, limits, cancellation)
                .map_err(|error| error.to_string())?;
        }
        state
            .open_managed_source_reservation(&event.row.id)
            .map_err(|error| error.to_string())?
            .revalidate()
            .map_err(|error| error.to_string())?;
        let guarded = GuardedEventStore::bind(store, &self.lease)?;
        let current = guarded
            .next_recovery_event(&self.lease)?
            .ok_or("Pending Unfork is no longer unresolved")?;
        if serde_json::to_value(&current).map_err(|error| error.to_string())?
            != serde_json::to_value(event.event()).map_err(|error| error.to_string())?
        {
            return Err("Unfork is stale or another event requires recovery first".into());
        }
        Ok(())
    }

    /// The adapter must stop all stage writers before resolving a possible launch.
    pub fn resolve_skills_sh_unapplied(
        &self,
        store: &EventStore,
        event: &PendingSkillsShUnforkEvent,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<(), EventWriteFailure> {
        if !matches!(
            event.intent.provider,
            SkillsShUnforkProviderState::NotStarted | SkillsShUnforkProviderState::MayHaveStarted
        ) {
            return Err(EventWriteFailure::BeforeWrite(
                "Verified skills.sh Unfork requires publication recovery".into(),
            ));
        }
        self.validate_pending_snapshot(store, event, limits, cancellation)
            .map_err(EventWriteFailure::BeforeWrite)?;
        GuardedEventStore::bind(store, &self.lease)
            .map_err(EventWriteFailure::BeforeWrite)?
            .finish_recovery_snapshot(
                &self.lease,
                event.event(),
                crate::skill_event::EventStatus::Failed,
                None,
            )
    }
}

impl ScopedSkillService {
    pub fn prepare_skills_sh_unfork_resume(
        &mut self,
        event: &PendingSkillsShUnforkEvent,
        store: &EventStore,
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedSkillsShUnfork<'_>, WritePreparationError> {
        let invalid = WritePreparationError::InvalidRepairSelection;
        if cancellation.is_cancelled() {
            return Err(invalid("Unfork resume cancelled".into()));
        }
        event
            .intent
            .validate_for_operation(&event.row.id)
            .map_err(invalid)?;
        let state =
            BackupStateRoot::bind(&store.app_data).map_err(|error| invalid(error.to_string()))?;
        let backup = state
            .open_existing(&event.row.id)
            .map_err(|error| invalid(error.to_string()))?;
        snapshot::SkillsShUnforkSnapshotReceipt::read(
            &backup,
            &event.intent.before,
            limits,
            &cancellation,
        )
        .map_err(invalid)?;
        let document = backup
            .read_tree_record(
                "live-tree",
                "SKILL.md",
                crate::skill_service::MAX_REPAIR_DOCUMENT_BYTES,
            )
            .map_err(|error| invalid(error.to_string()))?;
        let request = SkillsShUnforkRequest {
            deployment_id: event.intent.registry.record().deployment_id.clone(),
            expected_owner_revision: RegistryOwnerRecord::Fork(
                event.intent.registry.recorded_provenance(),
            )
            .revision()
            .ok_or_else(|| invalid("Unfork owner revision missing".into()))?,
            expected_document_fingerprint: content_fingerprint(&document),
            resolved_commit: event.intent.request.resolved_commit().into(),
        };
        let prepared = self.prepare_current_skills_sh_unfork(
            &request,
            store,
            limits,
            timeout,
            cancellation.clone(),
        )?;
        prepared
            .validate_pending_snapshot(store, event, limits, &cancellation)
            .map_err(invalid)?;
        Ok(prepared)
    }
}
