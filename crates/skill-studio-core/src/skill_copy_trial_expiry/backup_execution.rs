use super::*;
use crate::{
    skill_backup_copy::{copy_entry, inspect_entry, BackupCopyLimits},
    skill_backup_source::{BackupSource, BackupSourceRoot},
    skill_coordination::{
        CancellationToken, CoordinationMode, CoordinationPlan, DirectoryEffect, FinalizedWriteLease,
    },
    skill_copy_removal::{admit_copy_deployment, AdmittedCopy, CopyRemovalRequest},
    skill_event::{EventRow, EventStatus},
    skill_event_operations::{EventWriteFailure, GuardedEventStore},
    skill_event_store::EventStore,
    skill_fork_registry::RegistryOwnerRecord,
    skill_scope::SkillReadScope,
    skill_service::ScopedSkillService,
};
use cap_std::fs::MetadataExt;
use std::{io, time::Duration};

#[derive(Debug, Clone)]
pub struct CopyTrialExpiryRequest {
    pub deployment_id: String,
    pub expected_owner_revision: String,
}

/// Backup preparation is pending work, not a completed expiry. Source, readers
/// and ownership remain live until the forward executor publishes their removal.
#[derive(Debug)]
pub struct PendingCopyTrialExpiry {
    pub event_id: String,
    pub verified_backup: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyTrialExpiryReceipt {
    pub event_id: String,
    pub skill_name: String,
    pub visible_trash: PathBuf,
}

#[derive(Debug)]
pub struct CopyTrialExpiryError {
    pub event_id: Option<String>,
    pub recovery_required: bool,
    pub message: String,
}

struct PreparedBackup<'scope> {
    admitted: AdmittedCopy,
    lease: FinalizedWriteLease<'scope>,
}

fn prepare_source<'scope>(
    service: &'scope mut ScopedSkillService,
    store: &EventStore,
    request: &CopyRemovalRequest,
    entries: &[PathBuf],
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<PreparedBackup<'scope>, String> {
    let parsed =
        parse_deployment_id(&request.deployment_id).ok_or("Invalid Copy trial deployment ID")?;
    let scope = service.scope();
    if !crate::skill_agents::skill_roots(&scope.home, &scope.projects)
        .iter()
        .any(|root| {
            root.label != "parked"
                && (parsed.lexical_path == root.path.join(&parsed.name)
                    || parsed.lexical_path
                        == root.path.join(".skill-studio-disabled").join(&parsed.name))
        })
    {
        return Err("Copy trial source is outside configured skill roots".into());
    }
    let mut trees = vec![
        store.app_data.clone(),
        parsed.lexical_path.clone(),
        scope.home.join(".agents"),
    ];
    trees.extend(entries.iter().filter_map(|entry| {
        entry
            .parent()
            .filter(|parent| parent.exists())
            .map(Path::to_path_buf)
    }));
    let (inventory, lease) = service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([parsed.name])),
            &trees,
            entries,
            timeout,
            cancellation.clone(),
        )
        .map_err(|error| error.to_string())?;
    let admitted = admit_copy_deployment(&inventory, &lease, request, limits, cancellation)?;
    Ok(PreparedBackup { admitted, lease })
}

fn require_pending(
    store: &EventStore,
    lease: &FinalizedWriteLease<'_>,
    intent: &CopyTrialExpiryIntent,
) -> Result<(), String> {
    let row = GuardedEventStore::bind(store, lease)?
        .next_recovery_event(lease)?
        .ok_or("Copy trial expiry is no longer pending")?;
    if CopyTrialExpiryIntent::from_event(&row)? != *intent {
        return Err("Copy trial expiry event changed or is out of order".into());
    }
    Ok(())
}

fn backup_entries(paths: &CopyTrialExpiryPaths) -> Vec<PathBuf> {
    let mut entries = vec![
        paths.source.clone(),
        paths
            .hidden_trash
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf(),
        paths.hidden_trash.parent().unwrap().to_path_buf(),
        paths.hidden_trash.clone(),
        paths.source_quarantine.clone(),
        paths.source_quarantine.parent().unwrap().to_path_buf(),
        paths.visible_trash.clone(),
        paths.visible_trash.parent().unwrap().to_path_buf(),
    ];
    for stage in &paths.reader_stages {
        entries.push(stage.clone());
        entries.push(stage.parent().unwrap().to_path_buf());
    }
    entries
}

fn prepare_pending<'scope>(
    service: &'scope mut ScopedSkillService,
    store: &EventStore,
    intent: &CopyTrialExpiryIntent,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<PreparedBackup<'scope>, String> {
    let paths = intent.paths_for_scope(&service.scope())?;
    let request = CopyRemovalRequest {
        deployment_id: intent.selection.selected.deployment_id.clone(),
        expected_owner_revision: RegistryOwnerRecord::Copy(&intent.selection.selected)
            .revision()
            .ok_or("Copy revision is unavailable")?,
    };
    let mut entries = backup_entries(&paths);
    entries.extend(
        intent
            .selection
            .readers
            .iter()
            .map(|reader| reader.path.clone()),
    );
    let prepared = prepare_source(
        service,
        store,
        &request,
        &entries,
        limits,
        timeout,
        cancellation,
    )?;
    require_pending(store, &prepared.lease, intent)?;
    if prepared.admitted.tree.tree_identity != intent.expected_tree
        || prepared.admitted.tree.fingerprint != intent.selection.trial.deployment_fingerprint
        || prepared.admitted.reader_descriptions != intent.selection.readers
    {
        return Err("Copy trial source or readers changed before backup".into());
    }
    let proposed = intent
        .selection
        .without_selected_records(&prepared.admitted.registry)?;
    if proposed == prepared.admitted.registry {
        return Err("Copy trial was already published; backup preparation cannot resume".into());
    }
    Ok(prepared)
}

fn select_relative_entry(root: &Path, path: &Path) -> Result<BackupSource, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "Copy trial path escaped its root")?;
    BackupSourceRoot::bind(root)
        .and_then(|root| root.select_relative(relative))
        .map_err(|error| error.to_string())
}

fn select_backup_entry(home: &Path, path: &Path) -> Result<BackupSource, String> {
    select_relative_entry(&home.join(".agents"), path)
}

fn trash_container(home: &Path, path: &Path) -> Result<Option<BackupSource>, String> {
    let trash_root = path
        .parent()
        .ok_or("Copy trial Trash container has no parent")?;
    let trash = select_backup_entry(home, trash_root)?;
    match trash.directory.symlink_metadata(&trash.name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            select_backup_entry(home, path).map(Some)
        }
        Ok(_) => Err("Copy trial Trash root is not an independent directory".into()),
    }
}

fn private_artifact(root: &Path, path: &Path) -> Result<BackupSource, String> {
    let parent = select_relative_entry(
        root,
        path.parent().ok_or("Copy trial artifact has no parent")?,
    )?;
    let device = parent
        .private_directory_device()
        .map_err(|error| error.to_string())?
        .ok_or("Copy trial private artifact parent is missing")?;
    if device
        != parent
            .directory
            .dir_metadata()
            .map_err(|error| error.to_string())?
            .dev()
    {
        return Err("Copy trial private artifact parent changed filesystem".into());
    }
    let artifact = select_relative_entry(root, path)?;
    validate_backup_parent(&artifact)?;
    Ok(artifact)
}

fn validate_backup_parent(backup: &BackupSource) -> Result<(), String> {
    backup.revalidate().map_err(|error| error.to_string())?;
    let metadata = backup
        .directory
        .dir_metadata()
        .map_err(|error| error.to_string())?;
    if metadata.mode() & 0o777 != 0o700 {
        return Err("Expected private directory permissions".into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Checkpoint {
    PendingRecorded,
    TrashCreated,
    StageCreated,
    BackupCopied,
}

#[allow(clippy::too_many_arguments)]
fn begin_with_checkpoint(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &CopyTrialExpiryRequest,
    now: DateTime<Utc>,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
    mut checkpoint: impl FnMut(Checkpoint) -> Result<(), String>,
) -> Result<PendingCopyTrialExpiry, CopyTrialExpiryError> {
    let before = |message| CopyTrialExpiryError {
        event_id: None,
        recovery_required: false,
        message,
    };
    let id = crate::skill_event_store::allocate_id();
    let scope = service.scope();
    let removal_request = CopyRemovalRequest {
        deployment_id: request.deployment_id.clone(),
        expected_owner_revision: request.expected_owner_revision.clone(),
    };
    let prepared = prepare_source(
        service,
        store,
        &removal_request,
        &[],
        limits,
        timeout,
        cancellation.clone(),
    )
    .map_err(before)?;
    let selection = select_due_copy_trial(
        &prepared.admitted.registry,
        &prepared.admitted.selected,
        &prepared.admitted.reader_descriptions,
        &prepared.admitted.tree.fingerprint,
        now,
    )
    .map_err(before)?;
    let intent = CopyTrialExpiryIntent::new(
        &scope,
        id.clone(),
        selection,
        prepared.admitted.tree.tree_identity.clone(),
        now,
    )
    .map_err(before)?;
    let paths = intent.paths_for_scope(&scope).map_err(before)?;
    GuardedEventStore::bind(store, &prepared.lease)
        .map_err(before)?
        .record_pending(&prepared.lease, &id, intent.event_draft().map_err(before)?)
        .map_err(|error| CopyTrialExpiryError {
            event_id: matches!(error, EventWriteFailure::MayHaveWritten(_)).then(|| id.clone()),
            recovery_required: matches!(error, EventWriteFailure::MayHaveWritten(_)),
            message: error.to_string(),
        })?;
    drop(prepared);
    let after = |message| CopyTrialExpiryError {
        event_id: Some(id.clone()),
        recovery_required: true,
        message,
    };
    checkpoint(Checkpoint::PendingRecorded).map_err(after)?;
    let mut backup = None;
    for (directory, private, phase) in [
        (
            paths.hidden_trash.parent().unwrap().parent().unwrap(),
            false,
            Checkpoint::TrashCreated,
        ),
        (
            paths.hidden_trash.parent().unwrap(),
            true,
            Checkpoint::StageCreated,
        ),
    ] {
        let prepared = prepare_pending(
            service,
            store,
            &intent,
            limits,
            timeout,
            cancellation.clone(),
        )
        .map_err(after)?;
        let entry = select_backup_entry(&scope.home, directory).map_err(after)?;
        let created = match entry.directory.symlink_metadata(&entry.name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                prepared
                    .lease
                    .validate_entry_move(directory, directory)
                    .map_err(after)?;
                entry
                    .create_directory()
                    .map_err(|error| after(error.to_string()))?;
                true
            }
            Err(error) => return Err(after(error.to_string())),
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                if private {
                    entry
                        .private_directory_device()
                        .map_err(|error| after(error.to_string()))?;
                }
                false
            }
            Ok(_) => {
                return Err(after(
                    "Trial backup container is not an independent directory".into(),
                ))
            }
        };
        if private {
            let destination =
                select_backup_entry(&scope.home, &paths.hidden_trash).map_err(after)?;
            validate_backup_parent(&destination).map_err(after)?;
            backup = Some(destination);
        }
        drop(prepared);
        if created {
            checkpoint(phase).map_err(after)?;
        }
    }
    let prepared = prepare_pending(
        service,
        store,
        &intent,
        limits,
        timeout,
        cancellation.clone(),
    )
    .map_err(after)?;
    let destination =
        backup.ok_or_else(|| after("Trial backup staging directory is missing".into()))?;
    validate_backup_parent(&destination).map_err(after)?;
    prepared
        .lease
        .validate_state_tree(&paths.source)
        .map_err(after)?;
    prepared
        .lease
        .validate_entry_move(&paths.hidden_trash, &paths.hidden_trash)
        .map_err(after)?;
    let copied = copy_entry(
        &prepared.admitted.source.directory,
        &prepared.admitted.source.name,
        &destination.directory,
        &destination.name,
        limits,
        &cancellation,
    )
    .map_err(|error| after(error.to_string()))?;
    if copied.tree_identity != intent.expected_tree
        || copied.fingerprint != intent.selection.trial.deployment_fingerprint
    {
        return Err(after(
            "Copy trial backup differs from admitted content".into(),
        ));
    }
    validate_backup_parent(&destination).map_err(after)?;
    drop(prepared);
    checkpoint(Checkpoint::BackupCopied).map_err(after)?;
    let prepared = prepare_pending(
        service,
        store,
        &intent,
        limits,
        timeout,
        cancellation.clone(),
    )
    .map_err(after)?;
    validate_backup_parent(&destination).map_err(after)?;
    let verified = inspect_entry(
        &destination.directory,
        &destination.name,
        limits,
        &cancellation,
    )
    .map_err(|error| after(error.to_string()))?;
    if verified.tree_identity != intent.expected_tree
        || verified.fingerprint != intent.selection.trial.deployment_fingerprint
    {
        return Err(after("Copy trial backup changed after copying".into()));
    }
    validate_backup_parent(&destination).map_err(after)?;
    require_pending(store, &prepared.lease, &intent).map_err(after)?;
    Ok(PendingCopyTrialExpiry {
        event_id: id,
        verified_backup: paths.hidden_trash,
    })
}

/// Records the intent and verifies a hidden Trash backup. This deliberately
/// leaves a pending event; it does not remove the trial or complete its history.
pub fn begin_copy_trial_expiry(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &CopyTrialExpiryRequest,
    now: DateTime<Utc>,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<PendingCopyTrialExpiry, CopyTrialExpiryError> {
    begin_with_checkpoint(
        service,
        store,
        request,
        now,
        limits,
        timeout,
        cancellation,
        |_| Ok(()),
    )
}

fn source_at(path: &Path) -> Result<BackupSource, String> {
    BackupSourceRoot::bind(path.parent().ok_or("Copy trial path has no parent")?)
        .and_then(|root| {
            root.select(
                path.file_name()
                    .ok_or_else(|| io::Error::other("Copy trial path has no name"))?,
            )
        })
        .map_err(|error| error.to_string())
}

fn inspect_tree(
    source: &BackupSource,
    limits: BackupCopyLimits,
    cancellation: &CancellationToken,
) -> Result<Option<String>, String> {
    match source.directory.symlink_metadata(&source.name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            inspect_entry(&source.directory, &source.name, limits, cancellation)
                .map(|report| Some(report.tree_identity))
                .map_err(|error| error.to_string())
        }
        Ok(_) => Err("Copy trial path is not an independent directory".into()),
    }
}

fn tree_move_message(error: crate::skill_tree_move::TreeMoveFailure) -> String {
    match error {
        crate::skill_tree_move::TreeMoveFailure::BeforeMove(message)
        | crate::skill_tree_move::TreeMoveFailure::MayHaveMoved(message) => message,
    }
}

struct PreparedEffects<'scope> {
    registry: Vec<u8>,
    source: BackupSource,
    quarantine_parent: BackupSource,
    quarantine: Option<BackupSource>,
    readers: Vec<BackupSource>,
    stage_parents: Vec<BackupSource>,
    reader_stages: Vec<Option<BackupSource>>,
    hidden_backup: Option<BackupSource>,
    visible_container: Option<BackupSource>,
    lease: FinalizedWriteLease<'scope>,
}

fn private_child(parent: &BackupSource, path: &Path) -> Result<Option<BackupSource>, String> {
    let parent_device = parent
        .directory
        .dir_metadata()
        .map_err(|error| error.to_string())?
        .dev();
    match parent
        .private_directory_device()
        .map_err(|error| error.to_string())?
    {
        Some(device) if device == parent_device => BackupSourceRoot::bind(&parent.original_path)
            .and_then(|root| {
                root.select(
                    path.file_name()
                        .ok_or_else(|| io::Error::other("Copy trial effect has no name"))?,
                )
            })
            .map(Some)
            .map_err(|error| error.to_string()),
        Some(_) => Err("Copy trial staging directory is on a different filesystem".into()),
        None => Ok(None),
    }
}

fn active_tree<'path>(
    candidates: impl IntoIterator<Item = &'path Path>,
    fallback: &Path,
) -> Result<PathBuf, String> {
    for path in candidates {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                return Ok(path.to_path_buf());
            }
            Ok(_) => return Err("Copy trial active tree is not an independent directory".into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(fallback.to_path_buf())
}

fn prepare_effects<'scope>(
    service: &'scope mut ScopedSkillService,
    store: &EventStore,
    intent: &CopyTrialExpiryIntent,
    timeout: Option<Duration>,
) -> Result<PreparedEffects<'scope>, String> {
    let paths = intent.paths_for_scope(&service.scope())?;
    let source_root = paths
        .source_quarantine
        .parent()
        .and_then(Path::parent)
        .ok_or("Copy trial quarantine root is missing")?;
    let trash_root = paths
        .hidden_trash
        .parent()
        .and_then(Path::parent)
        .ok_or("Copy trial Trash root is missing")?;
    let active_source = active_tree(
        [&paths.source_quarantine, &paths.source].map(PathBuf::as_path),
        source_root,
    )?;
    let active_trash = active_tree(
        [
            paths.hidden_trash.parent().unwrap(),
            paths.visible_trash.parent().unwrap(),
            trash_root,
        ],
        trash_root
            .parent()
            .ok_or("Copy trial Trash root parent is missing")?,
    )?;
    let mut trees = vec![
        store.app_data.clone(),
        active_source,
        source_root.to_path_buf(),
        active_trash,
    ];
    trees.extend(intent.selection.readers.iter().map(|reader| {
        reader
            .path
            .parent()
            .expect("validated Copy reader has a parent")
            .to_path_buf()
    }));
    trees.extend(paths.reader_stages.iter().map(|stage| {
        stage
            .parent()
            .and_then(Path::parent)
            .expect("validated Copy reader stage has a root")
            .to_path_buf()
    }));
    let mut entries = backup_entries(&paths);
    entries.push(paths.registry.clone());
    entries.extend(
        intent
            .selection
            .readers
            .iter()
            .map(|reader| reader.path.clone()),
    );
    let (_, lease) = service
        .prepare_write_inventory_with_entries(
            Some(&BTreeSet::from([intent.selection.selected.name.clone()])),
            &trees,
            &entries,
            timeout,
            CancellationToken::default(),
        )
        .map_err(|error| error.to_string())?;
    require_pending(store, &lease, intent)?;
    let registry = lease
        .read(&paths.registry, 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    let source = source_at(&paths.source)?;
    let quarantine_parent = source_at(
        paths
            .source_quarantine
            .parent()
            .ok_or("Copy trial quarantine parent is missing")?,
    )?;
    let quarantine = private_child(&quarantine_parent, &paths.source_quarantine)?;
    let readers = intent
        .selection
        .readers
        .iter()
        .map(|reader| source_at(&reader.path))
        .collect::<Result<Vec<_>, _>>()?;
    let stage_parents = paths
        .reader_stages
        .iter()
        .map(|stage| {
            source_at(
                stage
                    .parent()
                    .ok_or("Copy trial reader stage parent is missing")?,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let reader_stages = stage_parents
        .iter()
        .zip(&paths.reader_stages)
        .map(|(parent, stage)| private_child(parent, stage))
        .collect::<Result<Vec<_>, _>>()?;
    let hidden_container = trash_container(
        &intent.home,
        paths
            .hidden_trash
            .parent()
            .ok_or("Copy trial hidden container is missing")?,
    )?;
    let hidden_backup = hidden_container
        .as_ref()
        .map(|container| private_child(container, &paths.hidden_trash))
        .transpose()?
        .flatten();
    let visible_container = trash_container(
        &intent.home,
        paths
            .visible_trash
            .parent()
            .ok_or("Copy trial visible container is missing")?,
    )?;
    Ok(PreparedEffects {
        registry,
        source,
        quarantine_parent,
        quarantine,
        readers,
        stage_parents,
        reader_stages,
        hidden_backup,
        visible_container,
        lease,
    })
}

fn receipt(intent: &CopyTrialExpiryIntent, paths: &CopyTrialExpiryPaths) -> CopyTrialExpiryReceipt {
    CopyTrialExpiryReceipt {
        event_id: intent.event_id.clone(),
        skill_name: intent.selection.selected.name.clone(),
        visible_trash: paths.visible_trash.clone(),
    }
}

fn create_private_effect(
    lease: FinalizedWriteLease<'_>,
    directory: BackupSource,
) -> Result<(), String> {
    lease.validate_entry_move(&directory.original_path, &directory.original_path)?;
    directory
        .create_directory()
        .map_err(|error| error.to_string())
}

fn registry_is_published(
    service: &ScopedSkillService,
    store: &EventStore,
    intent: &CopyTrialExpiryIntent,
    timeout: Option<Duration>,
) -> Result<bool, String> {
    let paths = intent.paths_for_scope(&service.scope())?;
    let registry_parent = paths
        .registry
        .parent()
        .ok_or("Copy trial registry parent is missing")?;
    let read_scope = SkillReadScope::bind(&[store.app_data.clone(), registry_parent.to_path_buf()])
        .map_err(|error| error.to_string())?;
    let lease = CoordinationPlan::new(
        vec![
            DirectoryEffect::tree(&store.app_data, CoordinationMode::Exclusive),
            DirectoryEffect::entry(&paths.registry, CoordinationMode::Exclusive),
        ],
        timeout,
    )
    .map_err(|error| error.to_string())?
    .acquire()
    .map_err(|error| error.to_string())?
    .finalize_write(&read_scope, std::slice::from_ref(&paths.registry))
    .map_err(|error| error.to_string())?;
    require_pending(store, &lease, intent)?;
    let registry = lease
        .read(&paths.registry, 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    Ok(intent.selection.without_selected_records(&registry)? == registry)
}

fn with_published_lease<T>(
    service: &ScopedSkillService,
    store: &EventStore,
    intent: &CopyTrialExpiryIntent,
    timeout: Option<Duration>,
    action: impl FnOnce(FinalizedWriteLease<'_>, &CopyTrialExpiryPaths) -> Result<T, String>,
) -> Result<T, String> {
    let paths = intent.paths_for_scope(&service.scope())?;
    let quarantine = paths.source_quarantine.clone();
    let trash_root = paths
        .hidden_trash
        .parent()
        .and_then(Path::parent)
        .ok_or("Copy trial Trash root is missing")?;
    let active_trash = active_tree(
        [
            paths.hidden_trash.parent().unwrap(),
            paths.visible_trash.parent().unwrap(),
            trash_root,
        ],
        trash_root
            .parent()
            .ok_or("Copy trial Trash root parent is missing")?,
    )?;
    let registry_parent = paths
        .registry
        .parent()
        .ok_or("Copy trial registry parent is missing")?;
    let trees = vec![
        store.app_data.clone(),
        quarantine,
        active_trash,
        registry_parent.to_path_buf(),
    ];
    let entries = vec![
        paths.registry.clone(),
        paths.hidden_trash.parent().unwrap().to_path_buf(),
        paths.visible_trash.parent().unwrap().to_path_buf(),
    ];
    let read_scope = SkillReadScope::bind(&trees).map_err(|error| error.to_string())?;
    let effects = trees
        .into_iter()
        .map(|path| DirectoryEffect::tree(path, CoordinationMode::Exclusive))
        .chain(
            entries
                .into_iter()
                .map(|path| DirectoryEffect::entry(path, CoordinationMode::Exclusive)),
        )
        .collect();
    let lease = CoordinationPlan::new(effects, timeout)
        .map_err(|error| error.to_string())?
        .acquire()
        .map_err(|error| error.to_string())?
        .finalize_write(&read_scope, std::slice::from_ref(&paths.registry))
        .map_err(|error| error.to_string())?;
    require_pending(store, &lease, intent)?;
    let registry = lease
        .read(&paths.registry, 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    if intent.selection.without_selected_records(&registry)? != registry {
        return Err("Copy trial registry is not published".into());
    }
    action(lease, &paths)
}

fn publish_visible(
    service: &mut ScopedSkillService,
    store: &EventStore,
    intent: &CopyTrialExpiryIntent,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: &CancellationToken,
) -> Result<bool, String> {
    with_published_lease(service, store, intent, timeout, |lease, paths| {
        let hidden_container = trash_container(&intent.home, paths.hidden_trash.parent().unwrap())?;
        let visible_container =
            trash_container(&intent.home, paths.visible_trash.parent().unwrap())?;
        let hidden_backup = hidden_container
            .as_ref()
            .map(|container| private_child(container, &paths.hidden_trash))
            .transpose()?
            .flatten();
        let visible_backup = visible_container
            .as_ref()
            .map(|container| private_child(container, &paths.visible_trash))
            .transpose()?
            .flatten();
        match (&hidden_backup, &visible_backup) {
            (Some(hidden), None) => {
                if inspect_tree(hidden, limits, cancellation)?.as_deref()
                    != Some(&intent.expected_tree)
                {
                    return Err("Copy trial hidden backup changed".into());
                }
                let hidden_container = hidden_container.as_ref().unwrap();
                let visible_container = visible_container
                    .as_ref()
                    .ok_or("Copy trial visible Trash parent is missing")?;
                let container_tree = inspect_tree(hidden_container, limits, cancellation)?
                    .ok_or("Copy trial hidden backup container is missing")?;
                lease.validate_entry_move(
                    &hidden_container.original_path,
                    &visible_container.original_path,
                )?;
                hidden_container
                    .move_verified_tree(
                        visible_container,
                        &container_tree,
                        lease,
                        limits,
                        cancellation,
                    )
                    .map_err(tree_move_message)?;
                Ok(true)
            }
            (None, Some(visible))
                if inspect_tree(visible, limits, cancellation)?.as_deref()
                    == Some(&intent.expected_tree) =>
            {
                Ok(false)
            }
            _ => Err("Copy trial Trash publication is missing, changed or ambiguous".into()),
        }
    })
}

fn finish_done(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    intent: &CopyTrialExpiryIntent,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<(), String> {
    with_published_lease(service, store, intent, timeout, |lease, paths| {
        let visible = private_artifact(&intent.home.join(".agents"), &paths.visible_trash)?;
        if inspect_tree(&visible, limits, &CancellationToken::default())?.as_deref()
            != Some(&intent.expected_tree)
        {
            return Err("Visible Copy trial Trash backup changed before completion".into());
        }
        let quarantine_root = paths
            .source_quarantine
            .parent()
            .and_then(Path::parent)
            .ok_or("Copy trial quarantine root is missing")?;
        let quarantine = private_artifact(quarantine_root, &paths.source_quarantine)?;
        if inspect_tree(&quarantine, limits, &CancellationToken::default())?.as_deref()
            != Some(&intent.expected_tree)
        {
            return Err("Copy trial source quarantine changed before completion".into());
        }
        validate_backup_parent(&visible)?;
        validate_backup_parent(&quarantine)?;
        require_pending(store, &lease, intent)?;
        GuardedEventStore::bind(store, &lease)?
            .finish_recovery_snapshot(&lease, row, EventStatus::Done, None)
            .map_err(|error| error.to_string())
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForwardCheckpoint {
    SourceStageCreated,
    ReaderStageCreated,
    ReaderMoved,
    SourceMoved,
    RegistryPublished,
    TrashPublished,
}

#[allow(clippy::too_many_arguments)]
fn execute_forward_with_checkpoint(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    intent: &CopyTrialExpiryIntent,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
    mut checkpoint: impl FnMut(ForwardCheckpoint) -> Result<(), String>,
) -> Result<CopyTrialExpiryReceipt, String> {
    let paths = intent.paths_for_scope(&service.scope())?;
    let mut prepared = prepare_effects(service, store, intent, timeout)?;
    if prepared.quarantine.is_none() {
        create_private_effect(prepared.lease, prepared.quarantine_parent)?;
        checkpoint(ForwardCheckpoint::SourceStageCreated)?;
        prepared = prepare_effects(service, store, intent, timeout)?;
    }
    for index in 0..intent.selection.readers.len() {
        if prepared.reader_stages[index].is_none() {
            let parent = prepared.stage_parents.swap_remove(index);
            create_private_effect(prepared.lease, parent)?;
            checkpoint(ForwardCheckpoint::ReaderStageCreated)?;
            prepared = prepare_effects(service, store, intent, timeout)?;
        }
        if cancellation.is_cancelled() {
            return Err("Copy trial expiry cancelled".into());
        }
        let reader = &intent.selection.readers[index];
        let stage = prepared.reader_stages[index].as_ref().unwrap();
        match (
            prepared.readers[index]
                .exact_symlink_target()
                .map_err(|error| error.to_string())?,
            stage
                .exact_symlink_target()
                .map_err(|error| error.to_string())?,
        ) {
            (Some(actual), None) if actual == reader.raw_target => {
                prepared
                    .lease
                    .validate_entry_move(&reader.path, &stage.original_path)?;
                prepared.readers[index]
                    .move_exact_symlink_to(stage, &reader.raw_target)
                    .map_err(|error| error.to_string())?;
                drop(prepared);
                checkpoint(ForwardCheckpoint::ReaderMoved)?;
                prepared = prepare_effects(service, store, intent, timeout)?;
            }
            (None, Some(actual)) if actual == reader.raw_target => (),
            _ => return Err("Copy trial reader changed or is ambiguously staged".into()),
        }
    }
    let quarantine = prepared.quarantine.as_ref().unwrap();
    match (
        inspect_tree(&prepared.source, limits, &cancellation)?,
        inspect_tree(quarantine, limits, &cancellation)?,
    ) {
        (Some(tree), None) if tree == intent.expected_tree => {
            prepared
                .source
                .move_verified_tree(
                    quarantine,
                    &intent.expected_tree,
                    prepared.lease,
                    limits,
                    &cancellation,
                )
                .map_err(tree_move_message)?;
            checkpoint(ForwardCheckpoint::SourceMoved)?;
            prepared = prepare_effects(service, store, intent, timeout)?;
        }
        (None, Some(tree)) if tree == intent.expected_tree => (),
        _ => return Err("Copy trial source or quarantine changed".into()),
    }
    for (index, reader) in intent.selection.readers.iter().enumerate() {
        if prepared.readers[index]
            .exact_symlink_target()
            .map_err(|error| error.to_string())?
            .is_some()
            || prepared.reader_stages[index]
                .as_ref()
                .unwrap()
                .exact_symlink_target()
                .map_err(|error| error.to_string())?
                .as_deref()
                != Some(&reader.raw_target)
        {
            return Err("Copy trial reader staging changed before publication".into());
        }
    }
    let hidden = prepared
        .hidden_backup
        .as_ref()
        .ok_or("Copy trial hidden backup is missing")?;
    if inspect_tree(hidden, limits, &cancellation)?.as_deref() != Some(&intent.expected_tree) {
        return Err("Copy trial hidden backup changed before publication".into());
    }
    let visible_container = prepared
        .visible_container
        .as_ref()
        .ok_or("Copy trial visible Trash parent is missing")?;
    if inspect_tree(visible_container, limits, &cancellation)?.is_some() {
        return Err("Copy trial visible Trash destination is occupied".into());
    }
    let proposed = intent
        .selection
        .without_selected_records(&prepared.registry)?;
    if proposed != prepared.registry {
        let mut lease = prepared.lease;
        crate::skill_document_target::SkillRegistryTarget::bind(
            paths
                .registry
                .parent()
                .ok_or("Copy trial registry parent is missing")?,
        )?
        .replace(&mut lease, &prepared.registry, &proposed)
        .map_err(|error| error.to_string())?;
        checkpoint(ForwardCheckpoint::RegistryPublished)?;
    } else {
        drop(prepared);
    }
    if publish_visible(service, store, intent, limits, timeout, &cancellation)? {
        checkpoint(ForwardCheckpoint::TrashPublished)?;
    }
    finish_done(service, store, row, intent, limits, timeout)?;
    Ok(receipt(intent, &paths))
}

fn execute_forward(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    intent: &CopyTrialExpiryIntent,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<CopyTrialExpiryReceipt, String> {
    execute_forward_with_checkpoint(
        service,
        store,
        row,
        intent,
        limits,
        timeout,
        cancellation,
        |_| Ok(()),
    )
}

pub fn expire_copy_trial(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &CopyTrialExpiryRequest,
    now: DateTime<Utc>,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<CopyTrialExpiryReceipt, CopyTrialExpiryError> {
    let pending = begin_copy_trial_expiry(
        service,
        store,
        request,
        now,
        limits,
        timeout,
        cancellation.clone(),
    )?;
    let row = store
        .get(&pending.event_id)
        .map_err(|message| CopyTrialExpiryError {
            event_id: Some(pending.event_id.clone()),
            recovery_required: true,
            message,
        })?
        .ok_or_else(|| CopyTrialExpiryError {
            event_id: Some(pending.event_id.clone()),
            recovery_required: true,
            message: "Recorded Copy trial expiry is missing".into(),
        })?;
    let intent =
        CopyTrialExpiryIntent::from_event(&row).map_err(|message| CopyTrialExpiryError {
            event_id: Some(pending.event_id.clone()),
            recovery_required: true,
            message,
        })?;
    execute_forward(service, store, &row, &intent, limits, timeout, cancellation).map_err(
        |message| CopyTrialExpiryError {
            event_id: Some(pending.event_id),
            recovery_required: true,
            message,
        },
    )
}

pub fn recover_copy_trial_expiry(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<Option<CopyTrialExpiryReceipt>, String> {
    let intent = CopyTrialExpiryIntent::from_event(row)?;
    let paths = intent.paths_for_scope(&service.scope())?;
    let cancellation = CancellationToken::default();
    if registry_is_published(service, store, &intent, timeout)? {
        publish_visible(service, store, &intent, limits, timeout, &cancellation)?;
        finish_done(service, store, row, &intent, limits, timeout)?;
        return Ok(Some(receipt(&intent, &paths)));
    }
    let mut prepared = prepare_effects(service, store, &intent, timeout)?;
    let quarantine_tree = prepared
        .quarantine
        .as_ref()
        .map(|quarantine| inspect_tree(quarantine, limits, &cancellation))
        .transpose()?
        .flatten();
    match (
        inspect_tree(&prepared.source, limits, &cancellation)?,
        quarantine_tree,
    ) {
        (None, Some(tree)) if tree == intent.expected_tree => {
            prepared
                .quarantine
                .as_ref()
                .unwrap()
                .move_verified_tree(
                    &prepared.source,
                    &intent.expected_tree,
                    prepared.lease,
                    limits,
                    &cancellation,
                )
                .map_err(tree_move_message)?;
            prepared = prepare_effects(service, store, &intent, timeout)?;
        }
        (Some(tree), None) if tree == intent.expected_tree => (),
        _ => {
            return Err(
                "Copy trial rollback would overwrite a replacement or changed artifact".into(),
            )
        }
    }
    for (index, reader) in intent.selection.readers.iter().enumerate() {
        let original_target = prepared.readers[index]
            .exact_symlink_target()
            .map_err(|error| error.to_string())?;
        let stage_target = prepared.reader_stages[index]
            .as_ref()
            .map(|stage| {
                stage
                    .exact_symlink_target()
                    .map_err(|error| error.to_string())
            })
            .transpose()?
            .flatten();
        match (original_target, stage_target) {
            (Some(actual), None) if actual == reader.raw_target => (),
            (None, Some(actual)) if actual == reader.raw_target => {
                let stage = prepared.reader_stages[index].as_ref().unwrap();
                prepared
                    .lease
                    .validate_entry_move(&stage.original_path, &reader.path)?;
                prepared.readers[index]
                    .restore_exact_symlink_from(stage, &reader.raw_target)
                    .map_err(|error| error.to_string())?;
                drop(prepared);
                prepared = prepare_effects(service, store, &intent, timeout)?;
            }
            (None, None) => {
                prepared
                    .lease
                    .validate_entry_move(&reader.path, &reader.path)?;
                prepared.readers[index]
                    .restore_absent_symlink(&reader.raw_target)
                    .map_err(|error| error.to_string())?;
                drop(prepared);
                prepared = prepare_effects(service, store, &intent, timeout)?;
            }
            _ => return Err("Copy trial reader rollback would overwrite a replacement".into()),
        }
    }
    if intent
        .selection
        .without_selected_records(&prepared.registry)?
        == prepared.registry
    {
        return Err("Copy trial registry changed during rollback".into());
    }
    require_pending(store, &prepared.lease, &intent)?;
    GuardedEventStore::bind(store, &prepared.lease)?
        .finish_recovery_snapshot(&prepared.lease, row, EventStatus::Failed, None)
        .map_err(|error| error.to_string())?;
    Ok(None)
}

#[cfg(test)]
mod tests;
