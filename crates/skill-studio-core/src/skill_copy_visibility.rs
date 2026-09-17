//! Exact Copy visibility preparation. Requests and persisted intents grant no path authority.
use crate::{
    skill_backup_copy::{inspect_entry, BackupCopyLimits},
    skill_backup_source::{BackupSource, BackupSourceRoot},
    skill_coordination::{CancellationToken, FinalizedWriteLease},
    skill_copy_move::CopyMoveTransition,
    skill_copy_move_intent::CopyMoveIntent,
    skill_deployment::parse_deployment_id,
    skill_discovery::{live_skill_content_hash_with_check, STUDIO_DISABLED_DIR_NAME},
    skill_fork_registry::{CopyDeploymentRecord, RegistryOwnerRecord},
    skill_ownership::LifecycleOwnerKind,
    skill_scope::SkillReadScope,
    skill_service::ScopedSkillService,
};
use crate::{
    skill_copy_move_intent::CopyMoveObservedState,
    skill_event::EventStatus,
    skill_event_operations::{EventWriteFailure, GuardedEventStore},
    skill_event_store::EventStore,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

#[derive(Debug, PartialEq, Eq)]
pub enum CopyVisibilityOutcome {
    Unchanged {
        deployment_id: String,
    },
    Changed {
        event_id: String,
        deployment_id: String,
    },
}

#[derive(Debug)]
pub struct CopyVisibilityError {
    pub event_id: Option<String>,
    pub recovery_required: bool,
    pub message: String,
}

/// Once admitted, settlement uses a fresh cancellation token so cancellation
/// cannot strand a moved directory without its registry record.
pub fn set_copy_visibility(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &CopyVisibilityRequest,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<CopyVisibilityOutcome, CopyVisibilityError> {
    set_copy_visibility_from_source(service, store, request, limits, timeout, cancellation, None)
}

pub fn reverse_copy_visibility(
    service: &mut ScopedSkillService,
    store: &EventStore,
    source_id: &str,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<CopyVisibilityOutcome, CopyVisibilityError> {
    let before = |message| CopyVisibilityError {
        event_id: None,
        recovery_required: false,
        message,
    };
    let source = store
        .get(source_id)
        .map_err(before)?
        .ok_or_else(|| before("Copy visibility event is missing".into()))?;
    if source.status != "done" || source.reverted_by.is_some() {
        return Err(before(
            "Copy visibility event is no longer available to reverse".into(),
        ));
    }
    let intent = CopyMoveIntent::from_event(&source).map_err(before)?;
    let record = intent.transition().after();
    let request = CopyVisibilityRequest {
        deployment_id: record.deployment_id.clone(),
        expected_owner_revision: RegistryOwnerRecord::Copy(record)
            .revision()
            .ok_or_else(|| before("Copy owner revision is missing".into()))?,
        enabled: record.disabled,
    };
    set_copy_visibility_from_source(
        service,
        store,
        &request,
        limits,
        timeout,
        cancellation,
        Some(&source),
    )
}

fn set_copy_visibility_from_source(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &CopyVisibilityRequest,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
    source: Option<&crate::skill_event::EventRow>,
) -> Result<CopyVisibilityOutcome, CopyVisibilityError> {
    let before = |message| CopyVisibilityError {
        event_id: None,
        recovery_required: false,
        message,
    };
    let preparation = service
        .prepare_copy_visibility(
            request,
            std::slice::from_ref(&store.app_data),
            limits,
            timeout,
            cancellation.clone(),
        )
        .map_err(before)?;
    let CopyVisibilityPreparation::Ready(mut prepared) = preparation else {
        return Ok(CopyVisibilityOutcome::Unchanged {
            deployment_id: request.deployment_id.clone(),
        });
    };
    if let Some(source) = source {
        prepared.intent = prepared.intent.reversing(source).map_err(before)?;
    }
    let intent = prepared.intent.clone();
    let id = crate::skill_event_store::allocate_id();
    let events = GuardedEventStore::bind(store, &prepared.lease).map_err(before)?;
    match source {
        Some(source) => events.record_copy_move_reversal(&prepared.lease, source, &id, &intent),
        None => events.record_pending(&prepared.lease, &id, intent.event_draft().map_err(before)?),
    }
    .map_err(|failure| CopyVisibilityError {
        event_id: matches!(failure, EventWriteFailure::MayHaveWritten(_)).then(|| id.clone()),
        recovery_required: matches!(failure, EventWriteFailure::MayHaveWritten(_)),
        message: failure.to_string(),
    })?;
    let needs_parent = prepared.needs_holding_directory();
    let movement = if needs_parent {
        prepared.ensure_holding_directory()
    } else {
        prepared.move_tree()
    };
    let movement = if needs_parent && movement.is_ok() {
        service
            .prepare_copy_visibility(
                request,
                std::slice::from_ref(&store.app_data),
                limits,
                timeout,
                cancellation,
            )
            .and_then(|preparation| match preparation {
                CopyVisibilityPreparation::Ready(mut prepared) => {
                    if let Some(source) = source {
                        prepared.intent = prepared.intent.reversing(source)?;
                    }
                    require_pending_intent(store, &prepared.lease, &id, &intent)?;
                    if serde_json::to_value(prepared.intent()).map_err(|e| e.to_string())?
                        != serde_json::to_value(&intent).map_err(|e| e.to_string())?
                    {
                        return Err("Copy move inputs changed after parent creation".into());
                    }
                    prepared.move_tree()
                }
                CopyVisibilityPreparation::Unchanged { .. } => {
                    Err("Copy changed after event admission".into())
                }
            })
    } else {
        movement
    };
    match settle_copy_visibility(service, store, &id, &intent, limits, timeout) {
        Ok(true) => Ok(CopyVisibilityOutcome::Changed {
            event_id: id,
            deployment_id: intent.transition().after().deployment_id.clone(),
        }),
        Ok(false) => Err(CopyVisibilityError {
            event_id: Some(id),
            recovery_required: false,
            message: movement
                .err()
                .unwrap_or_else(|| "Copy move did not occur".into()),
        }),
        Err(message) => Err(CopyVisibilityError {
            event_id: Some(id),
            recovery_required: true,
            message: match movement {
                Ok(()) => message,
                Err(error) => format!("{error}; {message}"),
            },
        }),
    }
}

fn require_pending_intent(
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    id: &str,
    intent: &CopyMoveIntent,
) -> Result<crate::skill_event::EventRow, String> {
    let events = GuardedEventStore::bind(store, lease)?;
    let row = events
        .next_recovery_event(lease)?
        .ok_or("Copy move event is no longer pending")?;
    if row.id != id
        || row.restorable
        || row.inverse.is_some()
        || row.backup_dir.is_some()
        || row.reverted_by.is_some()
        || serde_json::to_value(CopyMoveIntent::from_event(&row)?).map_err(|e| e.to_string())?
            != serde_json::to_value(intent).map_err(|e| e.to_string())?
    {
        return Err("Pending Copy move event changed or is out of order".into());
    }
    events.validate_copy_move_claim(lease, &row, intent)?;
    Ok(row)
}

pub fn recover_copy_visibility(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &crate::skill_event::EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<bool, String> {
    let intent = CopyMoveIntent::from_event(row)?;
    settle_copy_visibility(service, store, &row.id, &intent, limits, timeout)
}

fn settle_copy_visibility(
    service: &mut ScopedSkillService,
    store: &EventStore,
    id: &str,
    intent: &CopyMoveIntent,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<bool, String> {
    intent.validate()?;
    let scope = service.scope();
    if intent.registry_path() != scope.home.join(".agents/skill-studio.json") {
        return Err("Copy move registry is outside the selected home".into());
    }
    let before = intent.transition().before();
    let after = intent.transition().after();
    let active = if before.disabled { after } else { before };
    if !crate::skill_agents::skill_roots(&scope.home, &scope.projects)
        .iter()
        .any(|root| {
            root.path.join(&active.name) == active.path
                && root.project_path.as_deref()
                    == active.project_path.as_deref().map(std::path::Path::new)
                && root.label != "shared"
                && root.label != "parked"
        })
    {
        return Err("Copy move path is outside configured harness roots".into());
    }
    let cancellation = CancellationToken::default();
    let trees = [
        store.app_data.clone(),
        active
            .path
            .parent()
            .ok_or("Copy root missing")?
            .to_path_buf(),
    ];
    let entries = [before.path.clone(), after.path.clone()];
    let (inventory, mut lease) = service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([before.name.clone()])),
            &trees,
            &entries,
            timeout,
            cancellation.clone(),
        )
        .map_err(|e| e.to_string())?;
    let mut matches = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| {
            deployment.id == before.deployment_id || deployment.id == after.deployment_id
        });
    let deployment = matches.next().ok_or("Copy move deployment is missing")?;
    if matches.next().is_some() {
        return Err("Copy move deployment is ambiguous".into());
    }
    if deployment.is_symlink
        || deployment.plugin.is_some()
        || !matches!(
            deployment.owner_kind,
            LifecycleOwnerKind::Copy
                | LifecycleOwnerKind::Manual
                | LifecycleOwnerKind::Unknown
                | LifecycleOwnerKind::InRepo
        )
    {
        return Err("Copy move recovery ownership changed".into());
    }
    let snapshot = require_pending_intent(store, &lease, id, intent)?;
    let observe = |lease: &FinalizedWriteLease<'_>,
                   published: Option<&[u8]>|
     -> Result<(CopyMoveObservedState, Vec<u8>), String> {
        lease.revalidate().map_err(|e| e.to_string())?;
        let source = observe_move_tree(&before.path, limits, &cancellation)?;
        let destination = observe_move_tree(&after.path, limits, &cancellation)?;
        let registry = match published {
            Some(bytes) => {
                lease.validate_published_document(intent.registry_path(), bytes)?;
                bytes.to_vec()
            }
            None => lease
                .read(intent.registry_path(), 8 * 1024 * 1024)
                .map_err(|e| e.to_string())?,
        };
        let state = intent.observe(source.as_deref(), destination.as_deref(), &registry)?;
        lease.revalidate().map_err(|e| e.to_string())?;
        Ok((state, registry))
    };
    let (state, registry) = observe(&lease, None)?;
    let published = if state == CopyMoveObservedState::TreeMoved {
        let proposed = intent.transition().apply_document(&registry)?;
        let target = crate::skill_document_target::SkillRegistryTarget::bind(
            intent
                .registry_path()
                .parent()
                .ok_or("Registry parent missing")?,
        )?;
        let current = require_pending_intent(store, &lease, id, intent)?;
        if serde_json::to_value(&current).map_err(|e| e.to_string())?
            != serde_json::to_value(&snapshot).map_err(|e| e.to_string())?
        {
            return Err("Copy move event changed before registry publication".into());
        }
        target
            .replace(&mut lease, &registry, &proposed)
            .map_err(|e| e.to_string())?;
        Some(proposed)
    } else {
        None
    };
    let (final_state, _) = observe(&lease, published.as_deref())?;
    let applied = match final_state {
        CopyMoveObservedState::BeforeMove => false,
        CopyMoveObservedState::RegistryPublished => true,
        CopyMoveObservedState::TreeMoved => {
            return Err("Copy registry publication did not settle".into())
        }
    };
    require_pending_intent(store, &lease, id, intent)?;
    GuardedEventStore::bind(store, &lease)?
        .finish_copy_move_recovery(
            &lease,
            &snapshot,
            intent,
            if applied {
                EventStatus::Done
            } else {
                EventStatus::Failed
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(applied)
}

fn observe_move_tree(
    path: &std::path::Path,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<Option<String>, String> {
    let parent = path.parent().ok_or("Copy tree parent missing")?;
    if parent
        .file_name()
        .is_some_and(|name| name == STUDIO_DISABLED_DIR_NAME)
    {
        let root = BackupSourceRoot::bind(parent.parent().ok_or("Holding root missing")?)
            .map_err(|e| e.to_string())?;
        let holding = root
            .select(std::ffi::OsStr::new(STUDIO_DISABLED_DIR_NAME))
            .map_err(|e| e.to_string())?;
        match holding.directory.symlink_metadata(&holding.name) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err("Copy holding path is not a directory".into()),
        }
    }
    let scope = SkillReadScope::bind(&[parent.to_path_buf()]).map_err(|e| e.to_string())?;
    match scope.clone_bound_directory(parent) {
        Ok(directory) => {
            let directory: cap_std::fs::Dir = directory.into();
            let name = path.file_name().ok_or("Copy tree name missing")?;
            match directory.symlink_metadata(name) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.to_string()),
                Ok(metadata) if metadata.is_dir() => {
                    inspect_entry(&directory, name, limits, cancellation)
                        .map(|report| Some(report.tree_identity))
                        .map_err(|e| e.to_string())
                }
                Ok(_) => Err("Copy move tree is not an independent directory".into()),
            }
        }
        Err(error) => Err(error.to_string()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyVisibilityRequest {
    pub deployment_id: String,
    pub expected_owner_revision: String,
    pub enabled: bool,
}

pub enum CopyVisibilityPreparation<'scope> {
    Unchanged { deployment_id: String },
    Ready(Box<PreparedCopyVisibility<'scope>>),
}

pub struct PreparedCopyVisibility<'scope> {
    intent: CopyMoveIntent,
    registry_original: Vec<u8>,
    source: BackupSource,
    destination: Option<BackupSource>,
    holding: BackupSource,
    content_scope: SkillReadScope,
    lease: FinalizedWriteLease<'scope>,
    limits: BackupCopyLimits,
    cancellation: CancellationToken,
}

impl PreparedCopyVisibility<'_> {
    fn ensure_holding_directory(self) -> Result<(), String> {
        self.revalidate()?;
        self.holding
            .directory
            .create_dir(&self.holding.name)
            .map_err(|e| e.to_string())?;
        self.holding
            .directory
            .try_clone()
            .map_err(|e| e.to_string())?
            .into_std_file()
            .sync_all()
            .map_err(|e| e.to_string())
    }

    fn move_tree(self) -> Result<(), String> {
        self.revalidate()?;
        let destination = self
            .destination
            .as_ref()
            .ok_or("Copy holding directory is absent")?;
        self.source
            .move_verified_tree(
                destination,
                self.intent.expected_tree(),
                self.lease,
                self.limits,
                &self.cancellation,
            )
            .map_err(|failure| match failure {
                crate::skill_tree_move::TreeMoveFailure::BeforeMove(message)
                | crate::skill_tree_move::TreeMoveFailure::MayHaveMoved(message) => message,
            })
    }

    pub fn intent(&self) -> &CopyMoveIntent {
        &self.intent
    }

    pub fn needs_holding_directory(&self) -> bool {
        self.destination.is_none()
    }

    pub fn revalidate(&self) -> Result<(), String> {
        let error = |error: std::io::Error| error.to_string();
        self.lease.revalidate().map_err(|error| error.to_string())?;
        self.source.revalidate().map_err(error)?;
        self.holding.revalidate().map_err(error)?;
        if !self
            .source
            .directory
            .symlink_metadata(&self.source.name)
            .map_err(error)?
            .is_dir()
        {
            return Err("Copy visibility requires an independent directory".into());
        }
        match &self.destination {
            Some(destination) => {
                destination.revalidate().map_err(error)?;
                require_absent(destination)?;
                self.lease.validate_tree_move(
                    &self.intent.transition().before().path,
                    &self.intent.transition().after().path,
                )?;
            }
            None => require_absent(&self.holding)?,
        }
        let tree = inspect_entry(
            &self.source.directory,
            &self.source.name,
            self.limits,
            &self.cancellation,
        )
        .map_err(error)?;
        let record = self.intent.transition().before();
        let hash = live_skill_content_hash_with_check(&self.content_scope, &record.path, || {
            if self.cancellation.is_cancelled() {
                Err("Copy visibility cancelled".into())
            } else {
                Ok(())
            }
        })?;
        if tree.tree_identity != self.intent.expected_tree() || hash != record.content_hash {
            return Err("Copy visibility source content changed".into());
        }
        let registry = self
            .lease
            .read(self.intent.registry_path(), 8 * 1024 * 1024)
            .map_err(|error| error.to_string())?;
        if registry != self.registry_original {
            return Err("Copy visibility registry changed".into());
        }
        self.intent.transition().apply_document(&registry)?;
        self.lease.revalidate().map_err(|error| error.to_string())
    }
}

fn require_absent(source: &BackupSource) -> Result<(), String> {
    match source.directory.symlink_metadata(&source.name) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
        Ok(_) => Err("Copy visibility destination is occupied".into()),
    }
}

impl ScopedSkillService {
    pub fn prepare_copy_visibility(
        &mut self,
        request: &CopyVisibilityRequest,
        additional_trees: &[PathBuf],
        limits: BackupCopyLimits,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<CopyVisibilityPreparation<'_>, String> {
        let selected =
            parse_deployment_id(&request.deployment_id).ok_or("Invalid Copy deployment ID")?;
        let source_path = selected.lexical_path;
        let parent = source_path.parent().ok_or("Copy path has no parent")?;
        let root = if parent
            .file_name()
            .is_some_and(|name| name == STUDIO_DISABLED_DIR_NAME)
        {
            parent
                .parent()
                .ok_or("Copy holding directory has no parent")?
        } else {
            parent
        };
        let holding_path = root.join(STUDIO_DISABLED_DIR_NAME);
        let destination_path = if request.enabled {
            root.join(&selected.name)
        } else {
            holding_path.join(&selected.name)
        };
        let mut trees = additional_trees.to_vec();
        trees.push(source_path.clone());
        let entries = [source_path.clone(), destination_path, holding_path.clone()];
        let (inventory, lease) = self
            .prepare_write_inventory_with_entries(
                Some(&BTreeSet::from([selected.name])),
                &trees,
                &entries,
                timeout,
                cancellation.clone(),
            )
            .map_err(|error| error.to_string())?;
        let mut matches = inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .filter(|deployment| deployment.id == request.deployment_id);
        let deployment = matches.next().ok_or("Selected Copy deployment is absent")?;
        if matches.next().is_some()
            || deployment.owner_kind != LifecycleOwnerKind::Copy
            || deployment.owner_revision.as_deref()
                != Some(request.expected_owner_revision.as_str())
            || deployment.is_symlink
            || std::path::Path::new(&deployment.path) != source_path
        {
            return Err("Copy visibility ownership is ambiguous, unsupported or changed".into());
        }
        let registry_path = inventory.scope.home.join(".agents/skill-studio.json");
        let registry_original = lease
            .read(&registry_path, 8 * 1024 * 1024)
            .map_err(|error| error.to_string())?;
        let document: serde_json::Value =
            serde_json::from_slice(&registry_original).map_err(|error| error.to_string())?;
        let record: CopyDeploymentRecord = serde_json::from_value(
            document
                .get("copies")
                .and_then(|copies| copies.get(&request.deployment_id))
                .ok_or("Selected Copy registry record is absent")?
                .clone(),
        )
        .map_err(|error| error.to_string())?;
        if record.path != source_path
            || RegistryOwnerRecord::Copy(&record).revision().as_deref()
                != Some(request.expected_owner_revision.as_str())
        {
            return Err("Copy visibility registry ownership changed".into());
        }
        let transition = CopyMoveTransition::new(record.clone(), record.disabled)?;
        let source = BackupSourceRoot::bind(parent)
            .map_err(|error| error.to_string())?
            .select(source_path.file_name().ok_or("Copy source has no name")?)
            .map_err(|error| error.to_string())?;
        let holding = BackupSourceRoot::bind(root)
            .map_err(|error| error.to_string())?
            .select(std::ffi::OsStr::new(STUDIO_DISABLED_DIR_NAME))
            .map_err(|error| error.to_string())?;
        let content_scope = SkillReadScope::bind(std::slice::from_ref(&source_path))
            .map_err(|error| error.to_string())?;
        if !source
            .directory
            .symlink_metadata(&source.name)
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            return Err("Copy visibility requires an independent directory".into());
        }
        let tree = inspect_entry(&source.directory, &source.name, limits, &cancellation)
            .map_err(|error| error.to_string())?;
        let hash = live_skill_content_hash_with_check(&content_scope, &source_path, || {
            if cancellation.is_cancelled() {
                Err("Copy visibility cancelled".into())
            } else {
                Ok(())
            }
        })?;
        if hash != record.content_hash {
            return Err("Copy visibility source content changed".into());
        }
        lease.revalidate().map_err(|error| error.to_string())?;
        if record.disabled != request.enabled {
            return Ok(CopyVisibilityPreparation::Unchanged {
                deployment_id: request.deployment_id.clone(),
            });
        }
        transition.apply_document(&registry_original)?;
        let target = &transition.after().path;
        let destination = match holding.directory.symlink_metadata(&holding.name) {
            Ok(metadata) if metadata.is_dir() => Some(
                BackupSourceRoot::bind(target.parent().ok_or("Copy destination has no parent")?)
                    .map_err(|error| error.to_string())?
                    .select(target.file_name().ok_or("Copy destination has no name")?)
                    .map_err(|error| error.to_string())?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !request.enabled => None,
            Err(error) => return Err(error.to_string()),
            Ok(_) => return Err("Copy holding path is not a directory".into()),
        };
        let prepared = PreparedCopyVisibility {
            intent: CopyMoveIntent::new(transition, tree.tree_identity, registry_path)?,
            registry_original,
            source,
            destination,
            holding,
            content_scope,
            lease,
            limits,
            cancellation,
        };
        prepared.revalidate()?;
        Ok(CopyVisibilityPreparation::Ready(Box::new(prepared)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        skill_deployment::{InstallScope, SkillDestination},
        skill_fork_registry::ForkRegistry,
        skill_service::SkillScope,
    };
    use std::{fs, os::unix::fs::symlink};

    struct Fixture {
        _temp: tempfile::TempDir,
        scope: SkillScope,
        path: PathBuf,
        registry: PathBuf,
        request: CopyVisibilityRequest,
    }
    impl Fixture {
        fn new(project: bool, disabled: bool) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().canonicalize().unwrap();
            let project_path = home.join("project");
            let root = if project { &project_path } else { &home };
            let path = root.join(if disabled {
                ".codex/skills/.skill-studio-disabled/sample"
            } else {
                ".codex/skills/sample"
            });
            fs::create_dir_all(&path).unwrap();
            fs::create_dir_all(home.join(".agents")).unwrap();
            fs::create_dir_all(project_path.join(".git")).unwrap();
            fs::create_dir(home.join(".git")).unwrap();
            fs::write(
                path.join("SKILL.md"),
                "---\nname: sample\ndescription: Fixture\n---\nBody\n",
            )
            .unwrap();
            let scope = SkillScope {
                home: home.clone(),
                projects: vec![project_path.clone()],
                backing_roots: vec![],
                plugin_ownership_roots: vec![],
            };
            let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
            let inventory = service.scan(None, Some(Duration::from_secs(10))).unwrap();
            let deployment = inventory
                .skills
                .iter()
                .flat_map(|skill| &skill.deployments)
                .find(|deployment| std::path::Path::new(&deployment.path) == path)
                .unwrap();
            let record = CopyDeploymentRecord {
                deployment_id: deployment.id.clone(),
                name: "sample".into(),
                path: path.clone(),
                scope: if project {
                    InstallScope::Project
                } else {
                    InstallScope::Global
                },
                destination: SkillDestination::PerHarness,
                slot: "codex".into(),
                project_path: project.then(|| project_path.to_string_lossy().into_owned()),
                content_hash: deployment.content_hash.clone(),
                disabled,
            };
            let request = CopyVisibilityRequest {
                deployment_id: record.deployment_id.clone(),
                expected_owner_revision: RegistryOwnerRecord::Copy(&record).revision().unwrap(),
                enabled: disabled,
            };
            let mut registry = ForkRegistry::default();
            registry.copies.insert(record.deployment_id.clone(), record);
            let registry_path = home.join(".agents/skill-studio.json");
            fs::write(
                &registry_path,
                serde_json::to_vec_pretty(&registry).unwrap(),
            )
            .unwrap();
            Self {
                _temp: temp,
                scope,
                path,
                registry: registry_path,
                request,
            }
        }
        fn prepare<'a>(
            &self,
            service: &'a mut ScopedSkillService,
        ) -> Result<CopyVisibilityPreparation<'a>, String> {
            service.prepare_copy_visibility(
                &self.request,
                &[],
                BackupCopyLimits {
                    max_bytes: 1024 * 1024,
                    max_entries: 100,
                    max_depth: 10,
                },
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
        }
    }

    #[test]
    fn visibility_undo_redo_reverses_files_and_ownership_once() {
        for project in [false, true] {
            let fixture = Fixture::new(project, false);
            let original: serde_json::Value =
                serde_json::from_slice(&fs::read(&fixture.registry).unwrap()).unwrap();
            let document = fs::read(fixture.path.join("SKILL.md")).unwrap();
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            let store = EventStore::open(&fixture.scope.home.join("history")).unwrap();
            let limits = BackupCopyLimits {
                max_bytes: 1024 * 1024,
                max_entries: 100,
                max_depth: 10,
            };
            let CopyVisibilityOutcome::Changed { mut event_id, .. } = set_copy_visibility(
                &mut service,
                &store,
                &fixture.request,
                limits,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap() else {
                panic!("expected move")
            };
            for (index, enabled) in [true, false, true].into_iter().enumerate() {
                let source_id = event_id;
                let CopyVisibilityOutcome::Changed {
                    event_id: next,
                    deployment_id,
                } = reverse_copy_visibility(
                    &mut service,
                    &store,
                    &source_id,
                    limits,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap()
                else {
                    panic!("expected reversal")
                };
                let row = store.get(&next).unwrap().unwrap();
                let intent = CopyMoveIntent::from_event(&row).unwrap();
                assert_eq!(row.status, "done");
                assert_eq!(
                    store
                        .get(&source_id)
                        .unwrap()
                        .unwrap()
                        .reverted_by
                        .as_deref(),
                    Some(next.as_str())
                );
                assert_eq!(intent.transition().after().deployment_id, deployment_id);
                assert_eq!(intent.transition().after().disabled, !enabled);
                assert_eq!(
                    fs::read(intent.transition().after().path.join("SKILL.md")).unwrap(),
                    document
                );
                let registry: serde_json::Value =
                    serde_json::from_slice(&fs::read(&fixture.registry).unwrap()).unwrap();
                if enabled {
                    assert_eq!(registry, original);
                }
                let bytes = fs::read(&fixture.registry).unwrap();
                assert!(reverse_copy_visibility(
                    &mut service,
                    &store,
                    &source_id,
                    limits,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default()
                )
                .is_err());
                assert_eq!(fs::read(&fixture.registry).unwrap(), bytes);
                assert_eq!(store.list(10, None).unwrap().len(), index + 2);
                event_id = next;
            }
            fs::write(fixture.path.join("SKILL.md"), "external edit").unwrap();
            let registry = fs::read(&fixture.registry).unwrap();
            assert!(reverse_copy_visibility(
                &mut service,
                &store,
                &event_id,
                limits,
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .is_err());
            assert_eq!(fs::read(&fixture.registry).unwrap(), registry);
            assert_eq!(
                fs::read(fixture.path.join("SKILL.md")).unwrap(),
                b"external edit"
            );
            assert_eq!(store.list(10, None).unwrap().len(), 4);
        }
    }

    #[test]
    fn interrupted_visibility_reversal_finishes_or_releases_its_claim() {
        for project in [false, true] {
            for phase in ["intent", "moved", "published", "conflict"] {
                let fixture = Fixture::new(project, false);
                let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                let store = EventStore::open(&fixture.scope.home.join("history")).unwrap();
                let limits = BackupCopyLimits {
                    max_bytes: 1024 * 1024,
                    max_entries: 100,
                    max_depth: 10,
                };
                let CopyVisibilityOutcome::Changed { event_id, .. } = set_copy_visibility(
                    &mut service,
                    &store,
                    &fixture.request,
                    limits,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap() else {
                    panic!("expected move")
                };
                let source = store.get(&event_id).unwrap().unwrap();
                let source_intent = CopyMoveIntent::from_event(&source).unwrap();
                let record = source_intent.transition().after();
                let request = CopyVisibilityRequest {
                    deployment_id: record.deployment_id.clone(),
                    expected_owner_revision: RegistryOwnerRecord::Copy(record).revision().unwrap(),
                    enabled: true,
                };
                let CopyVisibilityPreparation::Ready(mut prepared) = service
                    .prepare_copy_visibility(
                        &request,
                        std::slice::from_ref(&store.app_data),
                        limits,
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap()
                else {
                    panic!("expected reversal")
                };
                prepared.intent = prepared.intent.reversing(&source).unwrap();
                let intent = prepared.intent.clone();
                GuardedEventStore::bind(&store, &prepared.lease)
                    .unwrap()
                    .record_copy_move_reversal(
                        &prepared.lease,
                        &source,
                        "interrupted-undo",
                        &intent,
                    )
                    .unwrap();
                if phase == "intent" {
                    drop(prepared);
                } else {
                    prepared.move_tree().unwrap();
                }
                if phase == "published" {
                    fs::write(
                        &fixture.registry,
                        intent
                            .transition()
                            .apply_document(&fs::read(&fixture.registry).unwrap())
                            .unwrap(),
                    )
                    .unwrap();
                }
                if phase == "conflict" {
                    fs::write(fixture.path.join("SKILL.md"), "external edit").unwrap();
                }
                store
                    .conn
                    .execute(
                        "UPDATE events SET status = 'interrupted' WHERE id = 'interrupted-undo'",
                        [],
                    )
                    .unwrap();
                let row = store.get("interrupted-undo").unwrap().unwrap();
                let registry = fs::read(&fixture.registry).unwrap();
                drop(service);
                let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                let result = recover_copy_visibility(
                    &mut service,
                    &store,
                    &row,
                    limits,
                    Some(Duration::from_secs(10)),
                );
                if phase == "conflict" {
                    assert!(result.is_err());
                    assert_eq!(store.get(&row.id).unwrap().unwrap().status, "interrupted");
                    assert_eq!(fs::read(&fixture.registry).unwrap(), registry);
                    assert_eq!(
                        fs::read(fixture.path.join("SKILL.md")).unwrap(),
                        b"external edit"
                    );
                } else {
                    assert_eq!(result.unwrap(), phase != "intent");
                    assert_eq!(
                        store.get(&row.id).unwrap().unwrap().status,
                        if phase == "intent" { "failed" } else { "done" }
                    );
                }
                assert_eq!(
                    store
                        .get(&event_id)
                        .unwrap()
                        .unwrap()
                        .reverted_by
                        .as_deref(),
                    if phase == "intent" {
                        None
                    } else {
                        Some("interrupted-undo")
                    }
                );
                if phase == "intent" {
                    reverse_copy_visibility(
                        &mut service,
                        &store,
                        &event_id,
                        limits,
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                }
            }
        }
    }

    #[test]
    fn admission_and_finish_failures_preserve_recoverable_effects() {
        for mode in ["cancel", "admit", "finish"] {
            let fixture = Fixture::new(false, false);
            let original = fs::read(&fixture.registry).unwrap();
            let store = EventStore::open(&fixture.scope.home.join("history")).unwrap();
            let cancellation = CancellationToken::default();
            if mode == "cancel" {
                cancellation.cancel();
            } else {
                let statement = if mode == "admit" {
                    "INSERT"
                } else {
                    "UPDATE OF status"
                };
                store
                    .conn
                    .execute_batch(&format!(
                        "CREATE TRIGGER reject_visibility BEFORE {statement} ON events
                     BEGIN SELECT RAISE(ABORT, 'injected visibility failure'); END;"
                    ))
                    .unwrap();
            }
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            let limits = BackupCopyLimits {
                max_bytes: 1024 * 1024,
                max_entries: 100,
                max_depth: 10,
            };
            let error = set_copy_visibility(
                &mut service,
                &store,
                &fixture.request,
                limits,
                Some(Duration::from_secs(10)),
                cancellation,
            )
            .unwrap_err();
            let events = store.list(10, None).unwrap();
            if mode == "finish" {
                assert!(error.recovery_required);
                assert_eq!(events.len(), 1);
                assert_eq!(error.event_id.as_deref(), Some(events[0].id.as_str()));
                assert_eq!(events[0].status, "pending");
                assert!(!fixture.path.exists());
                store
                    .conn
                    .execute_batch("DROP TRIGGER reject_visibility")
                    .unwrap();
                assert!(recover_copy_visibility(
                    &mut service,
                    &store,
                    &events[0],
                    limits,
                    Some(Duration::from_secs(10))
                )
                .unwrap());
                assert_eq!(store.get(&events[0].id).unwrap().unwrap().status, "done");
            } else {
                assert!(events.is_empty());
                assert!(fixture.path.join("SKILL.md").is_file());
                assert!(!fixture
                    .path
                    .parent()
                    .unwrap()
                    .join(STUDIO_DISABLED_DIR_NAME)
                    .exists());
                assert_eq!(fs::read(&fixture.registry).unwrap(), original);
            }
        }
    }

    #[test]
    fn cancellation_after_admission_settles_without_abandoning_registry() {
        for project in [false, true] {
            for disabled in [false, true] {
                for phase in ["before-parent", "before-move", "after-move"] {
                    let fixture = Fixture::new(project, disabled);
                    let original = fs::read(&fixture.registry).unwrap();
                    let document = fs::read(fixture.path.join("SKILL.md")).unwrap();
                    let store = EventStore::open(&fixture.scope.home.join("history")).unwrap();
                    let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                    let limits = BackupCopyLimits {
                        max_bytes: 1024 * 1024,
                        max_entries: 100,
                        max_depth: 10,
                    };
                    let cancellation = CancellationToken::default();
                    let CopyVisibilityPreparation::Ready(prepared) = service
                        .prepare_copy_visibility(
                            &fixture.request,
                            std::slice::from_ref(&store.app_data),
                            limits,
                            Some(Duration::from_secs(10)),
                            cancellation.clone(),
                        )
                        .unwrap()
                    else {
                        panic!("expected move")
                    };
                    let intent = prepared.intent.clone();
                    GuardedEventStore::bind(&store, &prepared.lease)
                        .unwrap()
                        .record_pending(
                            &prepared.lease,
                            "cancel-fixture",
                            intent.event_draft().unwrap(),
                        )
                        .unwrap();
                    if phase == "before-parent" {
                        cancellation.cancel();
                        let result = if prepared.needs_holding_directory() {
                            prepared.ensure_holding_directory()
                        } else {
                            prepared.move_tree()
                        };
                        assert!(result.is_err());
                    } else {
                        if prepared.needs_holding_directory() {
                            prepared.ensure_holding_directory().unwrap();
                        } else {
                            drop(prepared);
                        }
                        let CopyVisibilityPreparation::Ready(prepared) = service
                            .prepare_copy_visibility(
                                &fixture.request,
                                std::slice::from_ref(&store.app_data),
                                limits,
                                Some(Duration::from_secs(10)),
                                cancellation.clone(),
                            )
                            .unwrap()
                        else {
                            panic!("expected move")
                        };
                        if phase == "before-move" {
                            cancellation.cancel();
                            assert!(prepared.move_tree().is_err());
                        } else {
                            prepared.move_tree().unwrap();
                            cancellation.cancel();
                        }
                    }
                    let row = store.get("cancel-fixture").unwrap().unwrap();
                    let applied = recover_copy_visibility(
                        &mut service,
                        &store,
                        &row,
                        limits,
                        Some(Duration::from_secs(10)),
                    )
                    .unwrap();
                    assert_eq!(applied, phase == "after-move");
                    assert!(cancellation.is_cancelled());
                    assert_eq!(
                        store.get(&row.id).unwrap().unwrap().status,
                        if applied { "done" } else { "failed" }
                    );
                    if applied {
                        assert!(!fixture.path.exists());
                        assert_eq!(
                            fs::read(intent.transition().after().path.join("SKILL.md")).unwrap(),
                            document
                        );
                        let actual: serde_json::Value =
                            serde_json::from_slice(&fs::read(&fixture.registry).unwrap()).unwrap();
                        let expected: serde_json::Value = serde_json::from_slice(
                            &intent.transition().apply_document(&original).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(actual, expected);
                    } else {
                        assert_eq!(fs::read(fixture.path.join("SKILL.md")).unwrap(), document);
                        assert!(!intent.transition().after().path.exists());
                        assert_eq!(fs::read(&fixture.registry).unwrap(), original);
                    }
                }
            }
        }
    }

    #[test]
    fn durable_visibility_round_trip_preserves_registry_metadata() {
        for project in [false, true] {
            for existing_parent in [false, true] {
                let fixture = Fixture::new(project, false);
                if existing_parent {
                    fs::create_dir(
                        fixture
                            .path
                            .parent()
                            .unwrap()
                            .join(STUDIO_DISABLED_DIR_NAME),
                    )
                    .unwrap();
                }
                let mut registry: serde_json::Value =
                    serde_json::from_slice(&fs::read(&fixture.registry).unwrap()).unwrap();
                let mut sibling: CopyDeploymentRecord = serde_json::from_value(
                    registry["copies"][&fixture.request.deployment_id].clone(),
                )
                .unwrap();
                sibling.scope = if project {
                    InstallScope::Global
                } else {
                    InstallScope::Project
                };
                sibling.project_path =
                    (!project).then(|| fixture.scope.projects[0].to_string_lossy().into_owned());
                let sibling_root = if project {
                    &fixture.scope.home
                } else {
                    &fixture.scope.projects[0]
                };
                sibling.path = sibling_root.join(".codex/skills/sample");
                sibling.deployment_id = crate::skill_deployment::deployment_id(
                    &sibling.name,
                    if project { "global" } else { "project" },
                    sibling.destination,
                    &sibling.slot,
                    sibling.project_path.as_deref(),
                    &sibling.path,
                );
                fs::create_dir_all(&sibling.path).unwrap();
                let sibling_document = fs::read(fixture.path.join("SKILL.md")).unwrap();
                fs::write(sibling.path.join("SKILL.md"), &sibling_document).unwrap();
                registry["copies"][&sibling.deployment_id] =
                    serde_json::to_value(&sibling).unwrap();
                registry["copies"][&sibling.deployment_id]["future"] =
                    serde_json::json!({"sibling": true});
                registry["future"] = serde_json::json!({"keep": true});
                registry["copies"][&fixture.request.deployment_id]["future"] =
                    serde_json::json!([1, 2]);
                fs::write(
                    &fixture.registry,
                    serde_json::to_vec_pretty(&registry).unwrap(),
                )
                .unwrap();
                let store = EventStore::open(&fixture.scope.home.join("history")).unwrap();
                let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                let limits = BackupCopyLimits {
                    max_bytes: 1024 * 1024,
                    max_entries: 100,
                    max_depth: 10,
                };
                let outcome = set_copy_visibility(
                    &mut service,
                    &store,
                    &fixture.request,
                    limits,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
                let CopyVisibilityOutcome::Changed { deployment_id, .. } = outcome else {
                    panic!("expected move")
                };
                assert!(!fixture.path.exists());
                let after: serde_json::Value =
                    serde_json::from_slice(&fs::read(&fixture.registry).unwrap()).unwrap();
                assert_eq!(
                    after["copies"][&sibling.deployment_id],
                    registry["copies"][&sibling.deployment_id]
                );
                assert_eq!(
                    fs::read(sibling.path.join("SKILL.md")).unwrap(),
                    sibling_document
                );
                let record: CopyDeploymentRecord =
                    serde_json::from_value(after["copies"][&deployment_id].clone()).unwrap();
                assert!(record.disabled);
                assert!(record.path.join("SKILL.md").is_file());
                let request = CopyVisibilityRequest {
                    deployment_id,
                    expected_owner_revision: RegistryOwnerRecord::Copy(&record).revision().unwrap(),
                    enabled: true,
                };
                set_copy_visibility(
                    &mut service,
                    &store,
                    &request,
                    limits,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
                assert!(fixture.path.join("SKILL.md").is_file());
                assert_eq!(
                    fs::read(sibling.path.join("SKILL.md")).unwrap(),
                    sibling_document
                );
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(
                        &fs::read(&fixture.registry).unwrap()
                    )
                    .unwrap(),
                    registry
                );
                let events = store.list(10, None).unwrap();
                assert_eq!(events.len(), 2);
                assert!(events.iter().all(|row| row.status == "done"
                    && row.kind == "move_copy_deployment"
                    && !row.restorable));
            }
        }
    }

    #[test]
    fn restart_settles_each_copy_move_phase_and_refuses_content_conflicts() {
        for interrupted in [false, true] {
            for project in [false, true] {
                for phase in ["intent", "parent", "moved", "published", "conflict"] {
                    let fixture = Fixture::new(project, false);
                    let state = fixture.scope.home.join("history");
                    let store = EventStore::open(&state).unwrap();
                    let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                    let limits = BackupCopyLimits {
                        max_bytes: 1024 * 1024,
                        max_entries: 100,
                        max_depth: 10,
                    };
                    let CopyVisibilityPreparation::Ready(prepared) = service
                        .prepare_copy_visibility(
                            &fixture.request,
                            std::slice::from_ref(&state),
                            limits,
                            Some(Duration::from_secs(10)),
                            CancellationToken::default(),
                        )
                        .unwrap()
                    else {
                        panic!("expected move")
                    };
                    let intent = prepared.intent.clone();
                    GuardedEventStore::bind(&store, &prepared.lease)
                        .unwrap()
                        .record_pending(
                            &prepared.lease,
                            "restart-fixture",
                            intent.event_draft().unwrap(),
                        )
                        .unwrap();
                    drop(prepared);
                    if phase != "intent" {
                        fs::create_dir(intent.transition().after().path.parent().unwrap()).unwrap();
                    }
                    if matches!(phase, "moved" | "published" | "conflict") {
                        fs::rename(&fixture.path, &intent.transition().after().path).unwrap();
                    }
                    if phase == "published" {
                        fs::write(
                            &fixture.registry,
                            intent
                                .transition()
                                .apply_document(&fs::read(&fixture.registry).unwrap())
                                .unwrap(),
                        )
                        .unwrap();
                    }
                    if phase == "conflict" {
                        fs::write(
                            intent.transition().after().path.join("SKILL.md"),
                            "external edit",
                        )
                        .unwrap();
                    }
                    let registry_before = fs::read(&fixture.registry).unwrap();
                    drop(service);
                    drop(store);
                    let store = EventStore::open(&state).unwrap();
                    let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                    if interrupted {
                        store.conn.execute("UPDATE events SET status = 'interrupted' WHERE id = 'restart-fixture'", []).unwrap();
                    }
                    let row = store.get("restart-fixture").unwrap().unwrap();
                    let result = recover_copy_visibility(
                        &mut service,
                        &store,
                        &row,
                        limits,
                        Some(Duration::from_secs(10)),
                    );
                    if phase == "conflict" {
                        assert!(result.is_err());
                        assert_eq!(
                            store.get(&row.id).unwrap().unwrap().status,
                            if interrupted {
                                "interrupted"
                            } else {
                                "pending"
                            }
                        );
                        assert_eq!(fs::read(&fixture.registry).unwrap(), registry_before);
                        assert_eq!(
                            fs::read(intent.transition().after().path.join("SKILL.md")).unwrap(),
                            b"external edit"
                        );
                    } else {
                        let applied = matches!(phase, "moved" | "published");
                        assert_eq!(result.unwrap(), applied, "{phase}");
                        assert_eq!(
                            store.get(&row.id).unwrap().unwrap().status,
                            if applied { "done" } else { "failed" }
                        );
                        assert_eq!(fixture.path.exists(), !applied);
                        assert_eq!(intent.transition().after().path.exists(), applied);
                    }
                }
            }
        }
    }

    #[test]
    fn prepares_global_and_project_enable_disable_without_effects() {
        for project in [false, true] {
            for disabled in [false, true] {
                for existing_parent in [false, true] {
                    let fixture = Fixture::new(project, disabled);
                    if !disabled && existing_parent {
                        fs::create_dir(
                            fixture
                                .path
                                .parent()
                                .unwrap()
                                .join(STUDIO_DISABLED_DIR_NAME),
                        )
                        .unwrap();
                    }
                    let original = fs::read(&fixture.registry).unwrap();
                    let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                    let CopyVisibilityPreparation::Ready(prepared) =
                        fixture.prepare(&mut service).unwrap()
                    else {
                        panic!("expected change")
                    };
                    assert_eq!(
                        prepared.needs_holding_directory(),
                        !disabled && !existing_parent
                    );
                    assert!(prepared.intent().expected_tree().starts_with("tree-v1:"));
                    assert_eq!(prepared.intent().transition().after().disabled, !disabled);
                    prepared.revalidate().unwrap();
                    assert!(fixture.path.join("SKILL.md").is_file());
                    assert!(!prepared.intent().transition().after().path.exists());
                    assert_eq!(fs::read(&fixture.registry).unwrap(), original);
                }
            }
        }
    }

    #[test]
    fn unchanged_request_still_validates_owner_and_content() {
        let mut fixture = Fixture::new(false, false);
        fixture.request.enabled = true;
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        assert!(matches!(
            fixture.prepare(&mut service).unwrap(),
            CopyVisibilityPreparation::Unchanged { .. }
        ));
        fixture.request.expected_owner_revision.push_str("stale");
        assert!(fixture.prepare(&mut service).is_err());
    }

    #[test]
    fn refuses_stale_content_and_occupied_or_linked_destination() {
        for mode in ["content", "occupied", "link"] {
            let fixture = Fixture::new(false, false);
            let original = fs::read(&fixture.registry).unwrap();
            let holding = fixture
                .path
                .parent()
                .unwrap()
                .join(STUDIO_DISABLED_DIR_NAME);
            match mode {
                "content" => fs::write(fixture.path.join("SKILL.md"), "changed").unwrap(),
                "occupied" => {
                    fs::create_dir_all(holding.join("sample")).unwrap();
                }
                _ => symlink("missing", &holding).unwrap(),
            }
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            assert!(fixture.prepare(&mut service).is_err(), "{mode}");
            assert!(fixture.path.join("SKILL.md").exists());
            assert_eq!(fs::read(&fixture.registry).unwrap(), original);
        }
    }

    #[test]
    fn retained_preparation_refuses_parent_content_and_registry_drift() {
        for mode in ["parent", "content", "registry"] {
            let fixture = Fixture::new(true, false);
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            let CopyVisibilityPreparation::Ready(prepared) = fixture.prepare(&mut service).unwrap()
            else {
                panic!("expected change")
            };
            match mode {
                "parent" => fs::create_dir(
                    fixture
                        .path
                        .parent()
                        .unwrap()
                        .join(STUDIO_DISABLED_DIR_NAME),
                )
                .unwrap(),
                "content" => fs::write(fixture.path.join("resource.txt"), "external").unwrap(),
                _ => fs::write(&fixture.registry, "{}").unwrap(),
            }
            assert!(prepared.revalidate().is_err(), "{mode}");
            assert!(fixture.path.join("SKILL.md").exists());
        }
    }
}
