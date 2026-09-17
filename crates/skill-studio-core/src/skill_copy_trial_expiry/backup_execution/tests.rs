use super::*;
use crate::{
    skill_deployment::deployment_id, skill_scope::SkillReadScope, skill_service::SkillScope,
};
use std::fs;

const DOCUMENT: &str =
    "---\nname: sample\ndescription: Trial backup fixture\n---\nPreserve this skill.\n";
const LIMITS: BackupCopyLimits = BackupCopyLimits {
    max_bytes: 1024 * 1024,
    max_entries: 100,
    max_depth: 8,
};
const TIMEOUT: Option<Duration> = Some(Duration::from_secs(5));

struct Fixture {
    _temp: tempfile::TempDir,
    scope: SkillScope,
    store: EventStore,
    request: CopyTrialExpiryRequest,
    source: PathBuf,
    readers: Vec<PathBuf>,
    registry: Vec<u8>,
    now: DateTime<Utc>,
}

impl Fixture {
    fn new(project: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let project_path = root.join("project");
        fs::create_dir_all(home.join(".git")).unwrap();
        fs::create_dir_all(project_path.join(".git")).unwrap();
        fs::create_dir_all(home.join(".agents")).unwrap();
        let base = if project { &project_path } else { &home };
        let source = base.join(".agents/skills/sample");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("SKILL.md"), DOCUMENT).unwrap();
        fs::write(source.join("resource.txt"), "resource bytes").unwrap();
        let readers: Vec<_> = [".claude/skills/sample", ".codex/skills/sample"]
            .iter()
            .map(|relative| {
                let path = base.join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink("../../.agents/skills/sample", &path).unwrap();
                path
            })
            .collect();
        let project_string = project.then(|| project_path.to_string_lossy().into_owned());
        let scope_name = if project { "project" } else { "global" };
        let read_scope = SkillReadScope::bind(std::slice::from_ref(&source)).unwrap();
        let record = CopyDeploymentRecord {
            deployment_id: deployment_id(
                "sample",
                scope_name,
                SkillDestination::Universal,
                "universal",
                project_string.as_deref(),
                &source,
            ),
            name: "sample".into(),
            path: source.clone(),
            scope: if project {
                InstallScope::Project
            } else {
                InstallScope::Global
            },
            destination: SkillDestination::Universal,
            slot: "universal".into(),
            project_path: project_string.clone(),
            content_hash: crate::skill_discovery::live_skill_content_hash(&read_scope, &source)
                .unwrap(),
            disabled: false,
        };
        let now = Utc::now();
        let mut raw = serde_json::to_value(&record).unwrap();
        raw["future_copy_field"] = serde_json::json!({"keep":true});
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
            project_path: project_string.clone(),
            skill_dir: source.clone(),
            deployment_fingerprint: crate::skill_event_store::fingerprint_path(&source),
            claude_link: Some(readers[0].clone()),
            claude_link_target: Some(PathBuf::from("../../.agents/skills/sample")),
        };
        let mut trial_raw = serde_json::to_value(trial).unwrap();
        trial_raw["future_trial_field"] = serde_json::json!([1, 2, 3]);
        let other_source = base.join(".agents/skills/other");
        fs::create_dir_all(&other_source).unwrap();
        fs::write(
            other_source.join("SKILL.md"),
            DOCUMENT.replace("sample", "other"),
        )
        .unwrap();
        let other_record = CopyDeploymentRecord {
            deployment_id: deployment_id(
                "other",
                scope_name,
                SkillDestination::Universal,
                "universal",
                project_string.as_deref(),
                &other_source,
            ),
            name: "other".into(),
            path: other_source.clone(),
            scope: record.scope.clone(),
            destination: SkillDestination::Universal,
            slot: "universal".into(),
            project_path: project_string.clone(),
            content_hash: crate::skill_discovery::live_skill_content_hash(
                &SkillReadScope::bind(std::slice::from_ref(&other_source)).unwrap(),
                &other_source,
            )
            .unwrap(),
            disabled: false,
        };
        let mut other_raw = serde_json::to_value(&other_record).unwrap();
        other_raw["unknown_copy_field"] = serde_json::json!({"preserve": true});
        let other_trial = TrialRecord {
            deployment_id: other_record.deployment_id.clone(),
            started_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::hours(24)).to_rfc3339(),
            status: TrialStatus::Active,
            method: AddMethod::Copy,
            scope: if project {
                TrialScope::Project
            } else {
                TrialScope::Global
            },
            project_path: project_string,
            skill_dir: other_source.clone(),
            deployment_fingerprint: crate::skill_event_store::fingerprint_path(&other_source),
            claude_link: None,
            claude_link_target: None,
        };
        let mut other_trial_raw = serde_json::to_value(other_trial).unwrap();
        other_trial_raw["unknown_trial_field"] = serde_json::json!(["preserve"]);
        let registry =
            serde_json::to_vec(&serde_json::json!({"version":4,"future_root":{"keep":true},
            "copies":{record.deployment_id.clone():raw,other_record.deployment_id.clone():other_raw},
            "trials":{format!("deployment/{}",record.deployment_id):trial_raw,
                format!("deployment/{}",other_record.deployment_id):other_trial_raw}}))
            .unwrap();
        fs::write(home.join(".agents/skill-studio.json"), &registry).unwrap();
        let scope = SkillScope {
            home,
            projects: if project { vec![project_path] } else { vec![] },
            backing_roots: vec![],
            plugin_ownership_roots: if project { vec![root.clone()] } else { vec![] },
        };
        let store = EventStore::open(&root.join("events")).unwrap();
        let request = CopyTrialExpiryRequest {
            deployment_id: record.deployment_id.clone(),
            expected_owner_revision: RegistryOwnerRecord::Copy(&record).revision().unwrap(),
        };
        Self {
            _temp: temp,
            scope,
            store,
            request,
            source,
            readers,
            registry,
            now,
        }
    }

    fn unchanged(&self) {
        assert_eq!(
            fs::read_to_string(self.source.join("SKILL.md")).unwrap(),
            DOCUMENT
        );
        assert_eq!(
            fs::read_to_string(self.source.join("resource.txt")).unwrap(),
            "resource bytes"
        );
        assert_eq!(
            fs::read(self.scope.home.join(".agents/skill-studio.json")).unwrap(),
            self.registry
        );
        for reader in &self.readers {
            assert_eq!(
                fs::read_link(reader).unwrap(),
                PathBuf::from("../../.agents/skills/sample")
            );
        }
    }

    fn event_count(&self) -> i64 {
        self.store
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap()
    }
}

#[test]
fn records_before_copy_and_keeps_global_project_sources_live_with_verified_hidden_backups() {
    for project in [false, true] {
        let fixture = Fixture::new(project);
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let pending = begin_with_checkpoint(
            &mut service,
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
            |phase| {
                if phase == Checkpoint::PendingRecorded {
                    assert_eq!(fixture.event_count(), 1);
                    assert!(!fixture.scope.home.join(".agents/skills-trash").exists());
                    fixture.unchanged();
                }
                Ok(())
            },
        )
        .unwrap();
        fixture.unchanged();
        let row = fixture.store.get(&pending.event_id).unwrap().unwrap();
        assert_eq!(row.status, "pending");
        assert!(!row.restorable);
        let paths = CopyTrialExpiryIntent::from_event(&row)
            .unwrap()
            .paths_for_scope(&fixture.scope)
            .unwrap();
        assert_eq!(pending.verified_backup, paths.hidden_trash);
        assert!(!paths.visible_trash.exists());
        assert!(!paths.source_quarantine.exists());
        assert!(paths.reader_stages.iter().all(|path| !path.exists()));
        assert_eq!(
            fs::read_to_string(pending.verified_backup.join("SKILL.md")).unwrap(),
            DOCUMENT
        );
        assert_eq!(
            fs::read_to_string(pending.verified_backup.join("resource.txt")).unwrap(),
            "resource bytes"
        );
        assert_eq!(
            crate::skill_event_store::fingerprint_path(&pending.verified_backup),
            crate::skill_event_store::fingerprint_path(&fixture.source)
        );
    }
}

#[test]
fn interruption_at_each_backup_phase_retains_pending_intent_and_original_ownership() {
    for stopped in [
        Checkpoint::PendingRecorded,
        Checkpoint::TrashCreated,
        Checkpoint::StageCreated,
        Checkpoint::BackupCopied,
    ] {
        let fixture = Fixture::new(true);
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let error = begin_with_checkpoint(
            &mut service,
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
            |phase| {
                if phase == stopped {
                    Err(format!("interrupted at {phase:?}"))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(error.message.contains("interrupted at"), "{error:?}");
        assert!(error.recovery_required);
        let id = error.event_id.unwrap();
        fixture.unchanged();
        assert_eq!(fixture.event_count(), 1);
        assert_eq!(fixture.store.get(&id).unwrap().unwrap().status, "pending");
        fixture.store.reconcile_at_startup().unwrap();
        let row = fixture.store.get(&id).unwrap().unwrap();
        assert_eq!(row.status, "interrupted");
        let paths = CopyTrialExpiryIntent::from_event(&row)
            .unwrap()
            .paths_for_scope(&fixture.scope)
            .unwrap();
        assert!(!paths.visible_trash.exists());
        assert!(!paths.source_quarantine.exists());
    }
}

#[test]
fn restart_recovers_every_backup_effect_boundary() {
    for stopped in [
        Checkpoint::PendingRecorded,
        Checkpoint::TrashCreated,
        Checkpoint::StageCreated,
        Checkpoint::BackupCopied,
    ] {
        let fixture = Fixture::new(true);
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let error = begin_with_checkpoint(
            &mut service,
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
            |phase| {
                (phase != stopped)
                    .then_some(())
                    .ok_or_else(|| "interrupted".into())
            },
        )
        .unwrap_err();
        drop(service);
        let id = error.event_id.unwrap();
        let row = fixture.store.get(&id).unwrap().unwrap();
        assert!(recover_copy_trial_expiry(
            &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
            &fixture.store,
            &row,
            LIMITS,
            TIMEOUT,
        )
        .unwrap()
        .is_none());
        assert_eq!(fixture.store.get(&id).unwrap().unwrap().status, "failed");
        fixture.unchanged();
        let paths = CopyTrialExpiryIntent::from_event(&row)
            .unwrap()
            .paths_for_scope(&fixture.scope)
            .unwrap();
        assert!(!paths.visible_trash.exists());
        assert!(!paths.source_quarantine.exists());
    }
}

#[test]
fn changed_owner_content_or_duplicate_trial_refuses_before_recording() {
    for case in ["owner", "content", "reader", "trial-alias"] {
        let mut fixture = Fixture::new(false);
        match case {
            "owner" => fixture.request.expected_owner_revision = "stale".into(),
            "content" => fs::write(fixture.source.join("SKILL.md"), "external edit").unwrap(),
            "reader" => {
                fs::remove_file(&fixture.readers[0]).unwrap();
                std::os::unix::fs::symlink("replacement", &fixture.readers[0]).unwrap();
            }
            "trial-alias" => {
                let mut raw: Value = serde_json::from_slice(&fixture.registry).unwrap();
                raw["trials"]["duplicate"] =
                    serde_json::json!({"deployment_id":fixture.request.deployment_id});
                fixture.registry = serde_json::to_vec(&raw).unwrap();
                fs::write(
                    fixture.scope.home.join(".agents/skill-studio.json"),
                    &fixture.registry,
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let error = begin_copy_trial_expiry(
            &mut service,
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap_err();
        assert!(error.event_id.is_none(), "{case}: {error:?}");
        assert!(!error.recovery_required);
        assert_eq!(fixture.event_count(), 0);
        assert!(!fixture.scope.home.join(".agents/skills-trash").exists());
        assert_eq!(
            fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
            fixture.registry
        );
        match case {
            "content" => assert_eq!(
                fs::read_to_string(fixture.source.join("SKILL.md")).unwrap(),
                "external edit"
            ),
            "reader" => assert_eq!(
                fs::read_link(&fixture.readers[0]).unwrap(),
                Path::new("replacement")
            ),
            _ => fixture.unchanged(),
        }
    }
}

#[test]
fn unavailable_event_store_refuses_before_record_or_effect() {
    let fixture = Fixture::new(false);
    let unavailable = fixture.store.app_data.with_extension("unavailable");
    fs::rename(&fixture.store.app_data, &unavailable).unwrap();
    let error = expire_copy_trial(
        &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
        &fixture.store,
        &fixture.request,
        fixture.now,
        LIMITS,
        TIMEOUT,
        CancellationToken::default(),
    )
    .unwrap_err();
    assert!(error.event_id.is_none());
    assert!(!error.recovery_required);
    assert_eq!(fixture.event_count(), 0);
    assert!(!fixture.scope.home.join(".agents/skills-trash").exists());
    fixture.unchanged();
}

#[test]
fn changed_staging_permissions_or_identity_refuses_copy_and_preserves_live_state() {
    use std::os::unix::fs::PermissionsExt;
    for replacement in [false, true] {
        let fixture = Fixture::new(false);
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let error = begin_with_checkpoint(
            &mut service,
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
            |phase| {
                if phase == Checkpoint::StageCreated {
                    let trash = fixture.scope.home.join(".agents/skills-trash");
                    let stage = fs::read_dir(trash).unwrap().next().unwrap().unwrap().path();
                    if replacement {
                        fs::rename(&stage, stage.with_extension("replaced")).unwrap();
                        fs::create_dir(&stage).unwrap();
                        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
                    } else {
                        fs::set_permissions(&stage, fs::Permissions::from_mode(0o755)).unwrap();
                    }
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.recovery_required);
        let row = fixture
            .store
            .get(&error.event_id.unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "pending");
        let paths = CopyTrialExpiryIntent::from_event(&row)
            .unwrap()
            .paths_for_scope(&fixture.scope)
            .unwrap();
        assert!(!paths.hidden_trash.exists());
        fixture.unchanged();
    }
}

#[test]
fn cancellation_before_and_after_record_preserves_correct_recovery_classification() {
    for after_record in [false, true] {
        let fixture = Fixture::new(false);
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let cancellation = CancellationToken::default();
        if !after_record {
            cancellation.cancel();
        }
        let error = begin_with_checkpoint(
            &mut service,
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            cancellation.clone(),
            |phase| {
                if phase == Checkpoint::PendingRecorded {
                    cancellation.cancel();
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.recovery_required, after_record);
        assert_eq!(error.event_id.is_some(), after_record);
        assert_eq!(fixture.event_count(), i64::from(after_record));
        if let Some(id) = error.event_id {
            assert_eq!(fixture.store.get(&id).unwrap().unwrap().status, "pending");
        }
        assert!(!fixture.scope.home.join(".agents/skills-trash").exists());
        fixture.unchanged();
    }
}

#[test]
fn expires_global_and_project_trials_into_visible_trash() {
    for project in [false, true] {
        let fixture = Fixture::new(project);
        let receipt = expire_copy_trial(
            &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        let row = fixture.store.get(&receipt.event_id).unwrap().unwrap();
        let intent = CopyTrialExpiryIntent::from_event(&row).unwrap();
        let expected_registry = intent
            .selection
            .without_selected_records(&fixture.registry)
            .unwrap();
        assert_eq!(receipt.skill_name, "sample");
        assert!(receipt.visible_trash.join("SKILL.md").is_file());
        assert!(!fixture.source.exists());
        assert!(fixture
            .readers
            .iter()
            .all(|reader| fs::symlink_metadata(reader).is_err()));
        assert_eq!(
            fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
            expected_registry
        );
        assert_eq!(row.status, "done");
        assert!(!row.restorable);
        assert!(row.inverse.is_none());
        let paths = intent.paths_for_scope(&fixture.scope).unwrap();
        assert!(paths.source_quarantine.join("SKILL.md").is_file());
        assert!(!paths.hidden_trash.exists());
        let restored = crate::skill_trial_restore::restore_trial_backup(
            &fixture.scope.home,
            receipt.visible_trash.to_str().unwrap(),
            LIMITS,
        )
        .unwrap();
        assert_eq!(restored.name, "sample");
        assert_eq!(
            fs::read_to_string(restored.target.join("SKILL.md")).unwrap(),
            DOCUMENT
        );
        assert_eq!(
            fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
            expected_registry
        );
        assert!(receipt.visible_trash.join("SKILL.md").is_file());
    }
}

#[test]
fn restart_recovers_every_forward_effect_boundary() {
    for stopped in [
        ForwardCheckpoint::SourceStageCreated,
        ForwardCheckpoint::ReaderStageCreated,
        ForwardCheckpoint::ReaderMoved,
        ForwardCheckpoint::SourceMoved,
        ForwardCheckpoint::RegistryPublished,
        ForwardCheckpoint::TrashPublished,
    ] {
        let fixture = Fixture::new(true);
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let pending = begin_copy_trial_expiry(
            &mut service,
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        let row = fixture.store.get(&pending.event_id).unwrap().unwrap();
        let intent = CopyTrialExpiryIntent::from_event(&row).unwrap();
        let error = execute_forward_with_checkpoint(
            &mut service,
            &fixture.store,
            &row,
            &intent,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
            |phase| {
                (phase != stopped)
                    .then_some(())
                    .ok_or_else(|| "interrupted".into())
            },
        )
        .unwrap_err();
        assert_eq!(error, "interrupted", "{stopped:?}");
        drop(service);
        let interrupted = fixture.store.get(&pending.event_id).unwrap().unwrap();
        let published = matches!(
            stopped,
            ForwardCheckpoint::RegistryPublished | ForwardCheckpoint::TrashPublished
        );
        let recovered = recover_copy_trial_expiry(
            &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
            &fixture.store,
            &interrupted,
            LIMITS,
            TIMEOUT,
        )
        .unwrap();
        assert_eq!(recovered.is_some(), published, "{stopped:?}");
        let final_row = fixture.store.get(&pending.event_id).unwrap().unwrap();
        assert_eq!(
            final_row.status,
            if published { "done" } else { "failed" },
            "{stopped:?}"
        );
        if published {
            assert!(!fixture.source.exists(), "{stopped:?}");
            assert!(intent
                .paths_for_scope(&fixture.scope)
                .unwrap()
                .visible_trash
                .join("SKILL.md")
                .is_file());
        } else {
            fixture.unchanged();
        }
    }
}

#[test]
fn recovery_preserves_replacements_and_finishes_after_publication() {
    for replacement in [
        "source",
        "reader",
        "published-source",
        "published-file",
        "published-symlink",
    ] {
        let fixture = Fixture::new(false);
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let pending = begin_copy_trial_expiry(
            &mut service,
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        let row = fixture.store.get(&pending.event_id).unwrap().unwrap();
        let intent = CopyTrialExpiryIntent::from_event(&row).unwrap();
        let stopped = match replacement {
            "reader" => ForwardCheckpoint::ReaderMoved,
            "source" => ForwardCheckpoint::SourceMoved,
            "published-source" | "published-file" | "published-symlink" => {
                ForwardCheckpoint::RegistryPublished
            }
            _ => unreachable!(),
        };
        let interrupted = execute_forward_with_checkpoint(
            &mut service,
            &fixture.store,
            &row,
            &intent,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
            |phase| {
                (phase != stopped)
                    .then_some(())
                    .ok_or_else(|| "interrupted".into())
            },
        )
        .unwrap_err();
        assert_eq!(interrupted, "interrupted", "{replacement}");
        drop(service);
        let replacement_path = if replacement == "reader" {
            fixture.readers[0].clone()
        } else {
            fixture.source.clone()
        };
        if replacement == "reader" || replacement == "published-file" {
            fs::write(&replacement_path, "replacement reader").unwrap();
        } else if replacement == "published-symlink" {
            std::os::unix::fs::symlink("replacement-target", &replacement_path).unwrap();
        } else {
            fs::create_dir(&replacement_path)
                .unwrap_or_else(|error| panic!("{replacement}: {error}"));
            fs::write(replacement_path.join("SKILL.md"), "replacement source").unwrap();
        }
        let result = recover_copy_trial_expiry(
            &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
            &fixture.store,
            &fixture.store.get(&pending.event_id).unwrap().unwrap(),
            LIMITS,
            TIMEOUT,
        );
        if replacement.starts_with("published-") {
            assert!(result
                .unwrap_or_else(|error| panic!("{replacement}: {error}"))
                .is_some());
            match replacement {
                "published-source" => assert_eq!(
                    fs::read_to_string(replacement_path.join("SKILL.md")).unwrap(),
                    "replacement source"
                ),
                "published-file" => assert_eq!(
                    fs::read_to_string(&replacement_path).unwrap(),
                    "replacement reader"
                ),
                "published-symlink" => assert_eq!(
                    fs::read_link(&replacement_path).unwrap(),
                    Path::new("replacement-target")
                ),
                _ => unreachable!(),
            }
        } else {
            assert!(result.is_err(), "{replacement}");
            let retained = if replacement == "reader" {
                fs::read_to_string(&replacement_path).unwrap()
            } else {
                fs::read_to_string(replacement_path.join("SKILL.md")).unwrap()
            };
            assert!(retained.starts_with("replacement"));
            assert_eq!(
                fixture
                    .store
                    .get(&pending.event_id)
                    .unwrap()
                    .unwrap()
                    .status,
                "pending"
            );
        }
    }
}

#[test]
fn cancellation_during_forward_rolls_back_on_recovery() {
    let fixture = Fixture::new(true);
    let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
    let pending = begin_copy_trial_expiry(
        &mut service,
        &fixture.store,
        &fixture.request,
        fixture.now,
        LIMITS,
        TIMEOUT,
        CancellationToken::default(),
    )
    .unwrap();
    let row = fixture.store.get(&pending.event_id).unwrap().unwrap();
    let intent = CopyTrialExpiryIntent::from_event(&row).unwrap();
    let cancellation = CancellationToken::default();
    let signal = cancellation.clone();
    let error = execute_forward_with_checkpoint(
        &mut service,
        &fixture.store,
        &row,
        &intent,
        LIMITS,
        TIMEOUT,
        cancellation,
        |phase| {
            if phase == ForwardCheckpoint::ReaderMoved {
                signal.cancel();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.contains("cancelled"));
    drop(service);
    assert!(recover_copy_trial_expiry(
        &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
        &fixture.store,
        &fixture.store.get(&pending.event_id).unwrap().unwrap(),
        LIMITS,
        TIMEOUT,
    )
    .unwrap()
    .is_none());
    fixture.unchanged();
}

#[test]
fn completion_refuses_changed_visible_backup_or_retained_source() {
    for changed in ["visible", "quarantine"] {
        let fixture = Fixture::new(false);
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let pending = begin_copy_trial_expiry(
            &mut service,
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        let row = fixture.store.get(&pending.event_id).unwrap().unwrap();
        let intent = CopyTrialExpiryIntent::from_event(&row).unwrap();
        assert_eq!(
            execute_forward_with_checkpoint(
                &mut service,
                &fixture.store,
                &row,
                &intent,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
                |phase| (phase != ForwardCheckpoint::TrashPublished)
                    .then_some(())
                    .ok_or_else(|| "interrupted".into()),
            )
            .unwrap_err(),
            "interrupted"
        );
        drop(service);
        let paths = intent.paths_for_scope(&fixture.scope).unwrap();
        let changed_root = if changed == "visible" {
            paths.visible_trash
        } else {
            paths.source_quarantine
        };
        fs::write(changed_root.join("SKILL.md"), "changed after publication").unwrap();
        let error = recover_copy_trial_expiry(
            &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
            &fixture.store,
            &fixture.store.get(&pending.event_id).unwrap().unwrap(),
            LIMITS,
            TIMEOUT,
        )
        .unwrap_err();
        assert!(error.contains("changed"), "{changed}: {error}");
        assert_eq!(
            fixture
                .store
                .get(&pending.event_id)
                .unwrap()
                .unwrap()
                .status,
            "pending"
        );
    }
}

#[test]
fn legacy_shared_trash_permissions_support_expiry_and_restart_recovery() {
    use std::os::unix::fs::PermissionsExt;
    for interrupted in [false, true] {
        let fixture = Fixture::new(true);
        let trash = fixture.scope.home.join(".agents/skills-trash");
        fs::create_dir(&trash).unwrap();
        fs::set_permissions(&trash, fs::Permissions::from_mode(0o755)).unwrap();
        if interrupted {
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            let pending = begin_copy_trial_expiry(
                &mut service,
                &fixture.store,
                &fixture.request,
                fixture.now,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
            )
            .unwrap();
            drop(service);
            let row = fixture.store.get(&pending.event_id).unwrap().unwrap();
            assert!(recover_copy_trial_expiry(
                &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
                &fixture.store,
                &row,
                LIMITS,
                TIMEOUT
            )
            .unwrap()
            .is_none());
            fixture.unchanged();
            assert_eq!(
                fixture.store.get(&row.id).unwrap().unwrap().status,
                "failed"
            );
        } else {
            let receipt = expire_copy_trial(
                &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
                &fixture.store,
                &fixture.request,
                fixture.now,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
            )
            .unwrap();
            assert_eq!(
                fixture
                    .store
                    .get(&receipt.event_id)
                    .unwrap()
                    .unwrap()
                    .status,
                "done"
            );
            assert_eq!(
                fs::read_to_string(receipt.visible_trash.join("SKILL.md")).unwrap(),
                DOCUMENT
            );
        }
        assert_eq!(
            fs::metadata(trash).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}

#[test]
fn completion_refuses_private_parent_permission_and_symlink_substitution() {
    use std::os::unix::fs::PermissionsExt;
    for artifact in ["visible", "quarantine"] {
        for replacement in [false, true] {
            let fixture = Fixture::new(true);
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            let pending = begin_copy_trial_expiry(
                &mut service,
                &fixture.store,
                &fixture.request,
                fixture.now,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
            )
            .unwrap();
            let row = fixture.store.get(&pending.event_id).unwrap().unwrap();
            let intent = CopyTrialExpiryIntent::from_event(&row).unwrap();
            assert!(execute_forward_with_checkpoint(
                &mut service,
                &fixture.store,
                &row,
                &intent,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
                |phase| {
                    (phase != ForwardCheckpoint::TrashPublished)
                        .then_some(())
                        .ok_or_else(|| "interrupted".into())
                }
            )
            .is_err());
            drop(service);
            let paths = intent.paths_for_scope(&fixture.scope).unwrap();
            let target = if artifact == "visible" {
                &paths.visible_trash
            } else {
                &paths.source_quarantine
            };
            let parent = target.parent().unwrap();
            let retained = if replacement {
                let external = fixture._temp.path().join("substituted-parent");
                fs::rename(parent, &external).unwrap();
                std::os::unix::fs::symlink(&external, parent).unwrap();
                external.join(target.file_name().unwrap())
            } else {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
                target.clone()
            };
            let registry = fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap();
            assert!(
                recover_copy_trial_expiry(
                    &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
                    &fixture.store,
                    &row,
                    LIMITS,
                    TIMEOUT
                )
                .is_err(),
                "{artifact} replacement={replacement}"
            );
            assert_eq!(
                fixture.store.get(&row.id).unwrap().unwrap().status,
                "pending"
            );
            assert_eq!(
                fs::read_to_string(retained.join("SKILL.md")).unwrap(),
                DOCUMENT
            );
            assert_eq!(
                fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
                registry
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires explicit macOS disk-image mounting"]
fn mounted_project_expiry_rolls_back_then_restores_from_home_trash() {
    use std::os::unix::fs::MetadataExt;
    use std::process::Command;

    let fixture = Fixture::new(true);
    let project = &fixture.scope.projects[0];
    let original = fixture._temp.path().join("project-before-mount");
    fs::rename(project, &original).unwrap();
    fs::create_dir(project).unwrap();
    let image = fixture._temp.path().join("trial-project.dmg");
    let created = Command::new("/usr/bin/hdiutil")
        .args([
            "create",
            "-size",
            "16m",
            "-fs",
            "HFS+",
            "-volname",
            "CopyTrialFixture",
            "-nospotlight",
        ])
        .arg(&image)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let attached = Command::new("/usr/bin/hdiutil")
        .arg("attach")
        .arg(&image)
        .args(["-nobrowse", "-noautoopen", "-owners", "on", "-mountpoint"])
        .arg(project)
        .output()
        .unwrap();
    assert!(
        attached.status.success(),
        "{}",
        String::from_utf8_lossy(&attached.stderr)
    );

    let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let copied = Command::new("/usr/bin/ditto")
            .arg(&original)
            .arg(project)
            .output()
            .unwrap();
        assert!(
            copied.status.success(),
            "{}",
            String::from_utf8_lossy(&copied.stderr)
        );
        fixture.unchanged();
        let home_device = fs::metadata(&fixture.scope.home).unwrap().dev();
        let project_device = fs::metadata(&fixture.source).unwrap().dev();
        assert_ne!(home_device, project_device);

        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        let pending = begin_copy_trial_expiry(
            &mut service,
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        let row = fixture.store.get(&pending.event_id).unwrap().unwrap();
        let intent = CopyTrialExpiryIntent::from_event(&row).unwrap();
        assert_eq!(
            execute_forward_with_checkpoint(
                &mut service,
                &fixture.store,
                &row,
                &intent,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
                |phase| {
                    (phase != ForwardCheckpoint::SourceMoved)
                        .then_some(())
                        .ok_or_else(|| "interrupted after source move".into())
                }
            )
            .unwrap_err(),
            "interrupted after source move"
        );
        drop(service);
        fixture.store.reconcile_at_startup().unwrap();
        let row = fixture.store.get(&row.id).unwrap().unwrap();
        assert_eq!(row.status, "interrupted");
        assert!(recover_copy_trial_expiry(
            &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
            &fixture.store,
            &row,
            LIMITS,
            TIMEOUT
        )
        .unwrap()
        .is_none());
        fixture.unchanged();
        assert_eq!(
            fixture.store.get(&row.id).unwrap().unwrap().status,
            "failed"
        );

        let receipt = expire_copy_trial(
            &mut ScopedSkillService::bind(fixture.scope.clone()).unwrap(),
            &fixture.store,
            &fixture.request,
            fixture.now,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        let row = fixture.store.get(&receipt.event_id).unwrap().unwrap();
        let intent = CopyTrialExpiryIntent::from_event(&row).unwrap();
        let paths = intent.paths_for_scope(&fixture.scope).unwrap();
        assert_eq!(row.status, "done");
        assert!(!row.restorable);
        assert!(!fixture.source.exists());
        assert_eq!(
            fs::metadata(&paths.source_quarantine).unwrap().dev(),
            project_device
        );
        assert_eq!(
            fs::metadata(&receipt.visible_trash).unwrap().dev(),
            home_device
        );
        assert_eq!(
            fs::read_to_string(receipt.visible_trash.join("SKILL.md")).unwrap(),
            DOCUMENT
        );
        assert_eq!(intent.selection.readers.len(), fixture.readers.len());
        assert_eq!(paths.reader_stages.len(), fixture.readers.len());
        assert_eq!(
            intent
                .selection
                .readers
                .iter()
                .map(|reader| &reader.path)
                .collect::<BTreeSet<_>>(),
            fixture.readers.iter().collect::<BTreeSet<_>>()
        );
        for reader in &fixture.readers {
            assert_eq!(
                fs::symlink_metadata(reader).unwrap_err().kind(),
                std::io::ErrorKind::NotFound
            );
        }
        for (reader, staged) in intent.selection.readers.iter().zip(&paths.reader_stages) {
            assert!(fs::symlink_metadata(&reader.path).is_err());
            assert_eq!(fs::symlink_metadata(staged).unwrap().dev(), project_device);
            assert_eq!(fs::read_link(staged).unwrap(), reader.raw_target);
        }
        let registry_path = fixture.scope.home.join(".agents/skill-studio.json");
        let registry = fs::read(&registry_path).unwrap();
        let mut expected: Value = serde_json::from_slice(&fixture.registry).unwrap();
        expected["copies"]
            .as_object_mut()
            .unwrap()
            .remove(&fixture.request.deployment_id);
        expected["trials"]
            .as_object_mut()
            .unwrap()
            .remove(&format!("deployment/{}", fixture.request.deployment_id));
        assert_eq!(
            serde_json::from_slice::<Value>(&registry).unwrap(),
            expected
        );
        let restored = crate::skill_trial_restore::restore_trial_backup(
            &fixture.scope.home,
            receipt.visible_trash.to_str().unwrap(),
            LIMITS,
        )
        .unwrap();
        assert_eq!(fs::metadata(&restored.target).unwrap().dev(), home_device);
        assert_eq!(
            fs::read_to_string(restored.target.join("SKILL.md")).unwrap(),
            DOCUMENT
        );
        assert_eq!(fs::read(&registry_path).unwrap(), registry);
        assert_eq!(fixture.event_count(), 2);
        println!("mounted project device={project_device}; home Trash device={home_device}; source-move restart rollback, expiry and unmanaged restore passed");
    }));
    let detached = Command::new("/usr/bin/hdiutil")
        .arg("detach")
        .arg(project)
        .output()
        .unwrap();
    assert!(
        detached.status.success(),
        "{}",
        String::from_utf8_lossy(&detached.stderr)
    );
    println!("mounted trial fixture detached");
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}
