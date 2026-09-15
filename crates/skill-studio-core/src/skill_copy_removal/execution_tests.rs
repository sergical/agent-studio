use super::*;
use crate::{skill_deployment::deployment_id, skill_service::SkillScope};
use std::{
    cell::Cell,
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
};

const DOCUMENT: &str = "---\nname: sample\ndescription: Removal fixture\n---\nKeep these bytes.\n";
const LIMITS: BackupCopyLimits = BackupCopyLimits {
    max_bytes: 1024 * 1024,
    max_entries: 100,
    max_depth: 8,
};
const TIMEOUT: Option<Duration> = Some(Duration::from_secs(5));

struct Fixture {
    _temp: tempfile::TempDir,
    scope: SkillScope,
    source: PathBuf,
    readers: Vec<PathBuf>,
    request: CopyRemovalRequest,
    store: EventStore,
    registry: Vec<u8>,
}

impl Fixture {
    fn new(project: bool, independent: bool, disabled: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let project_path = root.join("project");
        fs::create_dir_all(home.join(".git")).unwrap();
        fs::create_dir_all(project_path.join(".git")).unwrap();
        let base = if project { &project_path } else { &home };
        let skill_root = base.join(if independent {
            ".cursor/skills"
        } else {
            ".agents/skills"
        });
        let source = if disabled {
            skill_root.join(".skill-studio-disabled/sample")
        } else {
            skill_root.join("sample")
        };
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("SKILL.md"), DOCUMENT).unwrap();
        fs::write(source.join("resource.txt"), "retained resource").unwrap();
        let readers = if independent {
            vec![]
        } else {
            [".claude/skills/sample", ".codex/skills/sample"]
                .map(|path| {
                    let path = base.join(path);
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::os::unix::fs::symlink("../../.agents/skills/sample", &path).unwrap();
                    path
                })
                .to_vec()
        };
        let install_scope = if project {
            InstallScope::Project
        } else {
            InstallScope::Global
        };
        let scope_name = if project { "project" } else { "global" };
        let destination = if independent {
            SkillDestination::PerHarness
        } else {
            SkillDestination::Universal
        };
        let slot = if independent { "cursor" } else { "universal" };
        let project_string = project.then(|| project_path.to_str().unwrap().to_string());
        let read_scope = SkillReadScope::bind(std::slice::from_ref(&source)).unwrap();
        let record = CopyDeploymentRecord {
            deployment_id: deployment_id(
                "sample",
                scope_name,
                destination,
                slot,
                project_string.as_deref(),
                &source,
            ),
            name: "sample".into(),
            path: source.clone(),
            scope: install_scope,
            destination,
            slot: slot.into(),
            project_path: project_string.clone(),
            content_hash: crate::skill_discovery::live_skill_content_hash(&read_scope, &source)
                .unwrap(),
            disabled,
        };
        let mut raw = serde_json::to_value(&record).unwrap();
        raw["future_copy_field"] = serde_json::json!({"preserve": true});
        let registry = serde_json::to_vec(&serde_json::json!({
            "version": 4, "future": {"preserve": [1, 2, 3]},
            "copies": { record.deployment_id.clone(): raw },
            "trials": { "selected-trial": {
                "deployment_id": record.deployment_id, "started_at": "2026-09-14T00:00:00Z",
                "expires_at": "2099-09-15T00:00:00Z", "method": "copy", "scope": scope_name,
                "project_path": project_string, "skill_dir": source, "future_trial_field": true
            }}
        }))
        .unwrap();
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(home.join(".agents/skill-studio.json"), &registry).unwrap();
        let request = CopyRemovalRequest {
            deployment_id: record.deployment_id.clone(),
            expected_owner_revision: RegistryOwnerRecord::Copy(&record).revision().unwrap(),
        };
        let scope = SkillScope {
            home,
            projects: if project { vec![project_path] } else { vec![] },
            backing_roots: vec![],
            plugin_ownership_roots: if project { vec![root.clone()] } else { vec![] },
        };
        let inventory = ScopedSkillService::bind(scope.clone())
            .unwrap()
            .scan(None, TIMEOUT)
            .unwrap();
        let actual = inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .find(|deployment| deployment.id == request.deployment_id)
            .unwrap();
        assert_eq!(
            actual.owner_kind,
            LifecycleOwnerKind::Copy,
            "fixture ownership: {actual:#?}; failures {:?}",
            inventory.ownership.failures()
        );
        assert_eq!(
            actual.owner_revision.as_deref(),
            Some(request.expected_owner_revision.as_str())
        );
        let store = EventStore::open(&root.join("events")).unwrap();
        Self {
            _temp: temp,
            scope,
            source,
            readers,
            request,
            store,
            registry,
        }
    }

    fn service(&self) -> ScopedSkillService {
        ScopedSkillService::bind(self.scope.clone()).unwrap()
    }

    fn admit(&self) -> (EventRow, CopyRemovalIntent) {
        let mut service = self.service();
        let id = crate::skill_event_store::allocate_id();
        let prepared = service
            .prepare_copy_removal(
                &self.store,
                &id,
                &self.request,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
            )
            .unwrap();
        let intent = prepared.intent.clone();
        GuardedEventStore::bind(&self.store, &prepared.lease)
            .unwrap()
            .record_pending(&prepared.lease, &id, intent.event_draft().unwrap())
            .unwrap();
        drop(prepared);
        (self.store.get(&id).unwrap().unwrap(), intent)
    }

    fn recover(&self, row: &EventRow) -> Result<bool, String> {
        recover_copy_removal(&mut self.service(), &self.store, row, LIMITS, TIMEOUT)
    }

    fn assert_restored(&self, row: &EventRow) {
        assert_eq!(self.store.get(&row.id).unwrap().unwrap().status, "failed");
        assert_eq!(
            fs::read_to_string(self.source.join("SKILL.md")).unwrap(),
            DOCUMENT
        );
        assert_eq!(
            fs::read_to_string(self.source.join("resource.txt")).unwrap(),
            "retained resource"
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
}

#[test]
fn read_only_parent_preserves_copy_bytes_and_ownership_before_retry() {
    for project in [false, true] {
        for independent in [false, true] {
            let fixture = Fixture::new(project, independent, false);
            let parent = fixture.source.parent().unwrap();
            let permissions = fs::metadata(parent).unwrap().permissions();
            fs::set_permissions(parent, fs::Permissions::from_mode(0o555)).unwrap();
            let permission_probe = fs::write(parent.join("permission-probe"), "probe");
            let result = remove_copy_deployment(
                &mut fixture.service(),
                &fixture.store,
                &fixture.request,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
            );
            fs::set_permissions(parent, permissions).unwrap();

            assert_eq!(
                permission_probe.unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied,
                "the fixture must enforce the native permission failure"
            );
            let error = result.unwrap_err();
            assert!(!error.recovery_required, "{error:?}");
            let events = fixture.store.list(2, None).unwrap();
            assert!(
                events.len() <= 1,
                "one removal must not create duplicate events"
            );
            assert!(
                events.iter().all(|row| row.status == "failed"),
                "{events:?}"
            );
            assert_eq!(
                fs::read(fixture.source.join("SKILL.md")).unwrap(),
                DOCUMENT.as_bytes()
            );
            assert_eq!(
                fs::read_to_string(fixture.source.join("resource.txt")).unwrap(),
                "retained resource"
            );
            assert_eq!(
                fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
                fixture.registry
            );
            for reader in &fixture.readers {
                assert_eq!(
                    fs::read_link(reader).unwrap(),
                    PathBuf::from("../../.agents/skills/sample")
                );
            }

            let outcome = remove_copy_deployment(
                &mut fixture.service(),
                &fixture.store,
                &fixture.request,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
            )
            .unwrap();
            assert!(!fixture.source.exists());
            let row = fixture.store.get(&outcome.event_id).unwrap().unwrap();
            assert_eq!(row.status, "done");
            let intent = CopyRemovalIntent::from_event(&row).unwrap();
            assert_eq!(
                fs::read(intent.quarantine_path().join("SKILL.md")).unwrap(),
                DOCUMENT.as_bytes()
            );
        }
    }
}

#[test]
fn removes_global_project_universal_independent_and_disabled_copies() {
    for project in [false, true] {
        for (independent, disabled) in [(false, false), (true, false), (true, true)] {
            let fixture = Fixture::new(project, independent, disabled);
            let outcome = remove_copy_deployment(
                &mut fixture.service(),
                &fixture.store,
                &fixture.request,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
            )
            .unwrap();
            assert!(!fixture.source.exists());
            for reader in &fixture.readers {
                assert!(fs::symlink_metadata(reader).is_err());
            }
            let row = fixture.store.get(&outcome.event_id).unwrap().unwrap();
            assert_eq!(row.status, "done");
            let intent = CopyRemovalIntent::from_event(&row).unwrap();
            let holding = intent.quarantine_path().parent().unwrap();
            assert_eq!(
                fs::metadata(holding).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(holding).unwrap().dev(),
                fs::metadata(holding.parent().unwrap()).unwrap().dev()
            );
            assert_eq!(
                fs::read_to_string(intent.quarantine_path().join("SKILL.md")).unwrap(),
                DOCUMENT
            );
            assert_eq!(
                fs::read_to_string(intent.quarantine_path().join("resource.txt")).unwrap(),
                "retained resource"
            );
            let registry: serde_json::Value =
                serde_json::from_slice(&fs::read(intent.registry_path()).unwrap()).unwrap();
            assert!(registry["copies"].as_object().unwrap().is_empty());
            assert!(registry["trials"].as_object().unwrap().is_empty());
            assert_eq!(
                registry["future"],
                serde_json::json!({"preserve": [1, 2, 3]})
            );
        }
    }
}

#[test]
fn recovers_each_prepublication_checkpoint_without_changing_registry_bytes() {
    for project in [false, true] {
        for phase in 0usize..5 {
            let fixture = Fixture::new(project, false, false);
            let (row, intent) = fixture.admit();
            let holding = intent.quarantine_path().parent().unwrap();
            if phase >= 1 {
                fs::create_dir(holding).unwrap();
                fs::set_permissions(holding, fs::Permissions::from_mode(0o700)).unwrap();
            }
            for index in 0..phase.saturating_sub(1).min(2) {
                fs::rename(
                    &fixture.readers[index],
                    holding.join(format!("{}-reader-{index}", row.id)),
                )
                .unwrap();
            }
            if phase == 4 {
                fs::rename(&fixture.source, intent.quarantine_path()).unwrap();
            }
            assert!(
                !fixture.recover(&row).unwrap(),
                "project={project}, phase={phase}"
            );
            fixture.assert_restored(&row);
        }
    }
}

#[test]
fn rollback_preserves_reader_replacement_restores_other_paths_and_can_retry() {
    let fixture = Fixture::new(true, false, false);
    let (row, intent) = fixture.admit();
    let holding = intent.quarantine_path().parent().unwrap();
    fs::create_dir(holding).unwrap();
    fs::set_permissions(holding, fs::Permissions::from_mode(0o700)).unwrap();
    for (index, reader) in fixture.readers.iter().enumerate() {
        fs::rename(reader, holding.join(format!("{}-reader-{index}", row.id))).unwrap();
    }
    fs::rename(&fixture.source, intent.quarantine_path()).unwrap();
    fs::write(&fixture.readers[0], "replacement").unwrap();
    assert!(fixture.recover(&row).is_err());
    assert_eq!(
        fs::read_to_string(&fixture.readers[0]).unwrap(),
        "replacement"
    );
    assert_eq!(
        fs::read_to_string(fixture.source.join("SKILL.md")).unwrap(),
        DOCUMENT
    );
    assert_eq!(
        fs::read_link(&fixture.readers[1]).unwrap(),
        PathBuf::from("../../.agents/skills/sample")
    );
    fs::remove_file(&fixture.readers[0]).unwrap();
    assert!(!fixture.recover(&row).unwrap());
    fixture.assert_restored(&row);
}

#[test]
fn admission_refuses_stale_owner_content_and_an_unsafe_holding_directory() {
    for change in ["owner", "content", "holding_link", "holding_permissions"] {
        let mut fixture = Fixture::new(false, true, false);
        match change {
            "owner" => fixture.request.expected_owner_revision.push('x'),
            "content" => fs::write(fixture.source.join("SKILL.md"), "external edit").unwrap(),
            "holding_link" => {
                let outside = fixture.scope.home.join("outside");
                fs::create_dir(&outside).unwrap();
                std::os::unix::fs::symlink(
                    outside,
                    fixture.scope.home.join(".cursor/.skill-studio-removing"),
                )
                .unwrap();
            }
            "holding_permissions" => {
                let holding = fixture.scope.home.join(".cursor/.skill-studio-removing");
                fs::create_dir(&holding).unwrap();
                fs::set_permissions(&holding, fs::Permissions::from_mode(0o755)).unwrap();
            }
            _ => unreachable!(),
        }
        let before = fs::read(fixture.source.join("SKILL.md")).unwrap();
        let error = remove_copy_deployment(
            &mut fixture.service(),
            &fixture.store,
            &fixture.request,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap_err();
        assert!(error.event_id.is_none(), "{change}: {error:?}");
        assert_eq!(fs::read(fixture.source.join("SKILL.md")).unwrap(), before);
        assert_eq!(
            fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
            fixture.registry
        );
        assert_eq!(
            fixture
                .store
                .conn
                .query_row("SELECT count(*) FROM events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn forward_publication_preserves_unrelated_registry_drift_after_a_reader_move() {
    let fixture = Fixture::new(false, false, false);
    let (row, intent) = fixture.admit();
    let changed = Cell::new(false);
    execute_forward_with_checkpoint(
        &mut fixture.service(),
        &fixture.store,
        &row,
        LIMITS,
        TIMEOUT,
        |checkpoint| {
            if checkpoint == ForwardCheckpoint::ReaderMoved && !changed.replace(true) {
                let mut registry: serde_json::Value = serde_json::from_slice(
                    &fs::read(intent.registry_path()).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
                registry["concurrent_unrelated"] = serde_json::json!({"preserved": true});
                fs::write(
                    intent.registry_path(),
                    serde_json::to_vec(&registry).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(changed.get());
    let registry: serde_json::Value =
        serde_json::from_slice(&fs::read(intent.registry_path()).unwrap()).unwrap();
    assert_eq!(
        registry["concurrent_unrelated"],
        serde_json::json!({"preserved": true})
    );
    assert!(registry["copies"].as_object().unwrap().is_empty());
    assert_eq!(fixture.store.get(&row.id).unwrap().unwrap().status, "done");
}

#[test]
fn forward_publication_refuses_selected_registry_drift_after_the_tree_move() {
    let fixture = Fixture::new(false, true, false);
    let (row, intent) = fixture.admit();
    let drifted = std::cell::RefCell::new(None);
    let error = execute_forward_with_checkpoint(
        &mut fixture.service(),
        &fixture.store,
        &row,
        LIMITS,
        TIMEOUT,
        |checkpoint| {
            if checkpoint == ForwardCheckpoint::TreeMoved && drifted.borrow().is_none() {
                let mut registry: serde_json::Value = serde_json::from_slice(
                    &fs::read(intent.registry_path()).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
                registry["copies"][&fixture.request.deployment_id]["disabled"] =
                    serde_json::json!(true);
                let bytes = serde_json::to_vec(&registry).map_err(|error| error.to_string())?;
                fs::write(intent.registry_path(), &bytes).map_err(|error| error.to_string())?;
                *drifted.borrow_mut() = Some(bytes);
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.contains("changed"), "{error}");
    assert_eq!(
        fs::read(intent.registry_path()).unwrap(),
        drifted.into_inner().unwrap()
    );
    assert!(!fixture.source.exists());
    assert!(intent.quarantine_path().join("SKILL.md").is_file());
    assert_eq!(
        fixture.store.get(&row.id).unwrap().unwrap().status,
        "pending"
    );
}

#[test]
fn forward_reprepare_refuses_holding_permissions_changed_during_a_lease_gap() {
    let fixture = Fixture::new(false, true, false);
    let (row, intent) = fixture.admit();
    let changed = Cell::new(false);
    let error = execute_forward_with_checkpoint(
        &mut fixture.service(),
        &fixture.store,
        &row,
        LIMITS,
        TIMEOUT,
        |checkpoint| {
            if checkpoint == ForwardCheckpoint::HoldingCreated && !changed.replace(true) {
                fs::set_permissions(
                    intent.quarantine_path().parent().unwrap(),
                    fs::Permissions::from_mode(0o755),
                )
                .map_err(|error| error.to_string())?;
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.contains("private directory permissions"), "{error}");
    assert!(fixture.source.join("SKILL.md").is_file());
    assert_eq!(fs::read(intent.registry_path()).unwrap(), fixture.registry);
    assert_eq!(
        fixture.store.get(&row.id).unwrap().unwrap().status,
        "pending"
    );
}

#[test]
fn successful_published_recovery_is_returned_as_removal_success() {
    let fixture = Fixture::new(false, true, false);
    fixture
        .store
        .conn
        .execute_batch(
            "CREATE TRIGGER fail_copy_removal_finalization
             BEFORE UPDATE OF status ON events
             WHEN OLD.status = 'pending' AND NEW.status = 'done'
             BEGIN SELECT RAISE(ABORT, 'injected finalization failure'); END;",
        )
        .unwrap();
    let cleared = Cell::new(false);
    let outcome = remove_copy_deployment_with_checkpoints(
        &mut fixture.service(),
        &fixture.store,
        &fixture.request,
        LIMITS,
        TIMEOUT,
        CancellationToken::default(),
        RemovalCheckpoints {
            forward: |_| Ok(()),
            after_forward_failure: || {
                fixture
                    .store
                    .conn
                    .execute_batch("DROP TRIGGER fail_copy_removal_finalization")
                    .unwrap();
                cleared.set(true);
            },
        },
    )
    .unwrap();
    assert!(cleared.get());
    assert!(!fixture.source.exists());
    assert_eq!(
        fixture
            .store
            .get(&outcome.event_id)
            .unwrap()
            .unwrap()
            .status,
        "done"
    );
    let registry: serde_json::Value = serde_json::from_slice(
        &fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
    )
    .unwrap();
    assert!(registry["copies"].as_object().unwrap().is_empty());
}

#[test]
fn published_recovery_does_not_require_the_removed_skill_root() {
    let fixture = Fixture::new(true, true, false);
    let outcome = remove_copy_deployment(
        &mut fixture.service(),
        &fixture.store,
        &fixture.request,
        LIMITS,
        TIMEOUT,
        CancellationToken::default(),
    )
    .unwrap();
    fs::remove_dir(fixture.source.parent().unwrap()).unwrap();
    fixture
        .store
        .conn
        .execute(
            "UPDATE events SET status = 'interrupted' WHERE id = ?1",
            [&outcome.event_id],
        )
        .unwrap();
    let row = fixture.store.get(&outcome.event_id).unwrap().unwrap();
    assert!(fixture.recover(&row).unwrap());
    assert!(!fixture.source.parent().unwrap().exists());
    assert_eq!(fixture.store.get(&row.id).unwrap().unwrap().status, "done");
}

#[test]
fn source_conflict_keeps_the_quarantine_and_retry_restores_the_copy() {
    let fixture = Fixture::new(false, true, false);
    let (row, intent) = fixture.admit();
    let holding = intent.quarantine_path().parent().unwrap();
    fs::create_dir(holding).unwrap();
    fs::set_permissions(holding, fs::Permissions::from_mode(0o700)).unwrap();
    fs::rename(&fixture.source, intent.quarantine_path()).unwrap();
    fs::create_dir(&fixture.source).unwrap();
    fs::write(fixture.source.join("SKILL.md"), "replacement").unwrap();
    assert!(fixture.recover(&row).is_err());
    assert_eq!(
        fs::read_to_string(fixture.source.join("SKILL.md")).unwrap(),
        "replacement"
    );
    assert_eq!(
        fs::read_to_string(intent.quarantine_path().join("SKILL.md")).unwrap(),
        DOCUMENT
    );
    assert_eq!(
        fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
        fixture.registry
    );
    fs::remove_file(fixture.source.join("SKILL.md")).unwrap();
    fs::remove_dir(&fixture.source).unwrap();
    assert!(!fixture.recover(&row).unwrap());
    fixture.assert_restored(&row);
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires explicit macOS disk-image mounting"]
fn admission_refuses_a_real_mounted_holding_filesystem() {
    use std::process::Command;

    let fixture = Fixture::new(false, true, false);
    let image = fixture._temp.path().join("holding.dmg");
    let holding = fixture.scope.home.join(".cursor/.skill-studio-removing");
    fs::create_dir(&holding).unwrap();
    let created = Command::new("/usr/bin/hdiutil")
        .args([
            "create",
            "-size",
            "16m",
            "-fs",
            "HFS+",
            "-volname",
            "CopyRemovalFixture",
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
        .arg(&holding)
        .output()
        .unwrap();
    assert!(
        attached.status.success(),
        "{}",
        String::from_utf8_lossy(&attached.stderr)
    );

    let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fs::set_permissions(&holding, fs::Permissions::from_mode(0o700)).unwrap();
        let source_device = fs::metadata(&fixture.source).unwrap().dev();
        let holding_device = fs::metadata(&holding).unwrap().dev();
        assert_ne!(source_device, holding_device);
        let sentinel = holding.join("preserve.txt");
        fs::write(&sentinel, "mounted fixture data").unwrap();
        let error = remove_copy_deployment(
            &mut fixture.service(),
            &fixture.store,
            &fixture.request,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap_err();
        assert!(error.message.contains("different filesystem"), "{error:?}");
        assert!(error.event_id.is_none());
        assert!(!error.recovery_required);
        assert_eq!(
            fs::read_to_string(fixture.source.join("SKILL.md")).unwrap(),
            DOCUMENT
        );
        assert_eq!(
            fs::read_to_string(fixture.source.join("resource.txt")).unwrap(),
            "retained resource"
        );
        assert_eq!(
            fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
            fixture.registry
        );
        assert_eq!(
            fs::read_to_string(sentinel).unwrap(),
            "mounted fixture data"
        );
        assert_eq!(
            fixture
                .store
                .conn
                .query_row("SELECT count(*) FROM events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        println!("different-device refusal: source={source_device}, holding={holding_device}; no event or live effect");
    }));
    let detached = Command::new("/usr/bin/hdiutil")
        .arg("detach")
        .arg(&holding)
        .output()
        .unwrap();
    assert!(
        detached.status.success(),
        "{}",
        String::from_utf8_lossy(&detached.stderr)
    );
    println!("mounted fixture detached");
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn publication_refuses_quarantine_and_source_changes_after_tree_move() {
    for replace_source in [false, true] {
        let fixture = Fixture::new(false, false, false);
        let (row, intent) = fixture.admit();
        let error = execute_forward_with_checkpoint(
            &mut fixture.service(),
            &fixture.store,
            &row,
            LIMITS,
            TIMEOUT,
            |checkpoint| {
                if checkpoint == ForwardCheckpoint::TreeMoved {
                    if replace_source {
                        fs::create_dir(&fixture.source).unwrap();
                        fs::write(fixture.source.join("SKILL.md"), DOCUMENT).unwrap();
                        fs::write(fixture.source.join("resource.txt"), "retained resource")
                            .unwrap();
                    } else {
                        fs::write(
                            intent.quarantine_path().join("resource.txt"),
                            "external change",
                        )
                        .unwrap();
                    }
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.contains("changed before publication"), "{error}");
        assert_eq!(fs::read(intent.registry_path()).unwrap(), fixture.registry);
        assert_eq!(
            fixture.store.get(&row.id).unwrap().unwrap().status,
            "pending"
        );
        if replace_source {
            assert_eq!(
                fs::read_to_string(fixture.source.join("SKILL.md")).unwrap(),
                DOCUMENT
            );
        } else {
            assert_eq!(
                fs::read_to_string(intent.quarantine_path().join("resource.txt")).unwrap(),
                "external change"
            );
        }
    }
}

#[test]
fn forward_refuses_new_plugin_ownership_after_admission_and_holding_creation() {
    for during_holding in [false, true] {
        let fixture = Fixture::new(false, false, false);
        let (row, _) = fixture.admit();
        let add_plugin = || {
            let manifest = fixture.scope.home.join(".agents/.claude-plugin");
            fs::create_dir_all(&manifest).unwrap();
            fs::write(manifest.join("plugin.json"), r#"{"name":"new-owner"}"#).unwrap();
        };
        if !during_holding {
            add_plugin();
        }
        let error = execute_forward_with_checkpoint(
            &mut fixture.service(),
            &fixture.store,
            &row,
            LIMITS,
            TIMEOUT,
            |checkpoint| {
                if during_holding && checkpoint == ForwardCheckpoint::HoldingCreated {
                    add_plugin();
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert!(
            error.contains("ownership changed after admission"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(fixture.source.join("SKILL.md")).unwrap(),
            DOCUMENT
        );
        assert_eq!(
            fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
            fixture.registry
        );
        assert!(fixture.readers.iter().all(|reader| reader.is_symlink()));
    }
}
