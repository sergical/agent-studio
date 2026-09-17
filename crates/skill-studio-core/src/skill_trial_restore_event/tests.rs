use super::*;
use crate::{
    skill_copy_trial_expiry::{expire_copy_trial, CopyTrialExpiryRequest},
    skill_deployment::{deployment_id, InstallScope, SkillDestination},
    skill_fork_registry::{
        AddMethod, CopyDeploymentRecord, RegistryOwnerRecord, TrialRecord, TrialScope, TrialStatus,
    },
    skill_service::{ScopedSkillService, SkillScope},
};
use chrono::Utc;
use std::{sync::Arc, thread};

const LIMITS: BackupCopyLimits = BackupCopyLimits {
    max_bytes: 1024 * 1024,
    max_entries: 100,
    max_depth: 8,
};
const TIMEOUT: Option<Duration> = Some(Duration::from_secs(3));
const DOCUMENT: &str = "---\nname: sample\ndescription: Restore fixture\n---\nRetained content.\n";

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    store: EventStore,
    source_id: String,
    backup: PathBuf,
    registry: Vec<u8>,
}
impl Fixture {
    fn new(project: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let project_path = root.join("project");
        fs::create_dir_all(home.join(".git")).unwrap();
        fs::create_dir_all(project_path.join(".git")).unwrap();
        let path = if project { &project_path } else { &home }.join(".agents/skills/sample");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("SKILL.md"), DOCUMENT).unwrap();
        let project_string = project.then(|| project_path.to_string_lossy().into_owned());
        let record = CopyDeploymentRecord {
            deployment_id: deployment_id(
                "sample",
                if project { "project" } else { "global" },
                SkillDestination::Universal,
                "universal",
                project_string.as_deref(),
                &path,
            ),
            name: "sample".into(),
            path: path.clone(),
            scope: if project {
                InstallScope::Project
            } else {
                InstallScope::Global
            },
            destination: SkillDestination::Universal,
            slot: "universal".into(),
            project_path: project_string.clone(),
            content_hash: crate::skill_discovery::live_skill_content_hash(
                &SkillReadScope::bind(std::slice::from_ref(&path)).unwrap(),
                &path,
            )
            .unwrap(),
            disabled: false,
        };
        let now = Utc::now();
        let trial = TrialRecord {
            deployment_id: record.deployment_id.clone(),
            started_at: (now - chrono::Duration::hours(24)).to_rfc3339(),
            expires_at: (now - chrono::Duration::seconds(1)).to_rfc3339(),
            status: TrialStatus::Active,
            method: AddMethod::Copy,
            scope: if project {
                TrialScope::Project
            } else {
                TrialScope::Global
            },
            project_path: project_string,
            skill_dir: path.clone(),
            deployment_fingerprint: crate::skill_event_store::fingerprint_path(&path),
            claude_link: None,
            claude_link_target: None,
        };
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(home.join(".agents/skill-studio.json"), serde_json::to_vec(&serde_json::json!({"copies": { record.deployment_id.clone(): record }, "trials": { format!("deployment/{}",record.deployment_id): trial }})).unwrap()).unwrap();
        let store = EventStore::open(&root.join("events")).unwrap();
        let mut service = ScopedSkillService::bind(SkillScope {
            home: home.clone(),
            projects: if project {
                vec![project_path.clone()]
            } else {
                vec![]
            },
            backing_roots: vec![],
            plugin_ownership_roots: if project { vec![root] } else { vec![] },
        })
        .unwrap();
        let receipt = expire_copy_trial(
            &mut service,
            &store,
            &CopyTrialExpiryRequest {
                deployment_id: record.deployment_id.clone(),
                expected_owner_revision: RegistryOwnerRecord::Copy(&record).revision().unwrap(),
            },
            now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        assert!(!path.exists());
        if project {
            fs::remove_dir_all(project_path).unwrap();
        }
        let registry = fs::read(home.join(".agents/skill-studio.json")).unwrap();
        Self {
            _temp: temp,
            home,
            store,
            source_id: receipt.event_id,
            backup: receipt.visible_trash,
            registry,
        }
    }
    fn restore(&self) -> Result<CopyTrialBackupRestoreReceipt, CopyTrialBackupRestoreError> {
        restore_expired_copy_trial_backup(
            &self.home,
            &self.store,
            &self.source_id,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
    }
    fn restore_with(
        &self,
        cancellation: CancellationToken,
        checkpoint: impl FnMut(RestoreCheckpoint) -> Result<(), TrialRestoreError>,
    ) -> Result<CopyTrialBackupRestoreReceipt, CopyTrialBackupRestoreError> {
        restore_expired_copy_trial_backup_with(
            &self.home,
            &self.store,
            &self.source_id,
            LIMITS,
            TIMEOUT,
            cancellation,
            checkpoint,
        )
    }
    fn interrupt_at(&self, selected: RestoreCheckpoint) -> EventRow {
        let error = self
            .restore_with(CancellationToken::default(), |checkpoint| {
                if std::mem::discriminant(&checkpoint) == std::mem::discriminant(&selected) {
                    return Err(transition_error("sample", "injected interruption".into()));
                }
                Ok(())
            })
            .unwrap_err();
        assert!(error.recovery_required);
        self.store
            .get(error.event_id.as_deref().unwrap())
            .unwrap()
            .unwrap()
    }
    fn target(&self) -> PathBuf {
        self.home.join(".agents/skills/sample")
    }
    fn pending_after_publication(&self) -> EventRow {
        let receipt = self.restore().unwrap();
        self.store
            .conn
            .execute(
                "UPDATE events SET status = 'interrupted' WHERE id = ?1",
                [&receipt.restore_event_id],
            )
            .unwrap();
        self.store.get(&receipt.restore_event_id).unwrap().unwrap()
    }
}

#[test]
fn restores_global_and_removed_project_without_recreating_ownership() {
    for project in [false, true] {
        let f = Fixture::new(project);
        let receipt = f.restore().unwrap();
        assert_eq!(
            fs::read_to_string(receipt.target.join("SKILL.md")).unwrap(),
            DOCUMENT
        );
        assert_eq!(
            fs::read_to_string(f.backup.join("SKILL.md")).unwrap(),
            DOCUMENT
        );
        assert_eq!(
            fs::read(f.home.join(".agents/skill-studio.json")).unwrap(),
            f.registry
        );
        let source = f.store.get(&f.source_id).unwrap().unwrap();
        assert!(!source.restorable);
        assert_eq!(
            source.reverted_by.as_deref(),
            Some(receipt.restore_event_id.as_str())
        );
        let restored = f.store.get(&receipt.restore_event_id).unwrap().unwrap();
        assert_eq!(restored.status, "done");
        assert!(!restored.restorable);
        assert!(f.restore().is_err());
    }
}

#[test]
fn refuses_changed_backup_and_occupied_target() {
    for occupied in [false, true] {
        let f = Fixture::new(false);
        if occupied {
            fs::create_dir_all(f.target()).unwrap();
            fs::write(f.target().join("SKILL.md"), "occupied").unwrap();
        } else {
            fs::write(f.backup.join("SKILL.md"), "changed").unwrap();
        }
        assert!(f.restore().is_err());
        assert!(f
            .store
            .get(&f.source_id)
            .unwrap()
            .unwrap()
            .reverted_by
            .is_none());
        if occupied {
            assert_eq!(
                fs::read_to_string(f.target().join("SKILL.md")).unwrap(),
                "occupied"
            );
        } else {
            assert!(!f.target().exists());
        }
    }
}

#[test]
fn resumes_the_published_inode_after_restart() {
    let f = Fixture::new(false);
    let row = f.pending_after_publication();
    let identity = entry_identity(&f.target()).unwrap();
    let receipt =
        recover_copy_trial_backup_restore(&f.home, &f.store, &row, LIMITS, TIMEOUT).unwrap();
    assert_eq!(entry_identity(&receipt.target).unwrap(), identity);
    assert_eq!(f.store.get(&row.id).unwrap().unwrap().status, "done");
}

#[test]
fn changed_backup_after_publication_keeps_recovery_and_claim() {
    let f = Fixture::new(false);
    let row = f.pending_after_publication();
    fs::write(f.backup.join("SKILL.md"), "changed backup").unwrap();
    assert!(recover_copy_trial_backup_restore(&f.home, &f.store, &row, LIMITS, TIMEOUT).is_err());
    assert_eq!(f.store.get(&row.id).unwrap().unwrap().status, "interrupted");
    assert_eq!(
        f.store
            .get(&f.source_id)
            .unwrap()
            .unwrap()
            .reverted_by
            .as_deref(),
        Some(row.id.as_str())
    );
    assert_eq!(
        fs::read_to_string(f.target().join("SKILL.md")).unwrap(),
        DOCUMENT
    );
}

#[test]
fn identical_replacement_target_is_not_adopted() {
    let f = Fixture::new(false);
    let row = f.pending_after_publication();
    fs::rename(f.target(), f.home.join("original-restored-tree")).unwrap();
    fs::create_dir_all(f.target()).unwrap();
    fs::write(f.target().join("SKILL.md"), DOCUMENT).unwrap();
    assert!(recover_copy_trial_backup_restore(&f.home, &f.store, &row, LIMITS, TIMEOUT).is_err());
    assert_eq!(f.store.get(&row.id).unwrap().unwrap().status, "interrupted");
    assert_eq!(
        fs::read_to_string(f.target().join("SKILL.md")).unwrap(),
        DOCUMENT
    );
}

#[test]
fn target_replacement_after_link_is_not_committed_as_success() {
    let f = Fixture::new(false);
    let original = f.home.join("published-before-finish");
    let error = f
        .restore_with(CancellationToken::default(), |checkpoint| {
            if matches!(checkpoint, RestoreCheckpoint::AfterLink) {
                fs::rename(f.target(), &original).unwrap();
                fs::create_dir_all(f.target()).unwrap();
                fs::write(f.target().join("SKILL.md"), DOCUMENT).unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert!(error.recovery_required);
    let id = error.event_id.unwrap();
    assert_eq!(f.store.get(&id).unwrap().unwrap().status, "pending");
    assert_eq!(
        f.store
            .get(&f.source_id)
            .unwrap()
            .unwrap()
            .reverted_by
            .as_deref(),
        Some(id.as_str())
    );
    assert!(original.join("SKILL.md").is_file());
    assert_eq!(
        fs::read_to_string(f.target().join("SKILL.md")).unwrap(),
        DOCUMENT
    );
}

#[test]
fn recovers_each_recorded_staged_published_and_linked_checkpoint() {
    for checkpoint in [
        RestoreCheckpoint::BeforeCopy,
        RestoreCheckpoint::StageReady(RestoreEntryIdentity {
            device: 0,
            inode: 0,
        }),
        RestoreCheckpoint::BeforePublish,
        RestoreCheckpoint::BeforeLink,
        RestoreCheckpoint::AfterLink,
    ] {
        let f = Fixture::new(false);
        let row = f.interrupt_at(checkpoint);
        assert!(matches!(row.status.as_str(), "pending" | "interrupted"));
        let receipt =
            recover_copy_trial_backup_restore(&f.home, &f.store, &row, LIMITS, TIMEOUT).unwrap();
        assert_eq!(
            fs::read_to_string(receipt.target.join("SKILL.md")).unwrap(),
            DOCUMENT
        );
        assert!(f.home.join(".claude/skills/sample").is_symlink());
        assert_eq!(f.store.get(&row.id).unwrap().unwrap().status, "done");
        assert_eq!(
            f.store
                .get(&f.source_id)
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some(row.id.as_str())
        );
    }
}

#[test]
fn missing_backup_after_publication_keeps_recovery_and_claim() {
    let f = Fixture::new(false);
    let row = f.pending_after_publication();
    fs::remove_dir_all(&f.backup).unwrap();
    assert!(recover_copy_trial_backup_restore(&f.home, &f.store, &row, LIMITS, TIMEOUT).is_err());
    assert_eq!(f.store.get(&row.id).unwrap().unwrap().status, "interrupted");
    assert_eq!(
        f.store
            .get(&f.source_id)
            .unwrap()
            .unwrap()
            .reverted_by
            .as_deref(),
        Some(row.id.as_str())
    );
    assert_eq!(
        fs::read_to_string(f.target().join("SKILL.md")).unwrap(),
        DOCUMENT
    );
}

#[test]
fn source_event_drift_after_recording_preserves_the_pending_claim() {
    let f = Fixture::new(false);
    let row = f.interrupt_at(RestoreCheckpoint::BeforeCopy);
    f.store
        .conn
        .execute(
            "UPDATE events SET skill = 'changed-source' WHERE id = ?1",
            [&f.source_id],
        )
        .unwrap();
    assert!(recover_copy_trial_backup_restore(&f.home, &f.store, &row, LIMITS, TIMEOUT).is_err());
    assert!(!f.target().exists());
    assert_eq!(
        f.store
            .get(&f.source_id)
            .unwrap()
            .unwrap()
            .reverted_by
            .as_deref(),
        Some(row.id.as_str())
    );
}

#[test]
fn source_event_drift_between_staging_and_publish_blocks_the_next_effect() {
    let f = Fixture::new(false);
    let error = f
        .restore_with(CancellationToken::default(), |checkpoint| {
            if matches!(checkpoint, RestoreCheckpoint::BeforePublish) {
                f.store
                    .conn
                    .execute(
                        "UPDATE events SET skill = 'changed-during-restore' WHERE id = ?1",
                        [&f.source_id],
                    )
                    .unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert!(error.recovery_required);
    let id = error.event_id.unwrap();
    let row = f.store.get(&id).unwrap().unwrap();
    assert_eq!(
        RestoreIntent::from_row(&row).unwrap().phase,
        RestorePhase::Staged
    );
    assert!(!f.target().exists());
    assert_eq!(
        f.store
            .get(&f.source_id)
            .unwrap()
            .unwrap()
            .reverted_by
            .as_deref(),
        Some(id.as_str())
    );
}

#[test]
fn invalid_restore_metadata_is_never_used_as_path_authority() {
    let f = Fixture::new(false);
    let row = f.interrupt_at(RestoreCheckpoint::BeforeCopy);
    let mut invalid = Vec::new();

    let mut wrong_scope = row.clone();
    wrong_scope.scope = Some("project".into());
    invalid.push(wrong_scope);

    let mut wrong_id = row.clone();
    wrong_id.payload["event_id"] = serde_json::json!("not-a-ulid");
    invalid.push(wrong_id);

    let mut outside = row.clone();
    outside.payload["backup"] = serde_json::json!("/tmp/outside/backup");
    invalid.push(outside);

    let mut invalid_phase = row.clone();
    invalid_phase.payload["phase"] = serde_json::json!("staged");
    invalid_phase.payload["stage_identity"] = serde_json::Value::Null;
    invalid.push(invalid_phase);

    let mut invalid_revision = row.clone();
    invalid_revision.payload["source_revision"] = serde_json::json!("event-v1:not-a-digest");
    invalid.push(invalid_revision);

    let mut unexpected_backup = row.clone();
    unexpected_backup.backup_dir = Some("backups/untrusted".into());
    invalid.push(unexpected_backup);

    for candidate in invalid {
        assert!(RestoreIntent::from_row(&candidate).is_err());
        assert!(
            recover_copy_trial_backup_restore(&f.home, &f.store, &candidate, LIMITS, TIMEOUT)
                .is_err()
        );
    }
    assert!(!f.target().exists());
    assert_eq!(f.store.get(&row.id).unwrap().unwrap().status, "pending");
}

#[test]
fn changed_backup_after_staging_and_stage_replacement_stay_recoverable() {
    let changed = Fixture::new(false);
    let changed_error = changed
        .restore_with(CancellationToken::default(), |checkpoint| {
            if matches!(checkpoint, RestoreCheckpoint::StageReady(_)) {
                fs::write(changed.backup.join("SKILL.md"), "changed after staging").unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert!(changed_error.recovery_required);
    let changed_id = changed_error.event_id.unwrap();
    assert_eq!(
        RestoreIntent::from_row(&changed.store.get(&changed_id).unwrap().unwrap())
            .unwrap()
            .phase,
        RestorePhase::Staged
    );
    assert_eq!(
        changed
            .store
            .get(&changed.source_id)
            .unwrap()
            .unwrap()
            .reverted_by
            .as_deref(),
        Some(changed_id.as_str())
    );
    assert!(!changed.target().exists());

    let replaced = Fixture::new(false);
    let original_stage = replaced.home.join("original-stage");
    let replacement_error = replaced
        .restore_with(CancellationToken::default(), |checkpoint| {
            if matches!(checkpoint, RestoreCheckpoint::StageReady(_)) {
                let id = replaced
                    .store
                    .get(&replaced.source_id)
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .unwrap();
                let stage = replaced
                    .home
                    .join(".agents/skills")
                    .join(format!(".trial-restore-{id}"));
                fs::rename(&stage, &original_stage).unwrap();
                fs::create_dir(&stage).unwrap();
                fs::write(stage.join("SKILL.md"), DOCUMENT).unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert!(replacement_error.recovery_required);
    let replacement_id = replacement_error.event_id.unwrap();
    assert!(original_stage.join("SKILL.md").is_file());
    assert!(!replaced.target().exists());
    assert_eq!(
        replaced
            .store
            .get(&replaced.source_id)
            .unwrap()
            .unwrap()
            .reverted_by
            .as_deref(),
        Some(replacement_id.as_str())
    );
}

#[test]
fn unbound_stage_collision_fails_safely_and_releases_the_claim() {
    let f = Fixture::new(false);
    let collision = Arc::new(std::sync::Mutex::new(None));
    let saved = collision.clone();
    let error = f
        .restore_with(CancellationToken::default(), |checkpoint| {
            if matches!(checkpoint, RestoreCheckpoint::BeforeCopy) {
                let id = f
                    .store
                    .get(&f.source_id)
                    .unwrap()
                    .unwrap()
                    .reverted_by
                    .unwrap();
                let stage = f
                    .home
                    .join(".agents/skills")
                    .join(format!(".trial-restore-{id}"));
                fs::create_dir(&stage).unwrap();
                fs::write(stage.join("foreign"), "preserve").unwrap();
                *saved.lock().unwrap() = Some(stage);
            }
            Ok(())
        })
        .unwrap_err();
    assert!(!error.recovery_required);
    let event = f
        .store
        .get(error.event_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(event.status, "failed");
    assert!(f
        .store
        .get(&f.source_id)
        .unwrap()
        .unwrap()
        .reverted_by
        .is_none());
    assert_eq!(
        fs::read_to_string(collision.lock().unwrap().as_ref().unwrap().join("foreign")).unwrap(),
        "preserve"
    );
    assert!(!f.target().exists());
    assert!(f.backup.join("SKILL.md").is_file());
}

#[test]
fn cancellation_while_database_is_busy_does_not_require_recovery() {
    let fixture = Fixture::new(false);
    let blocker = rusqlite::Connection::open(fixture.store.conn.path().unwrap()).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let before = fixture.store.get(&fixture.source_id).unwrap().unwrap();
    let token = CancellationToken::default();
    let cancel = token.clone();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        cancel.cancel();
    });
    let error = restore_expired_copy_trial_backup(
        &fixture.home,
        &fixture.store,
        &fixture.source_id,
        LIMITS,
        TIMEOUT,
        token,
    )
    .unwrap_err();
    canceller.join().unwrap();
    blocker.execute_batch("ROLLBACK").unwrap();
    assert!(!error.recovery_required);
    assert!(error.event_id.is_none());
    assert!(error.message.contains("cancelled before recording"));
    assert_eq!(
        serde_json::to_value(fixture.store.get(&fixture.source_id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    let count: i64 = fixture
        .store
        .conn
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    assert!(!fixture.target().exists());
    assert_eq!(
        fs::read(fixture.backup.join("SKILL.md")).unwrap(),
        DOCUMENT.as_bytes()
    );
    assert_eq!(
        fs::read(fixture.home.join(".agents/skill-studio.json")).unwrap(),
        fixture.registry
    );
}

#[test]
fn cancellation_before_record_and_after_record_settles_without_publication() {
    let waiting = Fixture::new(false);
    let shared = waiting.home.join(".agents/skills");
    let guard = CoordinationPlan::new(
        vec![DirectoryEffect::tree(shared, CoordinationMode::Shared)],
        Some(Duration::from_secs(2)),
    )
    .unwrap()
    .acquire()
    .unwrap();
    let token = CancellationToken::default();
    let cancel = token.clone();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        cancel.cancel();
    });
    let error = restore_expired_copy_trial_backup(
        &waiting.home,
        &waiting.store,
        &waiting.source_id,
        LIMITS,
        Some(Duration::from_secs(2)),
        token,
    )
    .unwrap_err();
    drop(guard);
    canceller.join().unwrap();
    assert!(error.event_id.is_none());
    assert!(!error.recovery_required);
    assert!(waiting
        .store
        .get(&waiting.source_id)
        .unwrap()
        .unwrap()
        .reverted_by
        .is_none());

    let recorded = Fixture::new(false);
    let token = CancellationToken::default();
    let stop = token.clone();
    let error = recorded
        .restore_with(token, |checkpoint| {
            if matches!(checkpoint, RestoreCheckpoint::BeforeCopy) {
                stop.cancel();
            }
            Ok(())
        })
        .unwrap_err();
    assert!(!error.recovery_required);
    let row = recorded
        .store
        .get(error.event_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "failed");
    assert!(recorded
        .store
        .get(&recorded.source_id)
        .unwrap()
        .unwrap()
        .reverted_by
        .is_none());
    assert!(!recorded.target().exists());
    assert!(recorded.backup.join("SKILL.md").is_file());
}

#[test]
fn refuses_ineligible_source_status_kind_home_and_duplicate_claim() {
    let wrong_status = Fixture::new(false);
    wrong_status
        .store
        .conn
        .execute(
            "UPDATE events SET status = 'interrupted' WHERE id = ?1",
            [&wrong_status.source_id],
        )
        .unwrap();
    assert!(wrong_status.restore().is_err());
    assert!(wrong_status
        .store
        .get(&wrong_status.source_id)
        .unwrap()
        .unwrap()
        .reverted_by
        .is_none());

    let wrong_kind = Fixture::new(false);
    wrong_kind
        .store
        .conn
        .execute(
            "UPDATE events SET kind = 'other' WHERE id = ?1",
            [&wrong_kind.source_id],
        )
        .unwrap();
    assert!(wrong_kind.restore().is_err());

    let wrong_home = Fixture::new(false);
    assert!(restore_expired_copy_trial_backup(
        &wrong_home.home.join("another-home"),
        &wrong_home.store,
        &wrong_home.source_id,
        LIMITS,
        TIMEOUT,
        CancellationToken::default(),
    )
    .is_err());

    let claimed = Fixture::new(false);
    let row = claimed.interrupt_at(RestoreCheckpoint::BeforeCopy);
    assert!(claimed.restore().is_err());
    let restore_count: i64 = claimed
        .store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE kind = ?1",
            [EVENT_KIND],
            |result| result.get(0),
        )
        .unwrap();
    assert_eq!(restore_count, 1);
    assert_eq!(
        claimed
            .store
            .get(&claimed.source_id)
            .unwrap()
            .unwrap()
            .reverted_by
            .as_deref(),
        Some(row.id.as_str())
    );
}

#[test]
fn concurrent_restore_requests_publish_only_one_operation() {
    let fixture = Fixture::new(false);
    let first = EventStore::open(&fixture.store.app_data).unwrap();
    let second = EventStore::open(&fixture.store.app_data).unwrap();
    let barrier = std::sync::Barrier::new(2);
    let results = thread::scope(|scope| {
        let run = |store: EventStore| {
            let home = &fixture.home;
            let source_id = &fixture.source_id;
            let barrier = &barrier;
            scope.spawn(move || {
                let source = store.get(source_id).unwrap().unwrap();
                assert!(source.reverted_by.is_none());
                let backup =
                    CopyTrialExpiryIntent::completed_backup_for_home(&source, home).unwrap();
                let id = ulid::Ulid::new().to_string();
                let intent = RestoreIntent {
                    version: 1,
                    event_id: id.clone(),
                    source_event_id: backup.source_event_id,
                    source_revision: stable_source_revision(&source).unwrap(),
                    home: home.clone(),
                    name: backup.name,
                    backup: backup.backup,
                    expected_tree: backup.expected_tree,
                    phase: RestorePhase::Recorded,
                    stage_identity: None,
                };
                let draft = EventDraft {
                    kind: EVENT_KIND.into(),
                    skill: intent.name.clone(),
                    harness: None,
                    scope: Some("global".into()),
                    project_path: None,
                    payload: serde_json::to_value(&intent).unwrap(),
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                };
                barrier.wait();
                let (trash, shared) = restore_paths(home);
                let (_, event) = with_lease(
                    &store,
                    &[trash, shared],
                    TIMEOUT,
                    CancellationToken::default(),
                    |lease| {
                        GuardedEventStore::bind(&store, lease)
                            .map_err(EventWriteFailure::BeforeWrite)?
                            .record_trial_backup_restore(lease, &source, &id, draft)
                    },
                )
                .map_err(|error| error.to_string())?;
                recover_copy_trial_backup_restore(home, &store, &event, LIMITS, TIMEOUT)
            })
        };
        let first = run(first);
        let second = run(second);
        [first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let receipt = results.into_iter().find_map(Result::ok).unwrap();
    let count: i64 = fixture
        .store
        .conn
        .query_row(
            "SELECT count(*) FROM events WHERE kind = ?1",
            [EVENT_KIND],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        fixture
            .store
            .get(&receipt.restore_event_id)
            .unwrap()
            .unwrap()
            .status,
        "done"
    );
    assert_eq!(
        fixture
            .store
            .get(&fixture.source_id)
            .unwrap()
            .unwrap()
            .reverted_by
            .as_deref(),
        Some(receipt.restore_event_id.as_str())
    );
    assert_eq!(
        fs::read_to_string(fixture.target().join("SKILL.md")).unwrap(),
        DOCUMENT
    );
    assert_eq!(
        fs::read_to_string(fixture.backup.join("SKILL.md")).unwrap(),
        DOCUMENT
    );
    assert_eq!(
        fs::read(fixture.home.join(".agents/skill-studio.json")).unwrap(),
        fixture.registry
    );
}

#[test]
fn may_have_written_record_failure_exposes_the_recovery_event() {
    let id = ulid::Ulid::new().to_string();
    let error = record_failure(
        id.clone(),
        EventWriteFailure::MayHaveWritten("commit receipt unavailable".into()),
    );
    assert_eq!(error.event_id.as_deref(), Some(id.as_str()));
    assert!(error.recovery_required);
    assert!(error.message.contains("recovery"));
}
