//! Durable creation of one global Universal dotagents Fork.
use crate::{
    skill_backup_copy::{inspect_entry, BackupCopyLimits},
    skill_backup_reservation::BackupStateRoot,
    skill_backup_source::BackupSourceRoot,
    skill_coordination::{CancellationToken, FinalizedWriteLease},
    skill_deployment::{parse_deployment_id, SkillDestination},
    skill_document_target::{DotagentsLockTarget, DotagentsManifestTarget, SkillRegistryTarget},
    skill_dotagents_ledger::{DotagentsDetachIntent, DotagentsForkSource},
    skill_event::{EventDraft, EventRow, EventStatus},
    skill_event_operations::{EventWriteFailure, GuardedEventStore},
    skill_event_store::EventStore,
    skill_fork_registry::{ForkRecord, OriginTool},
    skill_fork_transition::ForkRegistryTransition,
    skill_ownership::LifecycleOwnerKind,
    skill_service::{ScopedSkillService, SkillScope},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    path::{Path, PathBuf},
    time::Duration,
};

pub const EVENT_KIND: &str = "fork_dotagents";
const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
const EMPTY_REGISTRY: &[u8] = b"{}";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkCreationRequest {
    pub deployment_id: String,
    pub expected_owner_revision: String,
    pub expected_source: DotagentsForkSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkCreationOutcome {
    pub event_id: String,
    pub record: ForkRecord,
}

#[derive(Debug)]
pub struct ForkCreationError {
    pub event_id: Option<String>,
    pub recovery_required: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkCreationIntent {
    version: u32,
    scope: SkillScope,
    deployment_id: String,
    owner_revision: String,
    source: DotagentsForkSource,
    name: String,
    skill_dir: PathBuf,
    agents_dir: PathBuf,
    registry_path: PathBuf,
    live_tree: String,
    upstream_tree: String,
    baseline_before: Option<String>,
    lock_before: Vec<u8>,
    lock_after: Vec<u8>,
    manifest_before: Vec<u8>,
    manifest_after: Vec<u8>,
    registry_before: Option<Vec<u8>>,
    registry_transition: ForkRegistryTransition,
    record: ForkRecord,
}

impl ForkCreationIntent {
    fn draft(&self, event_id: &str) -> Result<EventDraft, String> {
        self.validate(event_id)?;
        Ok(EventDraft {
            kind: EVENT_KIND.into(),
            skill: self.name.clone(),
            harness: Some("universal".into()),
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::to_value(self).map_err(|error| error.to_string())?,
            inverse: None,
            backup_dir: Some(format!("backups/{event_id}")),
            restorable: false,
        })
    }

    fn from_row(row: &EventRow) -> Result<Self, String> {
        if row.kind != EVENT_KIND || row.restorable || row.inverse.is_some() {
            return Err("Not a dotagents Fork event".into());
        }
        let intent: Self =
            serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?;
        intent.validate(&row.id)?;
        if row.skill != intent.name
            || row.harness.as_deref() != Some("universal")
            || row.scope.as_deref() != Some("global")
            || row.project_path.is_some()
            || row.backup_dir.as_deref() != Some(&format!("backups/{}", row.id))
            || row.reverted_by.is_some()
        {
            return Err("Fork event metadata does not match its intent".into());
        }
        Ok(intent)
    }

    fn validate(&self, event_id: &str) -> Result<(), String> {
        let parsed =
            parse_deployment_id(&self.deployment_id).ok_or("Invalid Fork deployment ID")?;
        let lock_before =
            std::str::from_utf8(&self.lock_before).map_err(|_| "Saved agents.lock is not UTF-8")?;
        let manifest_before = std::str::from_utf8(&self.manifest_before)
            .map_err(|_| "Saved agents.toml is not UTF-8")?;
        let (lock_after, manifest_after) =
            DotagentsDetachIntent::from_documents(&self.name, lock_before, manifest_before)?
                .propose_document_detach(lock_before, manifest_before)?;
        self.registry_transition.validate()?;
        self.registry_transition
            .apply_document(self.registry_before.as_deref().unwrap_or(EMPTY_REGISTRY))?;
        if self.version != 1
            || !crate::skill_backup_reservation::valid_id(event_id)
            || !self.scope.home.is_absolute()
            || self.owner_revision.is_empty()
            || parsed.scope != "global"
            || parsed.slot != "universal"
            || parsed.destination != SkillDestination::Universal
            || parsed.name != self.name
            || parsed.lexical_path != self.skill_dir
            || self.agents_dir != self.scope.home.join(".agents")
            || self.skill_dir != self.agents_dir.join("skills").join(&self.name)
            || self.registry_path != self.agents_dir.join("skill-studio.json")
            || self.record.deployment_id != self.deployment_id
            || self.record.skill_dir != self.skill_dir
            || self.record.origin_tool != OriginTool::Dotagents
            || self.record.origin_source != self.source.source()
            || self.record.repo != self.source.repo()
            || self.record.path != self.source.path()
            || self.record.base_commit != self.source.commit()
            || self.record.declared_ref.as_deref() != self.source.declared_ref()
            || self.registry_transition.record() != &self.record
            || self.lock_after != lock_after.as_bytes()
            || self.manifest_after != manifest_after.as_bytes()
            || tree_identity(&self.live_tree).is_err()
            || tree_identity(&self.upstream_tree).is_err()
            || self
                .baseline_before
                .as_deref()
                .is_some_and(|identity| tree_identity(identity).is_err())
        {
            return Err("Invalid dotagents Fork intent".into());
        }
        Ok(())
    }

    fn registry_after(&self) -> Result<Vec<u8>, String> {
        self.registry_transition
            .apply_document(self.registry_before.as_deref().unwrap_or(EMPTY_REGISTRY))
    }
}

fn tree_identity(value: &str) -> Result<&str, String> {
    value
        .strip_prefix("tree-v1:")
        .filter(|identity| !identity.is_empty())
        .ok_or_else(|| "Invalid Fork tree identity".into())
}

fn source(path: &Path) -> Result<crate::skill_backup_source::BackupSource, String> {
    BackupSourceRoot::bind(path.parent().ok_or("Skill path has no parent")?)
        .map_err(|error| error.to_string())?
        .select(path.file_name().ok_or("Skill path has no name")?)
        .map_err(|error| error.to_string())
}

fn tree(
    path: &Path,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<String, String> {
    let source = source(path)?;
    inspect_entry(&source.directory, &source.name, limits, cancellation)
        .map(|report| format!("tree-v1:{}", report.tree_identity))
        .map_err(|error| error.to_string())
}

fn optional_tree(
    path: &Path,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<Option<String>, String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            tree(path, limits, cancellation).map(Some)
        }
        Ok(_) => Err("Fork merge base is not an independent directory".into()),
    }
}

fn baseline_path(store: &EventStore, name: &str) -> PathBuf {
    store
        .app_data
        .join("skill-studio")
        .join("forks")
        .join(name)
        .join("base")
}

#[allow(clippy::too_many_arguments)]
fn prepare_admission<'a>(
    service: &'a mut ScopedSkillService,
    store: &EventStore,
    event_id: &str,
    request: &ForkCreationRequest,
    upstream_path: &Path,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<(ForkCreationIntent, FinalizedWriteLease<'a>), String> {
    let parsed = parse_deployment_id(&request.deployment_id).ok_or("Invalid Fork deployment ID")?;
    let scope = service.scope();
    let agents_dir = scope.home.join(".agents");
    let skill_dir = agents_dir.join("skills").join(&parsed.name);
    let registry_path = agents_dir.join("skill-studio.json");
    let (inventory, lease) = service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([parsed.name.clone()])),
            &[
                store.app_data.clone(),
                upstream_path.to_path_buf(),
                agents_dir.clone(),
            ],
            &[
                skill_dir.clone(),
                upstream_path.to_path_buf(),
                registry_path.clone(),
            ],
            timeout,
            cancellation.clone(),
        )
        .map_err(|error| error.to_string())?;
    let deployments = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| deployment.id == request.deployment_id)
        .collect::<Vec<_>>();
    let deployment = deployments
        .first()
        .ok_or("Selected Fork deployment is absent")?;
    if deployments.len() != 1
        || deployment.owner_kind != LifecycleOwnerKind::Dotagents
        || deployment.is_symlink
        || deployment.path != skill_dir.to_string_lossy()
        || deployment.scope != "global"
        || deployment.destination != SkillDestination::Universal
        || deployment.agent != "shared"
        || deployment.owner_revision.as_deref() != Some(request.expected_owner_revision.as_str())
    {
        return Err(
            "Fork requires one fresh canonical named dotagents Universal deployment".into(),
        );
    }
    let lock_before = lease
        .read(&agents_dir.join("agents.lock"), MAX_DOCUMENT_BYTES)
        .map_err(|error| error.to_string())?;
    let manifest_before = lease
        .read(&agents_dir.join("agents.toml"), MAX_DOCUMENT_BYTES)
        .map_err(|error| error.to_string())?;
    let lock_text = std::str::from_utf8(&lock_before).map_err(|_| "agents.lock is not UTF-8")?;
    let manifest_text =
        std::str::from_utf8(&manifest_before).map_err(|_| "agents.toml is not UTF-8")?;
    let detach = DotagentsDetachIntent::from_documents(&parsed.name, lock_text, manifest_text)?;
    let source = detach.fork_source()?;
    if source != request.expected_source {
        return Err("Fork source changed before admission".into());
    }
    let (lock_after, manifest_after) = detach.propose_document_detach(lock_text, manifest_text)?;
    let registry_before = lease.read_ownership_registry(&registry_path, MAX_DOCUMENT_BYTES)?;
    let live_tree = tree(&skill_dir, limits, &cancellation)?;
    let upstream_tree = tree(upstream_path, limits, &cancellation)?;
    let baseline_before =
        optional_tree(&baseline_path(store, &parsed.name), limits, &cancellation)?;
    lease.revalidate().map_err(|error| error.to_string())?;
    let record = ForkRecord {
        deployment_id: request.deployment_id.clone(),
        skill_dir: skill_dir.clone(),
        forked_at: chrono::Utc::now().to_rfc3339(),
        origin_tool: OriginTool::Dotagents,
        origin_source: source.source().into(),
        repo: source.repo().into(),
        path: source.path().into(),
        declared_ref: source.declared_ref().map(str::to_owned),
        base_commit: source.commit().into(),
    };
    let registry_transition = ForkRegistryTransition::new(
        parsed.name.clone(),
        record.clone(),
        registry_before.as_deref().unwrap_or(EMPTY_REGISTRY),
    )?;
    let intent = ForkCreationIntent {
        version: 1,
        scope,
        deployment_id: request.deployment_id.clone(),
        owner_revision: request.expected_owner_revision.clone(),
        source,
        name: parsed.name,
        skill_dir,
        agents_dir,
        registry_path,
        live_tree,
        upstream_tree,
        baseline_before,
        lock_before,
        lock_after: lock_after.into_bytes(),
        manifest_before,
        manifest_after: manifest_after.into_bytes(),
        registry_before,
        registry_transition,
        record,
    };
    intent.validate(event_id)?;
    Ok((intent, lease))
}

fn preserve_evidence(
    store: &EventStore,
    event_id: &str,
    intent: &ForkCreationIntent,
    upstream_path: &Path,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    lease.revalidate().map_err(|error| error.to_string())?;
    let root = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
    let backup = root.reserve(event_id).map_err(|error| error.to_string())?;
    let result = (|| {
        let live = source(&intent.skill_dir)?;
        let upstream = source(upstream_path)?;
        let saved_live = backup
            .copy_entry(
                &live.directory,
                &live.name,
                OsStr::new("live"),
                limits,
                cancellation,
            )
            .map_err(|error| error.to_string())?;
        let saved_upstream = backup
            .copy_entry(
                &upstream.directory,
                &upstream.name,
                OsStr::new("upstream"),
                limits,
                cancellation,
            )
            .map_err(|error| error.to_string())?;
        if let Some(expected) = &intent.baseline_before {
            let old = source(&baseline_path(store, &intent.name))?;
            let saved = backup
                .copy_entry(
                    &old.directory,
                    &old.name,
                    OsStr::new("previous-base"),
                    limits,
                    cancellation,
                )
                .map_err(|error| error.to_string())?;
            if format!("tree-v1:{}", saved.tree_identity) != *expected {
                return Err("Previous Fork base changed while it was preserved".into());
            }
        }
        backup
            .write_new_file("agents.lock", &intent.lock_before)
            .map_err(|error| error.to_string())?;
        backup
            .write_new_file("agents.toml", &intent.manifest_before)
            .map_err(|error| error.to_string())?;
        match &intent.registry_before {
            Some(registry) => backup
                .write_new_file("registry.json", registry)
                .map_err(|error| error.to_string())?,
            None => backup
                .write_new_file("registry.absent", b"")
                .map_err(|error| error.to_string())?,
        }
        if format!("tree-v1:{}", saved_live.tree_identity) != intent.live_tree
            || format!("tree-v1:{}", saved_upstream.tree_identity) != intent.upstream_tree
        {
            return Err("Fork evidence changed while it was preserved".into());
        }
        backup.revalidate().map_err(|error| error.to_string())?;
        lease.revalidate().map_err(|error| error.to_string())
    })();
    if let Err(error) = result {
        return match backup.discard() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "{error}; could not remove unjournaled Fork evidence: {cleanup}"
            )),
        };
    }
    Ok(())
}

fn unjournaled_error(
    store: &EventStore,
    event_id: &str,
    message: impl Into<String>,
) -> ForkCreationError {
    let message = message.into();
    let cleanup = BackupStateRoot::bind(&store.app_data)
        .and_then(|root| root.open_existing(event_id)?.discard());
    ForkCreationError {
        event_id: None,
        recovery_required: false,
        message: match cleanup {
            Ok(()) => message,
            Err(error) => {
                format!("{message}; could not remove unjournaled Fork evidence: {error}")
            }
        },
    }
}

fn validate_scope(service: &ScopedSkillService, intent: &ForkCreationIntent) -> Result<(), String> {
    if service.scope() != intent.scope {
        return Err("Fork recovery scope changed".into());
    }
    Ok(())
}

fn prepare_effects<'a>(
    service: &'a mut ScopedSkillService,
    store: &EventStore,
    intent: &ForkCreationIntent,
    timeout: Option<Duration>,
) -> Result<FinalizedWriteLease<'a>, String> {
    validate_scope(service, intent)?;
    let (_inventory, lease) = service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([intent.name.clone()])),
            &[store.app_data.clone(), intent.agents_dir.clone()],
            &[
                intent.skill_dir.clone(),
                intent.agents_dir.join("agents.lock"),
                intent.agents_dir.join("agents.toml"),
                intent.registry_path.clone(),
            ],
            timeout,
            CancellationToken::default(),
        )
        .map_err(|error| error.to_string())?;
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok(lease)
}

fn pending(
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    row: &EventRow,
    intent: &ForkCreationIntent,
) -> Result<(), String> {
    let current = GuardedEventStore::bind(store, lease)?
        .next_recovery_event(lease)?
        .ok_or("Fork event is no longer pending")?;
    if current.id != row.id || ForkCreationIntent::from_row(&current)? != *intent {
        return Err("Fork event changed or is out of order".into());
    }
    Ok(())
}

fn verify_document_evidence(
    backup: &crate::skill_backup_reservation::ExistingBackup<'_>,
    intent: &ForkCreationIntent,
) -> Result<(), String> {
    for (name, expected) in [
        ("agents.lock", intent.lock_before.as_slice()),
        ("agents.toml", intent.manifest_before.as_slice()),
        match &intent.registry_before {
            Some(bytes) => ("registry.json", bytes.as_slice()),
            None => ("registry.absent", b"".as_slice()),
        },
    ] {
        backup
            .verify_file(name, expected)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn verify_evidence(
    store: &EventStore,
    row: &EventRow,
    intent: &ForkCreationIntent,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
) -> Result<(), String> {
    lease.revalidate().map_err(|error| error.to_string())?;
    pending(store, lease, row, intent)?;
    let backup_root = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
    let backup = backup_root
        .open_existing(&row.id)
        .map_err(|error| error.to_string())?;
    verify_document_evidence(&backup, intent)?;
    let cancellation = CancellationToken::default();
    backup
        .verify_entry(
            OsStr::new("live"),
            tree_identity(&intent.live_tree)?,
            limits,
            &cancellation,
        )
        .map_err(|error| error.to_string())?;
    backup
        .verify_entry(
            OsStr::new("upstream"),
            tree_identity(&intent.upstream_tree)?,
            limits,
            &cancellation,
        )
        .map_err(|error| error.to_string())?;
    if let Some(previous) = &intent.baseline_before {
        backup
            .verify_entry(
                OsStr::new("previous-base"),
                tree_identity(previous)?,
                limits,
                &cancellation,
            )
            .map_err(|error| error.to_string())?;
    }
    if tree(&intent.skill_dir, limits, &cancellation)? != intent.live_tree {
        return Err("Live Fork tree changed after admission".into());
    }
    lease.revalidate().map_err(|error| error.to_string())?;
    pending(store, lease, row, intent)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForwardCheckpoint {
    IntentRecorded,
    BaselineRenamed,
    BaselinePublished,
    LockDetached,
    ManifestDetached,
    RegistryPublished,
}

fn baseline_is_published(
    store: &EventStore,
    row: &EventRow,
    intent: &ForkCreationIntent,
    limits: BackupCopyLimits,
) -> Result<bool, String> {
    let cancellation = CancellationToken::default();
    let base = optional_tree(&baseline_path(store, &intent.name), limits, &cancellation)?;
    let journal = optional_tree(
        &store
            .app_data
            .join("skill-studio/forks")
            .join(&intent.name)
            .join(format!(".previous-{}", row.id)),
        limits,
        &cancellation,
    )?;
    if base.as_deref() == Some(&intent.upstream_tree) {
        if intent.baseline_before.as_deref().is_some_and(|before| {
            before != intent.upstream_tree && journal.as_deref() != Some(before)
        }) {
            return Err("Fork base was replaced without its admitted prior-base journal".into());
        }
        return Ok(true);
    }
    match (&intent.baseline_before, base, journal) {
        (None, None, None) => Ok(false),
        (Some(before), Some(base), None) if base == *before => Ok(false),
        (Some(before), None, Some(journal)) if journal == *before => Ok(false),
        _ => Err("Fork merge-base state changed after admission".into()),
    }
}

fn observe_checkpoint(
    store: &EventStore,
    row: &EventRow,
    intent: &ForkCreationIntent,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
) -> Result<ForwardCheckpoint, String> {
    let baseline = baseline_is_published(store, row, intent, limits)?;
    let lock = lease.read_retained(&intent.agents_dir.join("agents.lock"), MAX_DOCUMENT_BYTES)?;
    let manifest =
        lease.read_retained(&intent.agents_dir.join("agents.toml"), MAX_DOCUMENT_BYTES)?;
    let registry =
        lease.read_current_ownership_registry(&intent.registry_path, MAX_DOCUMENT_BYTES)?;
    let registry_after = intent.registry_after()?;
    let checkpoint = if !baseline
        && lock == intent.lock_before
        && manifest == intent.manifest_before
        && registry == intent.registry_before
    {
        ForwardCheckpoint::IntentRecorded
    } else if baseline
        && lock == intent.lock_before
        && manifest == intent.manifest_before
        && registry == intent.registry_before
    {
        ForwardCheckpoint::BaselinePublished
    } else if baseline
        && lock == intent.lock_after
        && manifest == intent.manifest_before
        && registry == intent.registry_before
    {
        ForwardCheckpoint::LockDetached
    } else if baseline
        && lock == intent.lock_after
        && manifest == intent.manifest_after
        && registry == intent.registry_before
    {
        ForwardCheckpoint::ManifestDetached
    } else if baseline
        && lock == intent.lock_after
        && manifest == intent.manifest_after
        && registry.as_deref() == Some(registry_after.as_slice())
    {
        ForwardCheckpoint::RegistryPublished
    } else {
        return Err("Dotagents Fork recovery found an invalid or changed effect prefix".into());
    };
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok(checkpoint)
}

fn publish_registry(
    intent: &ForkCreationIntent,
    lease: &mut FinalizedWriteLease<'_>,
) -> Result<(), String> {
    let proposed = intent.registry_after()?;
    let target = SkillRegistryTarget::bind(&intent.agents_dir)?;
    match &intent.registry_before {
        Some(before) => target
            .replace(lease, before, &proposed)
            .map_err(|error| error.to_string()),
        None => target
            .create(lease, &proposed)
            .map_err(|error| error.to_string()),
    }
}

fn finish(
    store: &EventStore,
    row: &EventRow,
    intent: &ForkCreationIntent,
    lease: &FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
) -> Result<(), String> {
    let state = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
    state.publish_verified_fork_base(
        lease,
        &intent.name,
        &row.id,
        tree_identity(&intent.upstream_tree)?,
        intent
            .baseline_before
            .as_deref()
            .map(tree_identity)
            .transpose()?,
        limits,
        &CancellationToken::default(),
        || pending(store, lease, row, intent),
        || Ok(()),
    )?;
    verify_evidence(store, row, intent, lease, limits)?;
    if observe_checkpoint(store, row, intent, lease, limits)?
        != ForwardCheckpoint::RegistryPublished
    {
        return Err("Fork effects are incomplete".into());
    }
    pending(store, lease, row, intent)?;
    GuardedEventStore::bind(store, lease)?
        .finish_recovery_snapshot(lease, row, EventStatus::Done, None)
        .map_err(|error| error.to_string())
}

fn execute_forward(
    store: &EventStore,
    row: &EventRow,
    intent: &ForkCreationIntent,
    mut lease: FinalizedWriteLease<'_>,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
    mut checkpoint_hook: impl FnMut(ForwardCheckpoint) -> Result<(), String>,
) -> Result<(), String> {
    verify_evidence(store, row, intent, &lease, limits)?;
    let mut checkpoint = observe_checkpoint(store, row, intent, &lease, limits)?;
    checkpoint_hook(checkpoint)?;
    loop {
        if cancellation.is_cancelled() {
            return Err("Fork cancelled after durable intent was recorded".into());
        }
        pending(store, &lease, row, intent)?;
        let evidence_root =
            BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
        let evidence = evidence_root
            .open_existing(&row.id)
            .map_err(|error| error.to_string())?;
        verify_document_evidence(&evidence, intent)?;
        checkpoint = match checkpoint {
            ForwardCheckpoint::BaselineRenamed => {
                return Err("Transient Fork baseline checkpoint cannot be resumed directly".into())
            }
            ForwardCheckpoint::IntentRecorded => {
                let state =
                    BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
                state.publish_verified_fork_base(
                    &lease,
                    &intent.name,
                    &row.id,
                    tree_identity(&intent.upstream_tree)?,
                    intent
                        .baseline_before
                        .as_deref()
                        .map(tree_identity)
                        .transpose()?,
                    limits,
                    cancellation,
                    || pending(store, &lease, row, intent),
                    || checkpoint_hook(ForwardCheckpoint::BaselineRenamed),
                )?;
                ForwardCheckpoint::BaselinePublished
            }
            ForwardCheckpoint::BaselinePublished => {
                DotagentsLockTarget::bind(&intent.agents_dir)?
                    .replace(&mut lease, &intent.lock_before, &intent.lock_after)
                    .map_err(|error| error.to_string())?;
                ForwardCheckpoint::LockDetached
            }
            ForwardCheckpoint::LockDetached => {
                DotagentsManifestTarget::bind(&intent.agents_dir)?
                    .replace(&mut lease, &intent.manifest_before, &intent.manifest_after)
                    .map_err(|error| error.to_string())?;
                ForwardCheckpoint::ManifestDetached
            }
            ForwardCheckpoint::ManifestDetached => {
                publish_registry(intent, &mut lease)?;
                ForwardCheckpoint::RegistryPublished
            }
            ForwardCheckpoint::RegistryPublished => {
                finish(store, row, intent, &lease, limits)?;
                return Ok(());
            }
        };
        lease.revalidate().map_err(|error| error.to_string())?;
        if observe_checkpoint(store, row, intent, &lease, limits)? != checkpoint {
            return Err("Fork effect did not publish its exact expected state".into());
        }
        checkpoint_hook(checkpoint)?;
    }
}

#[allow(clippy::too_many_arguments)]
fn create_with_checkpoint(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &ForkCreationRequest,
    upstream_path: &Path,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
    checkpoint_hook: impl FnMut(ForwardCheckpoint) -> Result<(), String>,
) -> Result<ForkCreationOutcome, ForkCreationError> {
    let id = crate::skill_event_store::allocate_id();
    let (intent, lease) = prepare_admission(
        service,
        store,
        &id,
        request,
        upstream_path,
        limits,
        timeout,
        cancellation.clone(),
    )
    .map_err(|message| ForkCreationError {
        event_id: None,
        recovery_required: false,
        message,
    })?;
    preserve_evidence(
        store,
        &id,
        &intent,
        upstream_path,
        &lease,
        limits,
        &cancellation,
    )
    .map_err(|message| ForkCreationError {
        event_id: None,
        recovery_required: false,
        message,
    })?;
    let draft = intent
        .draft(&id)
        .map_err(|message| unjournaled_error(store, &id, message))?;
    let guarded = GuardedEventStore::bind(store, &lease)
        .map_err(|message| unjournaled_error(store, &id, message))?;
    if let Err(error) = guarded.record_pending(&lease, &id, draft) {
        return match error {
            EventWriteFailure::MayHaveWritten(message) => Err(ForkCreationError {
                event_id: Some(id),
                recovery_required: true,
                message,
            }),
            safe
            @ (EventWriteFailure::CancelledBeforeWrite | EventWriteFailure::BeforeWrite(_)) => {
                Err(unjournaled_error(store, &id, safe.to_string()))
            }
        };
    }
    let row = store
        .get(&id)
        .map_err(|message| ForkCreationError {
            event_id: Some(id.clone()),
            recovery_required: true,
            message,
        })?
        .ok_or_else(|| ForkCreationError {
            event_id: Some(id.clone()),
            recovery_required: true,
            message: "Recorded Fork event is missing".into(),
        })?;
    execute_forward(
        store,
        &row,
        &intent,
        lease,
        limits,
        &cancellation,
        checkpoint_hook,
    )
    .map_err(|message| ForkCreationError {
        event_id: Some(id.clone()),
        recovery_required: true,
        message,
    })?;
    Ok(ForkCreationOutcome {
        event_id: id,
        record: intent.record,
    })
}

/// Create a Fork after the caller fetched `upstream_path` at the requested commit.
pub fn create_dotagents_fork(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &ForkCreationRequest,
    upstream_path: &Path,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<ForkCreationOutcome, ForkCreationError> {
    create_with_checkpoint(
        service,
        store,
        request,
        upstream_path,
        limits,
        timeout,
        cancellation,
        |_| Ok(()),
    )
}

/// Resume one oldest durable Fork event without network access.
pub fn recover_dotagents_fork(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<bool, String> {
    let intent = ForkCreationIntent::from_row(row)?;
    validate_scope(service, &intent)?;
    let lease = prepare_effects(service, store, &intent, timeout)?;
    execute_forward(
        store,
        row,
        &intent,
        lease,
        limits,
        &CancellationToken::default(),
        |_| Ok(()),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: BackupCopyLimits = BackupCopyLimits {
        max_bytes: 1024 * 1024,
        max_entries: 100,
        max_depth: 8,
    };
    const TIMEOUT: Option<Duration> = Some(Duration::from_secs(5));

    struct Fixture {
        temp: tempfile::TempDir,
        root: PathBuf,
        scope: SkillScope,
        agents: PathBuf,
        skill: PathBuf,
        upstream: PathBuf,
        state: PathBuf,
        request: ForkCreationRequest,
    }

    impl Fixture {
        fn new(with_registry: bool) -> Self {
            Self::with_temp(with_registry, tempfile::tempdir().unwrap(), None)
        }

        fn with_temp(
            with_registry: bool,
            temp: tempfile::TempDir,
            app_data: Option<PathBuf>,
        ) -> Self {
            let root = temp.path().canonicalize().unwrap();
            let home = root.join("home");
            let agents = home.join(".agents");
            let skill = agents.join("skills/sample");
            let upstream = root.join("upstream");
            std::fs::create_dir_all(&skill).unwrap();
            std::fs::create_dir_all(&upstream).unwrap();
            std::fs::write(
                skill.join("SKILL.md"),
                "---\nname: sample\ndescription: Test skill\n---\nLocal\n",
            )
            .unwrap();
            std::fs::write(skill.join("resource"), "local resource").unwrap();
            std::fs::write(
                upstream.join("SKILL.md"),
                "---\nname: sample\ndescription: Test skill\n---\nUpstream\n",
            )
            .unwrap();
            std::fs::write(upstream.join("resource"), "upstream resource").unwrap();
            std::fs::write(agents.join("agents.lock"), "version = 1\nfuture = 'keep'\n[skills.sample]\nsource = 'owner/repo'\nresolved_path = 'skills/sample'\nresolved_commit = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'\nselected_future = 'keep'\n\n[skills.sibling]\nsource = 'owner/sibling'\nresolved_path = 'skills/sibling'\nresolved_commit = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'\n").unwrap();
            std::fs::write(agents.join("agents.toml"), "version = 1\nfuture = 'keep'\n[[skills]]\nname = 'sample'\nsource = 'owner/repo'\nref = 'main'\nselected_future = 'keep'\n\n[[skills]]\nname = 'sibling'\nsource = 'owner/sibling'\nsibling_future = 'keep'\n").unwrap();
            let scope = SkillScope {
                home: home.clone(),
                projects: vec![],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            };
            let deployment_id = crate::skill_deployment::deployment_id(
                "sample",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                &skill,
            );
            if with_registry {
                let sibling_dir = agents.join("skills/sibling");
                let sibling = ForkRecord {
                    deployment_id: crate::skill_deployment::deployment_id(
                        "sibling",
                        "global",
                        SkillDestination::Universal,
                        "universal",
                        None,
                        &sibling_dir,
                    ),
                    skill_dir: sibling_dir,
                    forked_at: "2026-09-16T00:00:00Z".into(),
                    origin_tool: OriginTool::Dotagents,
                    origin_source: "owner/sibling".into(),
                    repo: "owner/sibling".into(),
                    path: "skills/sibling".into(),
                    declared_ref: None,
                    base_commit: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                };
                let trial = serde_json::json!({
                    "deployment_id": deployment_id,
                    "started_at": "2026-09-16T00:00:00Z",
                    "expires_at": "2026-09-17T00:00:00Z",
                    "status": "active",
                    "method": "dotagents",
                    "scope": "global",
                    "project_path": null,
                    "skill_dir": skill,
                    "deployment_fingerprint": "fingerprint",
                    "claude_link": null,
                    "claude_link_target": null,
                    "future": "selected-unknown"
                });
                std::fs::write(
                    agents.join("skill-studio.json"),
                    serde_json::to_vec(&serde_json::json!({
                        "version": 4,
                        "future": {"keep": true},
                        "forks": {"sibling": sibling},
                        "trials": {
                            "global/sample": trial,
                            crate::skill_fork_registry::deployment_trial_key(&deployment_id): trial,
                            "global/unrelated": {
                                "deployment_id": "unrelated",
                                "started_at": "2026-09-16T00:00:00Z",
                                "expires_at": "2026-09-17T00:00:00Z",
                                "status": "active",
                                "method": "copy",
                                "scope": "global",
                                "project_path": null,
                                "skill_dir": "/tmp/unrelated",
                                "deployment_fingerprint": "fingerprint",
                                "claude_link": null,
                                "claude_link_target": null,
                                "future": "keep"
                            }
                        }
                    }))
                    .unwrap(),
                )
                .unwrap();
            }
            let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
            let snapshot = service.scan(None, TIMEOUT).unwrap();
            let deployment = snapshot
                .skills
                .iter()
                .flat_map(|skill| &skill.deployments)
                .find(|deployment| deployment.path == skill.to_string_lossy())
                .unwrap();
            assert_eq!(deployment.owner_kind, LifecycleOwnerKind::Dotagents);
            let source = DotagentsDetachIntent::from_documents(
                "sample",
                &std::fs::read_to_string(agents.join("agents.lock")).unwrap(),
                &std::fs::read_to_string(agents.join("agents.toml")).unwrap(),
            )
            .unwrap()
            .fork_source()
            .unwrap();
            let request = ForkCreationRequest {
                deployment_id: deployment.id.clone(),
                expected_owner_revision: deployment.owner_revision.clone().unwrap(),
                expected_source: source,
            };
            Self {
                temp,
                root: root.clone(),
                scope,
                agents,
                skill,
                upstream,
                state: app_data.unwrap_or_else(|| root.join("state")),
                request,
            }
        }

        fn service(&self) -> ScopedSkillService {
            ScopedSkillService::bind(self.scope.clone()).unwrap()
        }

        fn store(&self) -> EventStore {
            EventStore::open(&self.state).unwrap()
        }

        fn create(&self) -> Result<ForkCreationOutcome, ForkCreationError> {
            create_dotagents_fork(
                &mut self.service(),
                &self.store(),
                &self.request,
                &self.upstream,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
            )
        }

        fn interrupt(&self, stop: ForwardCheckpoint) -> ForkCreationError {
            create_with_checkpoint(
                &mut self.service(),
                &self.store(),
                &self.request,
                &self.upstream,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
                |checkpoint| {
                    if checkpoint == stop {
                        Err("injected checkpoint".into())
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap_err()
        }
    }

    #[test]
    fn creates_a_fork_without_changing_the_live_tree_or_sibling_records() {
        let fixture = Fixture::new(true);
        let live_before = tree(&fixture.skill, LIMITS, &CancellationToken::default()).unwrap();
        let outcome = fixture.create().unwrap();
        assert_eq!(
            tree(&fixture.skill, LIMITS, &CancellationToken::default()).unwrap(),
            live_before
        );
        let lock = std::fs::read_to_string(fixture.agents.join("agents.lock")).unwrap();
        let manifest = std::fs::read_to_string(fixture.agents.join("agents.toml")).unwrap();
        assert!(!lock.contains("[skills.sample]"));
        assert!(lock.contains("[skills.sibling]"));
        assert!(lock.contains("future = 'keep'"));
        assert!(!manifest.contains("name = 'sample'"));
        assert!(manifest.contains("name = 'sibling'"));
        assert!(manifest.contains("sibling_future = 'keep'"));
        let registry: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.agents.join("skill-studio.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            registry["forks"]["sample"]["deployment_id"],
            fixture.request.deployment_id
        );
        assert!(registry["forks"].get("sibling").is_some());
        assert_eq!(registry["future"]["keep"], true);
        assert!(registry["trials"].get("global/sample").is_none());
        assert!(registry["trials"]
            .get(crate::skill_fork_registry::deployment_trial_key(
                &fixture.request.deployment_id
            ))
            .is_none());
        assert_eq!(registry["trials"]["global/unrelated"]["future"], "keep");
        assert_eq!(
            std::fs::read_to_string(
                fixture
                    .state
                    .join("skill-studio/forks/sample/base/SKILL.md")
            )
            .unwrap(),
            "---\nname: sample\ndescription: Test skill\n---\nUpstream\n"
        );
        let row = fixture.store().get(&outcome.event_id).unwrap().unwrap();
        assert_eq!(row.status, "done");
        assert!(!row.restorable);
        assert!(row.inverse.is_none());
    }

    #[test]
    fn creates_an_absent_registry_with_the_same_durable_projection() {
        let fixture = Fixture::new(false);
        let outcome = fixture.create().unwrap();
        let row = fixture.store().get(&outcome.event_id).unwrap().unwrap();
        assert!(ForkCreationIntent::from_row(&row)
            .unwrap()
            .registry_before
            .is_none());
        let registry: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.agents.join("skill-studio.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            registry["forks"]["sample"]["deployment_id"],
            fixture.request.deployment_id
        );
    }

    #[test]
    fn every_production_checkpoint_recovers_after_reopen() {
        for stop in [
            ForwardCheckpoint::IntentRecorded,
            ForwardCheckpoint::BaselineRenamed,
            ForwardCheckpoint::BaselinePublished,
            ForwardCheckpoint::LockDetached,
            ForwardCheckpoint::ManifestDetached,
            ForwardCheckpoint::RegistryPublished,
        ] {
            let fixture = Fixture::new(true);
            let live_before = std::fs::read(fixture.skill.join("SKILL.md")).unwrap();
            let error = fixture.interrupt(stop);
            assert!(error.recovery_required, "{stop:?}");
            drop(fixture.store());
            let reopened = fixture.store();
            reopened.reconcile_at_startup().unwrap();
            let row = reopened
                .get(error.event_id.as_deref().unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(row.status, "interrupted");
            assert!(recover_dotagents_fork(
                &mut fixture.service(),
                &reopened,
                &row,
                LIMITS,
                TIMEOUT
            )
            .unwrap());
            assert_eq!(reopened.get(&row.id).unwrap().unwrap().status, "done");
            assert_eq!(
                std::fs::read(fixture.skill.join("SKILL.md")).unwrap(),
                live_before
            );
            assert!(!fixture
                .state
                .join("skill-studio/forks/sample")
                .join(format!(".base-{}", row.id))
                .exists());
            let second_reopen = fixture.store();
            assert_eq!(second_reopen.get(&row.id).unwrap().unwrap().status, "done");
        }
    }

    #[test]
    fn mixed_external_state_refuses_without_overwriting_any_document() {
        let fixture = Fixture::new(true);
        let error = fixture.interrupt(ForwardCheckpoint::LockDetached);
        let manifest_path = fixture.agents.join("agents.toml");
        let external = b"version = 1\nexternal = 'keep'\n";
        std::fs::write(&manifest_path, external).unwrap();
        let store = fixture.store();
        store.reconcile_at_startup().unwrap();
        let row = store
            .get(error.event_id.as_deref().unwrap())
            .unwrap()
            .unwrap();
        let recovery_error =
            recover_dotagents_fork(&mut fixture.service(), &store, &row, LIMITS, TIMEOUT)
                .unwrap_err();
        assert!(
            recovery_error.contains("invalid or changed effect prefix"),
            "{recovery_error}"
        );
        assert_eq!(std::fs::read(&manifest_path).unwrap(), external);
        assert_eq!(store.get(&row.id).unwrap().unwrap().status, "interrupted");
    }

    #[test]
    fn stale_merge_base_is_preserved_then_replaced_from_immutable_upstream() {
        let fixture = Fixture::new(true);
        let old = fixture.state.join("skill-studio/forks/sample/base");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("SKILL.md"), "old merge base").unwrap();
        let outcome = fixture.create().unwrap();
        let row = fixture.store().get(&outcome.event_id).unwrap().unwrap();
        let intent = ForkCreationIntent::from_row(&row).unwrap();
        assert!(intent.baseline_before.is_some());
        assert_eq!(
            tree(
                &baseline_path(&fixture.store(), "sample"),
                LIMITS,
                &CancellationToken::default()
            )
            .unwrap(),
            intent.upstream_tree
        );
        let backup_root = BackupStateRoot::bind(&fixture.state).unwrap();
        let backup = backup_root.open_existing(&row.id).unwrap();
        backup
            .verify_entry(
                OsStr::new("previous-base"),
                tree_identity(intent.baseline_before.as_deref().unwrap()).unwrap(),
                LIMITS,
                &CancellationToken::default(),
            )
            .unwrap();
    }

    #[test]
    fn stale_owner_revision_refuses_before_recording_any_event() {
        let fixture = Fixture::new(true);
        let mut request = fixture.request.clone();
        request.expected_owner_revision = "stale".into();
        let error = create_dotagents_fork(
            &mut fixture.service(),
            &fixture.store(),
            &request,
            &fixture.upstream,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap_err();
        assert!(error.event_id.is_none());
        assert!(!error.recovery_required);
        assert!(fixture.store().list(10, None).unwrap().is_empty());
        assert!(std::fs::read_to_string(fixture.agents.join("agents.lock"))
            .unwrap()
            .contains("[skills.sample]"));
    }

    #[test]
    fn failed_prejournal_evidence_copy_discards_its_reserved_backup() {
        let fixture = Fixture::new(true);
        let event_id = crate::skill_event_store::allocate_id();
        let store = fixture.store();
        let mut service = fixture.service();
        let (intent, lease) = prepare_admission(
            &mut service,
            &store,
            &event_id,
            &fixture.request,
            &fixture.upstream,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        assert!(preserve_evidence(
            &store,
            &event_id,
            &intent,
            &fixture.upstream,
            &lease,
            LIMITS,
            &cancellation,
        )
        .is_err());
        assert!(!fixture.state.join("backups").join(&event_id).exists());
        assert!(store.list(10, None).unwrap().is_empty());
    }

    #[test]
    fn cancellation_after_durable_intent_leaves_recovery_without_effects() {
        let fixture = Fixture::new(true);
        let cancellation = CancellationToken::default();
        let control = cancellation.clone();
        let error = create_with_checkpoint(
            &mut fixture.service(),
            &fixture.store(),
            &fixture.request,
            &fixture.upstream,
            LIMITS,
            TIMEOUT,
            cancellation,
            |checkpoint| {
                if checkpoint == ForwardCheckpoint::IntentRecorded {
                    control.cancel();
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.recovery_required);
        assert!(error.message.contains("cancelled after durable intent"));
        assert!(std::fs::read_to_string(fixture.agents.join("agents.lock"))
            .unwrap()
            .contains("[skills.sample]"));
        assert!(!baseline_path(&fixture.store(), "sample").exists());
        let store = fixture.store();
        store.reconcile_at_startup().unwrap();
        let row = store
            .get(error.event_id.as_deref().unwrap())
            .unwrap()
            .unwrap();
        recover_dotagents_fork(&mut fixture.service(), &store, &row, LIMITS, TIMEOUT).unwrap();
        assert_eq!(store.get(&row.id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn changed_document_evidence_refuses_recovery_without_effects() {
        for name in [
            "agents.lock",
            "agents.toml",
            "registry.json",
            "registry.absent",
        ] {
            for remove in [false, true] {
                let fixture = Fixture::new(name != "registry.absent");
                let error = fixture.interrupt(ForwardCheckpoint::IntentRecorded);
                let event_id = error.event_id.unwrap();
                let evidence = fixture.state.join("backups").join(&event_id).join(name);
                if remove {
                    std::fs::remove_file(evidence).unwrap();
                } else {
                    std::fs::write(evidence, "tampered").unwrap();
                }
                let lock = std::fs::read(fixture.agents.join("agents.lock")).unwrap();
                let manifest = std::fs::read(fixture.agents.join("agents.toml")).unwrap();
                let store = fixture.store();
                store.reconcile_at_startup().unwrap();
                let row = store.get(&event_id).unwrap().unwrap();
                assert!(
                    recover_dotagents_fork(&mut fixture.service(), &store, &row, LIMITS, TIMEOUT)
                        .is_err(),
                    "{name}: removed={remove}"
                );
                assert_eq!(
                    std::fs::read(fixture.agents.join("agents.lock")).unwrap(),
                    lock
                );
                assert_eq!(
                    std::fs::read(fixture.agents.join("agents.toml")).unwrap(),
                    manifest
                );
                assert!(!baseline_path(&store, "sample").exists());
                assert_eq!(store.get(&event_id).unwrap().unwrap().status, "interrupted");
            }
        }
    }

    #[test]
    fn changed_immutable_upstream_evidence_refuses_recovery_without_effects() {
        let fixture = Fixture::new(true);
        let error = fixture.interrupt(ForwardCheckpoint::IntentRecorded);
        let event_id = error.event_id.unwrap();
        std::fs::write(
            fixture
                .state
                .join("backups")
                .join(&event_id)
                .join("upstream/resource"),
            "tampered",
        )
        .unwrap();
        let store = fixture.store();
        store.reconcile_at_startup().unwrap();
        let row = store.get(&event_id).unwrap().unwrap();
        assert!(
            recover_dotagents_fork(&mut fixture.service(), &store, &row, LIMITS, TIMEOUT).is_err()
        );
        assert!(std::fs::read_to_string(fixture.agents.join("agents.lock"))
            .unwrap()
            .contains("[skills.sample]"));
        assert!(!baseline_path(&store, "sample").exists());
        assert_eq!(store.get(&row.id).unwrap().unwrap().status, "interrupted");
    }

    #[test]
    fn competing_fork_owner_refuses_before_recording_any_event() {
        let fixture = Fixture::new(true);
        let registry_path = fixture.agents.join("skill-studio.json");
        let mut registry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&registry_path).unwrap()).unwrap();
        let record = ForkRecord {
            deployment_id: fixture.request.deployment_id.clone(),
            skill_dir: fixture.skill.clone(),
            forked_at: "2026-09-16T00:00:00Z".into(),
            origin_tool: OriginTool::Dotagents,
            origin_source: fixture.request.expected_source.source().into(),
            repo: fixture.request.expected_source.repo().into(),
            path: fixture.request.expected_source.path().into(),
            declared_ref: fixture
                .request
                .expected_source
                .declared_ref()
                .map(str::to_owned),
            base_commit: fixture.request.expected_source.commit().into(),
        };
        registry["forks"]["sample"] = serde_json::to_value(record).unwrap();
        std::fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
        let error = fixture.create().unwrap_err();
        assert!(error.event_id.is_none());
        assert!(!error.recovery_required);
        assert!(fixture.store().list(10, None).unwrap().is_empty());
    }

    #[test]
    #[ignore = "writes a retained native restart fixture under an explicit task-owned parent"]
    fn generate_native_restart_fixture() {
        let parent = std::env::var_os("FORK_CREATION_FIXTURE_PARENT")
            .expect("set a task-owned fixture parent directory");
        let boundary = std::env::var("FORK_CREATION_CHECKPOINT").unwrap();
        let stop = match boundary.as_str() {
            "intent" => ForwardCheckpoint::IntentRecorded,
            "baseline-renamed" => ForwardCheckpoint::BaselineRenamed,
            "baseline" => ForwardCheckpoint::BaselinePublished,
            "lock" => ForwardCheckpoint::LockDetached,
            "manifest" => ForwardCheckpoint::ManifestDetached,
            "registry" => ForwardCheckpoint::RegistryPublished,
            _ => panic!("unsupported checkpoint"),
        };
        let temp = tempfile::Builder::new()
            .prefix("native-dotfork-restart-")
            .tempdir_in(parent)
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let app_data = root.join("home/Library/Application Support/com.skillstudio.app");
        let fixture = Fixture::with_temp(true, temp, Some(app_data.clone()));
        let error = fixture.interrupt(stop);
        assert_eq!(error.message, "injected checkpoint");
        std::fs::write(
            fixture.agents.join("skill-studio-projects.json"),
            serde_json::to_vec(&serde_json::json!({"tracked": [], "excluded": []})).unwrap(),
        )
        .unwrap();
        std::fs::write(
            fixture.agents.join("skill-studio-scope.json"),
            serde_json::to_vec(&serde_json::json!({
                "backing_roots": [], "plugin_ownership_roots": []
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            fixture.root.join("checkpoint.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "checkpoint": boundary,
                "event_id": error.event_id,
                "home": fixture.scope.home,
                "app_data": app_data,
                "skill": fixture.skill,
            }))
            .unwrap(),
        )
        .unwrap();
        let kept = fixture.temp.keep();
        println!("Native Fork restart fixture: {}", kept.display());
    }
}
