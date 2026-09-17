//! Durable restoration of a retained Copy trial backup.

use crate::{
    skill_backup_copy::{inspect_entry, BackupCopyLimits},
    skill_backup_source::BackupSourceRoot,
    skill_coordination::{CancellationToken, CoordinationMode, CoordinationPlan, DirectoryEffect},
    skill_copy_trial_expiry::CopyTrialExpiryIntent,
    skill_event::{EventDraft, EventRow},
    skill_event_operations::{EventWriteFailure, GuardedEventStore},
    skill_event_store::EventStore,
    skill_scope::SkillReadScope,
    skill_trial_restore::{
        entry_identity, restore_trial_backup_with, valid_skill_name, RestoreCheckpoint,
        RestoreControl, RestoreEntryIdentity, TrialRestoreError,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

pub const EVENT_KIND: &str = "restore_expired_copy_trial_backup";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyTrialBackupRestoreReceipt {
    pub restore_event_id: String,
    pub source_event_id: String,
    pub skill_name: String,
    pub target: PathBuf,
}

#[derive(Debug)]
pub struct CopyTrialBackupRestoreError {
    pub event_id: Option<String>,
    pub recovery_required: bool,
    pub message: String,
}

struct RestoreExecutionControl {
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
}

impl std::fmt::Display for CopyTrialBackupRestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CopyTrialBackupRestoreError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreIntent {
    version: u32,
    event_id: String,
    source_event_id: String,
    source_revision: String,
    home: PathBuf,
    name: String,
    backup: PathBuf,
    expected_tree: String,
    phase: RestorePhase,
    stage_identity: Option<RestoreEntryIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RestorePhase {
    Recorded,
    Staged,
}

impl RestoreIntent {
    fn validate(&self, row: &EventRow) -> Result<(), String> {
        if self.version != 1
            || self.event_id != row.id
            || self.event_id.len() != 26
            || ulid::Ulid::from_string(&self.event_id).is_err()
            || self.source_event_id.len() != 26
            || ulid::Ulid::from_string(&self.source_event_id).is_err()
            || row.kind != EVENT_KIND
            || row.skill != self.name
            || row.harness.is_some()
            || row.scope.as_deref() != Some("global")
            || row.project_path.is_some()
            || row.status != "pending" && row.status != "interrupted"
            || row.reverted_by.is_some()
            || row.restorable
            || row.inverse.is_some()
            || row.backup_dir.is_some()
            || !valid_skill_name(&self.name)
            || !clean_absolute(&self.home)
            || !clean_absolute(&self.backup)
            || self.backup.file_name().and_then(|name| name.to_str()) != Some("backup")
            || self.backup.parent().is_none_or(|parent| {
                parent == self.home.join(".agents/skills-trash")
                    || !parent.starts_with(self.home.join(".agents/skills-trash"))
            })
            || !valid_digest(&self.expected_tree, "tree-v1:")
            || !valid_digest(&self.source_revision, "event-v1:")
            || matches!(self.phase, RestorePhase::Recorded) && self.stage_identity.is_some()
            || matches!(self.phase, RestorePhase::Staged) && self.stage_identity.is_none()
        {
            return Err("Invalid trial backup restore intent".into());
        }
        Ok(())
    }

    fn from_row(row: &EventRow) -> Result<Self, String> {
        let intent: Self =
            serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?;
        intent.validate(row)?;
        Ok(intent)
    }
}

fn stable_source_revision(row: &EventRow) -> Result<String, String> {
    let mut stable = row.clone();
    stable.reverted_by = None;
    let serialized = serde_json::to_vec(&stable).map_err(|error| error.to_string())?;
    let digest = Sha256::digest(serialized)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("event-v1:{digest}"))
}

fn clean_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.as_os_str().len() <= 4096
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
}

fn valid_digest(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn with_lease<T>(
    store: &EventStore,
    paths: &[PathBuf],
    timeout: Option<Duration>,
    cancellation: CancellationToken,
    f: impl FnOnce(&crate::skill_coordination::FinalizedWriteLease<'_>) -> Result<T, EventWriteFailure>,
) -> Result<T, EventWriteFailure> {
    let mut roots = paths.to_vec();
    roots.push(store.app_data.clone());
    roots.sort();
    roots.dedup();
    let scope = SkillReadScope::bind(&roots)
        .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
    let plan = CoordinationPlan::new_cancellable(
        roots
            .into_iter()
            .map(|path| DirectoryEffect::tree(path, CoordinationMode::Exclusive))
            .collect(),
        timeout,
        cancellation.clone(),
    )
    .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
    let guard = plan.acquire().map_err(|error| {
        if cancellation.is_cancelled() {
            EventWriteFailure::CancelledBeforeWrite
        } else {
            EventWriteFailure::BeforeWrite(error.to_string())
        }
    })?;
    let lease = guard.finalize_write(&scope, &[]).map_err(|error| {
        if cancellation.is_cancelled() {
            EventWriteFailure::CancelledBeforeWrite
        } else {
            EventWriteFailure::BeforeWrite(error.to_string())
        }
    })?;
    f(&lease)
}

fn restore_paths(home: &Path) -> (PathBuf, PathBuf) {
    (
        home.join(".agents/skills-trash"),
        home.join(".agents/skills"),
    )
}

fn verify_tree(path: &Path, expected: &str, limits: BackupCopyLimits) -> Result<(), String> {
    let parent = path.parent().ok_or("Restore tree has no parent")?;
    let name = path.file_name().ok_or("Restore tree has no name")?;
    let root = BackupSourceRoot::bind(parent).map_err(|error| error.to_string())?;
    let source = root.select(name).map_err(|error| error.to_string())?;
    let report = inspect_entry(
        &source.directory,
        &source.name,
        limits,
        &CancellationToken::default(),
    )
    .map_err(|error| error.to_string())?;
    if report.tree_identity != expected {
        return Err("Restore tree differs from its recorded content".into());
    }
    Ok(())
}

pub fn restore_expired_copy_trial_backup(
    home: &Path,
    store: &EventStore,
    source_event_id: &str,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
) -> Result<CopyTrialBackupRestoreReceipt, CopyTrialBackupRestoreError> {
    restore_expired_copy_trial_backup_with(
        home,
        store,
        source_event_id,
        limits,
        timeout,
        cancellation,
        |_| Ok(()),
    )
}

#[allow(clippy::too_many_arguments)]
fn restore_expired_copy_trial_backup_with(
    home: &Path,
    store: &EventStore,
    source_event_id: &str,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
    cancellation: CancellationToken,
    after_checkpoint: impl FnMut(RestoreCheckpoint) -> Result<(), TrialRestoreError>,
) -> Result<CopyTrialBackupRestoreReceipt, CopyTrialBackupRestoreError> {
    if cancellation.is_cancelled() {
        return Err(before(
            "Trial backup restore cancelled before recording".into(),
        ));
    }
    let source = store
        .get(source_event_id)
        .map_err(before)?
        .ok_or_else(|| before("Copy trial expiry event is missing".into()))?;
    let backup = CopyTrialExpiryIntent::completed_backup_for_home(&source, home).map_err(before)?;
    let id = ulid::Ulid::new().to_string();
    let intent = RestoreIntent {
        version: 1,
        event_id: id.clone(),
        source_event_id: backup.source_event_id,
        source_revision: stable_source_revision(&source).map_err(before)?,
        home: home.to_path_buf(),
        name: backup.name,
        backup: backup.backup,
        expected_tree: backup.expected_tree,
        phase: RestorePhase::Recorded,
        stage_identity: None,
    };
    let (trash, shared) = restore_paths(home);
    fs::create_dir_all(&trash).map_err(|error| before(error.to_string()))?;
    fs::create_dir_all(&shared).map_err(|error| before(error.to_string()))?;
    let stage = shared.join(format!(".trial-restore-{id}"));
    if fs::symlink_metadata(shared.join(&intent.name)).is_ok()
        || fs::symlink_metadata(&stage).is_ok()
    {
        return Err(before(
            "Restore target or its durable staging entry already exists".into(),
        ));
    }
    let draft = EventDraft {
        kind: EVENT_KIND.into(),
        skill: intent.name.clone(),
        harness: None,
        scope: Some("global".into()),
        project_path: None,
        payload: serde_json::to_value(&intent).map_err(|error| before(error.to_string()))?,
        inverse: None,
        backup_dir: None,
        restorable: false,
    };
    let recorded = with_lease(
        store,
        &[trash.clone(), shared.clone()],
        timeout,
        cancellation.clone(),
        |lease| {
            let guarded =
                GuardedEventStore::bind(store, lease).map_err(EventWriteFailure::BeforeWrite)?;
            guarded.record_trial_backup_restore(lease, &source, &id, draft)
        },
    );
    let (claimed, event) = match recorded {
        Ok(recorded) => recorded,
        Err(error) => return Err(record_failure(id, error)),
    };
    execute_with(
        home,
        store,
        claimed,
        event,
        intent,
        RestoreExecutionControl {
            limits,
            timeout,
            cancellation,
        },
        after_checkpoint,
    )
}

pub fn recover_copy_trial_backup_restore(
    home: &Path,
    store: &EventStore,
    row: &EventRow,
    limits: BackupCopyLimits,
    timeout: Option<Duration>,
) -> Result<CopyTrialBackupRestoreReceipt, String> {
    let intent = RestoreIntent::from_row(row)?;
    if intent.home != home {
        return Err("Trial backup restore home is outside the current scope".into());
    }
    let source = store
        .get(&intent.source_event_id)?
        .ok_or("Trial backup restore source is missing")?;
    if source.reverted_by.as_deref() != Some(row.id.as_str())
        || stable_source_revision(&source)? != intent.source_revision
    {
        return Err("Trial backup restore source changed since recording".into());
    }
    let mut unclaimed = source.clone();
    unclaimed.reverted_by = None;
    let backup = CopyTrialExpiryIntent::completed_backup_for_home(&unclaimed, home)?;
    if backup.name != intent.name
        || backup.backup != intent.backup
        || backup.expected_tree != intent.expected_tree
    {
        return Err("Trial backup restore source no longer matches its recorded backup".into());
    }
    execute(
        home,
        store,
        source,
        row.clone(),
        intent,
        RestoreExecutionControl {
            limits,
            timeout,
            cancellation: CancellationToken::default(),
        },
    )
    .map_err(|error| error.message)
}

fn execute(
    home: &Path,
    store: &EventStore,
    source: EventRow,
    event: EventRow,
    intent: RestoreIntent,
    control: RestoreExecutionControl,
) -> Result<CopyTrialBackupRestoreReceipt, CopyTrialBackupRestoreError> {
    execute_with(home, store, source, event, intent, control, |_| Ok(()))
}

fn execute_with(
    home: &Path,
    store: &EventStore,
    source: EventRow,
    event: EventRow,
    intent: RestoreIntent,
    operation: RestoreExecutionControl,
    mut after_checkpoint: impl FnMut(RestoreCheckpoint) -> Result<(), TrialRestoreError>,
) -> Result<CopyTrialBackupRestoreReceipt, CopyTrialBackupRestoreError> {
    let RestoreExecutionControl {
        limits,
        timeout,
        cancellation,
    } = operation;
    let (trash, shared) = restore_paths(home);
    let target = shared.join(&intent.name);
    let current_event = RefCell::new(event);
    let state_roots = vec![store.app_data.clone()];
    let restore = RestoreControl {
        stage_name: OsString::from(format!(".trial-restore-{}", intent.event_id)),
        expected_name: Some(&intent.name),
        expected_tree: Some(&intent.expected_tree),
        expected_stage: intent.stage_identity,
        state_roots: &state_roots,
        timeout,
        cancellation: &cancellation,
    };
    let publish = || -> Result<(), TrialRestoreError> {
        restore_trial_backup_with(
            home,
            &intent.backup.to_string_lossy(),
            limits,
            restore,
            |checkpoint, lease| {
                let guarded = GuardedEventStore::bind(store, lease)
                    .map_err(|message| transition_error(&intent.name, message))?;
                guarded
                    .require_trial_backup_restore(lease, &source, &current_event.borrow())
                    .map_err(|error| transition_error(&intent.name, error.to_string()))?;
                if let RestoreCheckpoint::StageReady(identity) = checkpoint {
                    let saved: RestoreIntent =
                        serde_json::from_value(current_event.borrow().payload.clone())
                            .map_err(|error| transition_error(&intent.name, error.to_string()))?;
                    if saved.phase == RestorePhase::Recorded {
                        let mut staged_intent = saved;
                        staged_intent.phase = RestorePhase::Staged;
                        staged_intent.stage_identity = Some(identity);
                        let updated = guarded
                            .advance_trial_backup_restore(
                                lease,
                                &source,
                                &current_event.borrow(),
                                serde_json::to_value(&staged_intent).map_err(|error| {
                                    transition_error(&intent.name, error.to_string())
                                })?,
                            )
                            .map_err(|error| transition_error(&intent.name, error.to_string()))?;
                        *current_event.borrow_mut() = updated;
                    } else if saved.stage_identity != Some(identity) {
                        return Err(transition_error(
                            &intent.name,
                            "Saved restore staging identity changed".into(),
                        ));
                    }
                }
                after_checkpoint(checkpoint)?;
                if cancellation.is_cancelled() {
                    return Ok(());
                }
                GuardedEventStore::bind(store, lease)
                    .map_err(|message| transition_error(&intent.name, message))?
                    .require_trial_backup_restore(lease, &source, &current_event.borrow())
                    .map_err(|error| transition_error(&intent.name, error.to_string()))
            },
        )?;
        Ok(())
    };
    let result = publish()
        .and_then(|()| after_checkpoint(RestoreCheckpoint::AfterLink))
        .map_err(|error| {
            let staged =
                serde_json::from_value::<RestoreIntent>(current_event.borrow().payload.clone())
                    .is_ok_and(|saved| saved.phase == RestorePhase::Staged);
            if staged && !error.publication_possible() {
                transition_error(&intent.name, error.to_string())
            } else {
                error
            }
        });
    match result {
        Ok(()) => {
            with_lease(
                store,
                &[trash, shared],
                timeout,
                CancellationToken::default(),
                |lease| {
                    let guarded = GuardedEventStore::bind(store, lease)
                        .map_err(EventWriteFailure::BeforeWrite)?;
                    guarded.require_trial_backup_restore(
                        lease,
                        &source,
                        &current_event.borrow(),
                    )?;
                    let current_intent = RestoreIntent::from_row(&current_event.borrow())
                        .map_err(EventWriteFailure::BeforeWrite)?;
                    let identity = current_intent.stage_identity.ok_or_else(|| {
                        EventWriteFailure::BeforeWrite(
                            "Completed restore has no saved staging identity".into(),
                        )
                    })?;
                    if entry_identity(&target).map_err(EventWriteFailure::BeforeWrite)? != identity
                    {
                        return Err(EventWriteFailure::BeforeWrite(
                            "Completed restore target has a different physical identity".into(),
                        ));
                    }
                    verify_tree(&target, &current_intent.expected_tree, limits)
                        .map_err(EventWriteFailure::BeforeWrite)?;
                    verify_tree(
                        &current_intent.backup,
                        &current_intent.expected_tree,
                        limits,
                    )
                    .map_err(EventWriteFailure::BeforeWrite)?;
                    guarded.finish_trial_backup_restore(lease, &source, &current_event.borrow())
                },
            )
            .map_err(|error| CopyTrialBackupRestoreError {
                event_id: Some(current_event.borrow().id.clone()),
                recovery_required: true,
                message: format!("{error}; restore completion requires recovery"),
            })?;
            Ok(CopyTrialBackupRestoreReceipt {
                restore_event_id: current_event.into_inner().id,
                source_event_id: source.id,
                skill_name: intent.name,
                target,
            })
        }
        Err(error) if !error.publication_possible() => {
            let cancellation = with_lease(
                store,
                &[trash, shared],
                timeout,
                CancellationToken::default(),
                |lease| {
                    let guarded = GuardedEventStore::bind(store, lease)
                        .map_err(EventWriteFailure::BeforeWrite)?;
                    guarded.require_trial_backup_restore(
                        lease,
                        &source,
                        &current_event.borrow(),
                    )?;
                    let current_intent = RestoreIntent::from_row(&current_event.borrow())
                        .map_err(EventWriteFailure::BeforeWrite)?;
                    if current_intent.phase != RestorePhase::Recorded {
                        return Err(EventWriteFailure::BeforeWrite(
                            "A staged restore cannot release its claim automatically".into(),
                        ));
                    }
                    match fs::symlink_metadata(&target) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(EventWriteFailure::BeforeWrite(error.to_string()))
                        }
                        Ok(_) => {
                            return Err(EventWriteFailure::BeforeWrite(
                                "Restore target appeared before cancellation".into(),
                            ))
                        }
                    }
                    guarded.cancel_trial_backup_restore(lease, &source, &current_event.borrow())
                },
            );
            Err(CopyTrialBackupRestoreError {
                event_id: Some(current_event.into_inner().id),
                recovery_required: cancellation.is_err(),
                message: if cancellation.is_ok() {
                    error.to_string()
                } else {
                    format!("{}; restore record requires recovery", error)
                },
            })
        }
        Err(error) => Err(CopyTrialBackupRestoreError {
            event_id: Some(current_event.into_inner().id),
            recovery_required: true,
            message: error.to_string(),
        }),
    }
}

fn transition_error(name: &str, message: String) -> TrialRestoreError {
    TrialRestoreError::PublicationUncertain {
        name: name.into(),
        message,
    }
}

fn record_failure(id: String, error: EventWriteFailure) -> CopyTrialBackupRestoreError {
    match error {
        EventWriteFailure::CancelledBeforeWrite => {
            before("Trial backup restore cancelled before recording".into())
        }
        EventWriteFailure::BeforeWrite(message) => before(message),
        EventWriteFailure::MayHaveWritten(message) => CopyTrialBackupRestoreError {
            event_id: Some(id),
            recovery_required: true,
            message: format!("{message}; restore recording may require recovery"),
        },
    }
}

fn before(message: String) -> CopyTrialBackupRestoreError {
    CopyTrialBackupRestoreError {
        event_id: None,
        recovery_required: false,
        message,
    }
}

#[cfg(test)]
mod tests;
