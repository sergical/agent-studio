use super::*;
use crate::{
    skill_backup_copy::BackupCopyReport, skill_deployment::deployment_id,
    skill_fork_registry::OriginTool, skill_service::SkillScope,
};
use std::fs;

const DOCUMENT: &str = "---\nname: sample\ndescription: Removal fixture\n---\nOriginal fork.\n";
const LIMITS: BackupCopyLimits = BackupCopyLimits {
    max_bytes: 1024 * 1024,
    max_entries: 100,
    max_depth: 8,
};
const TIMEOUT: Option<Duration> = Some(Duration::from_secs(5));

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    scope: SkillScope,
    source: PathBuf,
    request: ForkRemovalRequest,
    selected: ForkRecord,
    registry: Vec<u8>,
    original_tree: BackupCopyReport,
}

impl Fixture {
    fn new(origin_tool: OriginTool, legacy: bool) -> Self {
        Self::with_temp(origin_tool, legacy, tempfile::tempdir().unwrap())
    }

    fn with_temp(origin_tool: OriginTool, legacy: bool, temp: tempfile::TempDir) -> Self {
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let project = root.join("project");
        let backing = root.join("backing");
        let plugin = root.join("plugin");
        let source = home.join(".agents/skills/sample");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(home.join(".git")).unwrap();
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(&backing).unwrap();
        fs::create_dir_all(&plugin).unwrap();
        fs::write(source.join("SKILL.md"), DOCUMENT).unwrap();
        fs::write(source.join("resource.txt"), "retained resource").unwrap();
        let id = deployment_id(
            "sample",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &source,
        );
        let selected = ForkRecord {
            deployment_id: if legacy { String::new() } else { id.clone() },
            skill_dir: if legacy {
                PathBuf::new()
            } else {
                source.clone()
            },
            forked_at: "2026-09-16T00:00:00Z".into(),
            origin_tool,
            origin_source: "owner/repo".into(),
            repo: "owner/repo".into(),
            path: "skills/sample".into(),
            declared_ref: (origin_tool == OriginTool::Dotagents).then(|| "main".into()),
            base_commit: "a".repeat(40),
        };
        let mut raw = serde_json::to_value(&selected).unwrap();
        raw["future_fork_field"] = serde_json::json!({"preserve": true});
        let sibling_record = ForkRecord {
            deployment_id: "sibling-id".into(),
            skill_dir: home.join(".agents/skills/sibling"),
            forked_at: "2026-09-16T00:00:00Z".into(),
            origin_tool: OriginTool::SkillsSh,
            origin_source: "owner/sibling".into(),
            repo: "owner/sibling".into(),
            path: "skills/sibling".into(),
            declared_ref: None,
            base_commit: "b".repeat(40),
        };
        let mut sibling_raw = serde_json::to_value(&sibling_record).unwrap();
        sibling_raw["future_sibling_field"] = serde_json::json!({"preserve": true});
        let trial = |deployment_id: &str, skill_dir: &Path| {
            serde_json::json!({
                "deployment_id": deployment_id,
                "started_at": "2026-09-16T00:00:00Z",
                "expires_at": "2099-09-16T00:00:00Z",
                "method": "skills-sh",
                "scope": "global",
                "skill_dir": skill_dir,
                "deployment_fingerprint": "fixture"
            })
        };
        let registry = serde_json::to_vec(&serde_json::json!({
            "version": 4,
            "future": {"preserve": [1, 2, 3]},
            "forks": {
                "sample": raw,
                "sibling": sibling_raw
            },
            "trials": {
                format!("deployment/{id}"): trial(&id, &source),
                "global/sample": trial("", &source),
                "global/sibling": trial("sibling-id", &home.join(".agents/skills/sibling"))
            }
        }))
        .unwrap();
        fs::write(home.join(".agents/skill-studio.json"), &registry).unwrap();
        let request = ForkRemovalRequest {
            deployment_id: id,
            expected_owner_revision: RegistryOwnerRecord::Fork(&selected).revision().unwrap(),
        };
        let scope = SkillScope {
            home,
            projects: vec![project],
            backing_roots: vec![backing],
            plugin_ownership_roots: vec![plugin],
        };
        let original_tree = inspect_path(&source);
        let inventory = ScopedSkillService::bind(scope.clone())
            .unwrap()
            .scan(None, TIMEOUT)
            .unwrap();
        let deployment = inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .find(|deployment| deployment.id == request.deployment_id)
            .unwrap();
        assert_eq!(deployment.owner_kind, LifecycleOwnerKind::Fork);
        assert_eq!(
            deployment.owner_revision.as_deref(),
            Some(request.expected_owner_revision.as_str())
        );
        Self {
            _temp: temp,
            root,
            scope,
            source,
            request,
            selected,
            registry,
            original_tree,
        }
    }

    fn store_path(&self) -> PathBuf {
        self.root.join("events")
    }

    fn store(&self) -> EventStore {
        EventStore::open(&self.store_path()).unwrap()
    }

    fn service(&self) -> ScopedSkillService {
        ScopedSkillService::bind(self.scope.clone()).unwrap()
    }

    fn admit(&self, store: &EventStore) -> (EventRow, ForkRemovalIntent) {
        let mut service = self.service();
        let id = crate::skill_event_store::allocate_id();
        let prepared = prepare(
            &mut service,
            store,
            &id,
            Some(&self.request),
            None,
            LIMITS,
            TIMEOUT,
            CancellationToken::default(),
        )
        .unwrap();
        let intent = prepared.intent.clone();
        GuardedEventStore::bind(store, &prepared.lease)
            .unwrap()
            .record_pending(&prepared.lease, &id, intent.event_draft().unwrap())
            .unwrap();
        drop(prepared);
        (store.get(&id).unwrap().unwrap(), intent)
    }
}

fn inspect_path(path: &Path) -> BackupCopyReport {
    let source = BackupSourceRoot::bind(path.parent().unwrap())
        .unwrap()
        .select(path.file_name().unwrap())
        .unwrap();
    inspect_entry(
        &source.directory,
        &source.name,
        LIMITS,
        &CancellationToken::default(),
    )
    .unwrap()
}

fn write_registry_value(fixture: &Fixture, update: impl FnOnce(&mut serde_json::Value)) {
    let path = fixture.scope.home.join(".agents/skill-studio.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    update(&mut document);
    fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
}

#[test]
fn removes_current_and_legacy_forks_from_both_origins() {
    for origin in [OriginTool::SkillsSh, OriginTool::Dotagents] {
        for legacy in [false, true] {
            let fixture = Fixture::new(origin, legacy);
            let store = fixture.store();
            let outcome = remove_fork_deployment(
                &mut fixture.service(),
                &store,
                &fixture.request,
                LIMITS,
                TIMEOUT,
                CancellationToken::default(),
            )
            .unwrap();
            assert_eq!(
                outcome.removed_deployment_ids.as_slice(),
                std::slice::from_ref(&fixture.request.deployment_id)
            );
            assert!(!fixture.source.exists());
            let row = store.get(&outcome.event_id).unwrap().unwrap();
            assert_eq!(row.status, "done");
            assert!(!row.restorable);
            assert!(row.inverse.is_none());
            assert!(row.backup_dir.is_none());
            let intent = ForkRemovalIntent::from_event(&row).unwrap();
            assert_eq!(intent.selected.deployment_id, fixture.request.deployment_id);
            assert_eq!(intent.selected.skill_dir, fixture.source);
            assert_eq!(inspect_path(&intent.quarantine_path), fixture.original_tree);
            assert!(intent
                .configured_roots
                .contains(&fixture.scope.backing_roots[0]));
            assert!(intent
                .configured_roots
                .contains(&fixture.scope.plugin_ownership_roots[0]));
            assert!(intent
                .configured_roots
                .iter()
                .any(|root| root.starts_with(&fixture.scope.projects[0])));
            let registry: serde_json::Value = serde_json::from_slice(
                &fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
            )
            .unwrap();
            assert!(registry["forks"].get("sample").is_none());
            assert_eq!(
                registry["forks"]["sibling"]["future_sibling_field"],
                serde_json::json!({"preserve": true})
            );
            assert!(registry["trials"]
                .get(format!("deployment/{}", fixture.request.deployment_id))
                .is_none());
            assert!(registry["trials"].get("global/sample").is_none());
            assert!(registry["trials"].get("global/sibling").is_some());
            assert_eq!(
                registry["future"],
                serde_json::json!({"preserve": [1, 2, 3]})
            );
            assert!(!store.app_data.join("backups").exists());
        }
    }
}

#[test]
fn reopen_recovers_each_prepublication_checkpoint() {
    let recorded = Fixture::new(OriginTool::SkillsSh, false);
    let store = recorded.store();
    let (row, _) = recorded.admit(&store);
    drop(store);
    let reopened = recorded.store();
    let pending_row = reopened.get(&row.id).unwrap().unwrap();
    assert!(!recover_fork_removal(
        &mut recorded.service(),
        &reopened,
        &pending_row,
        LIMITS,
        TIMEOUT,
    )
    .unwrap());
    let terminal = reopened.get(&row.id).unwrap().unwrap();
    assert_eq!(terminal.status, "failed");
    assert!(recover_fork_removal(
        &mut recorded.service(),
        &reopened,
        &terminal,
        LIMITS,
        TIMEOUT,
    )
    .is_err());
    assert_eq!(inspect_path(&recorded.source), recorded.original_tree);
    assert_eq!(
        fs::read(recorded.scope.home.join(".agents/skill-studio.json")).unwrap(),
        recorded.registry
    );

    for stop in [
        ForwardCheckpoint::HoldingCreated,
        ForwardCheckpoint::TreeMoved,
    ] {
        let fixture = Fixture::new(OriginTool::SkillsSh, false);
        let store = fixture.store();
        let (row, _) = fixture.admit(&store);
        let error = execute_forward_with_checkpoint(
            &mut fixture.service(),
            &store,
            &row,
            LIMITS,
            TIMEOUT,
            |checkpoint| {
                (checkpoint != stop)
                    .then_some(())
                    .ok_or("injected stop".into())
            },
        )
        .unwrap_err();
        assert_eq!(error, "injected stop");
        drop(store);

        let reopened = fixture.store();
        let interrupted = reopened.get(&row.id).unwrap().unwrap();
        assert_eq!(interrupted.status, "pending");
        let recovered = recover_fork_removal(
            &mut fixture.service(),
            &reopened,
            &interrupted,
            LIMITS,
            TIMEOUT,
        );
        assert_eq!(recovered, Ok(false), "checkpoint={stop:?}");
        assert_eq!(reopened.get(&row.id).unwrap().unwrap().status, "failed");
        assert_eq!(inspect_path(&fixture.source), fixture.original_tree);
        assert_eq!(
            fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
            fixture.registry
        );
    }
}

#[test]
fn published_reopen_ignores_and_preserves_a_replacement() {
    let fixture = Fixture::new(OriginTool::Dotagents, true);
    let store = fixture.store();
    let (row, intent) = fixture.admit(&store);
    let error = execute_forward_with_checkpoint(
        &mut fixture.service(),
        &store,
        &row,
        LIMITS,
        TIMEOUT,
        |checkpoint| {
            (checkpoint != ForwardCheckpoint::RegistryPublished)
                .then_some(())
                .ok_or("published stop".into())
        },
    )
    .unwrap_err();
    assert_eq!(error, "published stop");
    fs::create_dir(&fixture.source).unwrap();
    fs::write(fixture.source.join("SKILL.md"), "replacement owner").unwrap();
    drop(store);

    let reopened = fixture.store();
    let interrupted = reopened.get(&row.id).unwrap().unwrap();
    assert!(recover_fork_removal(
        &mut fixture.service(),
        &reopened,
        &interrupted,
        LIMITS,
        TIMEOUT,
    )
    .unwrap());
    assert_eq!(reopened.get(&row.id).unwrap().unwrap().status, "done");
    assert_eq!(
        fs::read_to_string(fixture.source.join("SKILL.md")).unwrap(),
        "replacement owner"
    );
    assert_eq!(inspect_path(&intent.quarantine_path), fixture.original_tree);
}

#[test]
fn prepublication_replacement_keeps_both_trees_and_the_event_unresolved() {
    let fixture = Fixture::new(OriginTool::SkillsSh, false);
    let store = fixture.store();
    let (row, intent) = fixture.admit(&store);
    execute_forward_with_checkpoint(
        &mut fixture.service(),
        &store,
        &row,
        LIMITS,
        TIMEOUT,
        |checkpoint| {
            (checkpoint != ForwardCheckpoint::TreeMoved)
                .then_some(())
                .ok_or("tree stop".into())
        },
    )
    .unwrap_err();
    fs::create_dir(&fixture.source).unwrap();
    fs::write(fixture.source.join("SKILL.md"), "replacement owner").unwrap();
    drop(store);

    let reopened = fixture.store();
    let interrupted = reopened.get(&row.id).unwrap().unwrap();
    assert!(recover_fork_removal(
        &mut fixture.service(),
        &reopened,
        &interrupted,
        LIMITS,
        TIMEOUT,
    )
    .is_err());
    assert_eq!(
        fs::read_to_string(fixture.source.join("SKILL.md")).unwrap(),
        "replacement owner"
    );
    assert_eq!(inspect_path(&intent.quarantine_path), fixture.original_tree);
    assert_eq!(reopened.get(&row.id).unwrap().unwrap().status, "pending");
    assert_eq!(
        fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
        fixture.registry
    );
}

#[test]
fn registry_change_after_tree_move_preserves_quarantine_and_registry() {
    let fixture = Fixture::new(OriginTool::SkillsSh, false);
    let store = fixture.store();
    let (row, intent) = fixture.admit(&store);
    let error = execute_forward_with_checkpoint(
        &mut fixture.service(),
        &store,
        &row,
        LIMITS,
        TIMEOUT,
        |checkpoint| {
            if checkpoint == ForwardCheckpoint::TreeMoved {
                write_registry_value(&fixture, |registry| {
                    registry["forks"]["sample"]["base_commit"] = serde_json::json!("changed");
                });
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.contains("ownership changed"), "{error}");
    assert!(!fixture.source.exists());
    assert_eq!(inspect_path(&intent.quarantine_path), fixture.original_tree);
    let changed = fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap();
    assert_ne!(changed, fixture.registry);
    assert!(recover_fork_removal(
        &mut fixture.service(),
        &store,
        &store.get(&row.id).unwrap().unwrap(),
        LIMITS,
        TIMEOUT,
    )
    .is_err());
    assert_eq!(
        fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
        changed
    );
    assert_eq!(store.get(&row.id).unwrap().unwrap().status, "pending");
}

#[test]
fn changed_missing_or_new_matching_trial_refuses_effects() {
    for change in ["changed", "missing", "new"] {
        let fixture = Fixture::new(OriginTool::SkillsSh, false);
        let store = fixture.store();
        let (row, _) = fixture.admit(&store);
        let trial_key = format!("deployment/{}", fixture.request.deployment_id);
        write_registry_value(&fixture, |registry| match change {
            "changed" => {
                registry["trials"][&trial_key]["expires_at"] = serde_json::json!("changed");
            }
            "missing" => {
                registry["trials"]
                    .as_object_mut()
                    .unwrap()
                    .remove(&trial_key);
            }
            "new" => {
                let new_value = registry["trials"][&trial_key].clone();
                registry["trials"]["new-target-trial"] = new_value;
            }
            _ => unreachable!(),
        });
        assert!(execute_forward_with_checkpoint(
            &mut fixture.service(),
            &store,
            &row,
            LIMITS,
            TIMEOUT,
            |_| Ok(()),
        )
        .is_err());
        assert_eq!(inspect_path(&fixture.source), fixture.original_tree);
        assert!(!fixture
            .scope
            .home
            .join(".agents/.skill-studio-removing")
            .exists());
        assert_eq!(store.get(&row.id).unwrap().unwrap().status, "pending");
    }
}

#[test]
fn published_recovery_refuses_a_new_matching_trial() {
    let fixture = Fixture::new(OriginTool::SkillsSh, false);
    let store = fixture.store();
    let (row, intent) = fixture.admit(&store);
    execute_forward_with_checkpoint(
        &mut fixture.service(),
        &store,
        &row,
        LIMITS,
        TIMEOUT,
        |checkpoint| {
            if checkpoint == ForwardCheckpoint::RegistryPublished {
                write_registry_value(&fixture, |registry| {
                    registry["trials"]["new-target-trial"] = serde_json::json!({
                        "deployment_id": fixture.request.deployment_id,
                        "started_at": "2026-09-16T00:00:00Z",
                        "expires_at": "2099-09-16T00:00:00Z",
                        "method": "skills-sh",
                        "scope": "global",
                        "skill_dir": fixture.source,
                        "deployment_fingerprint": "replacement"
                    });
                });
                return Err("published stop".into());
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(recover_fork_removal(
        &mut fixture.service(),
        &store,
        &store.get(&row.id).unwrap().unwrap(),
        LIMITS,
        TIMEOUT,
    )
    .is_err());
    assert!(!fixture.source.exists());
    assert_eq!(inspect_path(&intent.quarantine_path), fixture.original_tree);
    assert_eq!(store.get(&row.id).unwrap().unwrap().status, "pending");
}

#[test]
fn recovery_checks_the_persisted_event_before_restoring_the_tree() {
    let fixture = Fixture::new(OriginTool::Dotagents, false);
    let store = fixture.store();
    let (row, intent) = fixture.admit(&store);
    execute_forward_with_checkpoint(
        &mut fixture.service(),
        &store,
        &row,
        LIMITS,
        TIMEOUT,
        |checkpoint| {
            (checkpoint != ForwardCheckpoint::TreeMoved)
                .then_some(())
                .ok_or("tree stop".into())
        },
    )
    .unwrap_err();
    let mut changed = row.payload.clone();
    changed["expected_tree"] = serde_json::json!(format!("tree-v1:{}", "b".repeat(64)));
    store
        .conn
        .execute(
            "UPDATE events SET payload = ?1 WHERE id = ?2",
            [serde_json::to_string(&changed).unwrap(), row.id.clone()],
        )
        .unwrap();

    assert!(recover_fork_removal(&mut fixture.service(), &store, &row, LIMITS, TIMEOUT,).is_err());
    assert!(!fixture.source.exists());
    assert_eq!(inspect_path(&intent.quarantine_path), fixture.original_tree);
    assert_eq!(
        fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
        fixture.registry
    );
}

#[test]
fn event_tampering_and_current_root_changes_are_refused_without_effects() {
    for field in [
        "path",
        "quarantine_path",
        "expected_tree",
        "configured_roots",
    ] {
        let fixture = Fixture::new(OriginTool::SkillsSh, false);
        let store = fixture.store();
        let (row, _) = fixture.admit(&store);
        let mut payload = row.payload.clone();
        match field {
            "path" => payload[field] = serde_json::json!(fixture.root.join("outside/sample")),
            "quarantine_path" => {
                payload[field] = serde_json::json!(fixture.root.join("outside").join(&row.id))
            }
            "expected_tree" => payload[field] = serde_json::json!("malformed"),
            "configured_roots" => payload[field].as_array_mut().unwrap().reverse(),
            _ => unreachable!(),
        }
        store
            .conn
            .execute(
                "UPDATE events SET payload = ?1 WHERE id = ?2",
                [serde_json::to_string(&payload).unwrap(), row.id.clone()],
            )
            .unwrap();
        let tampered = store.get(&row.id).unwrap().unwrap();
        assert!(
            recover_fork_removal(&mut fixture.service(), &store, &tampered, LIMITS, TIMEOUT,)
                .is_err()
        );
        assert_eq!(inspect_path(&fixture.source), fixture.original_tree);
        assert_eq!(
            fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
            fixture.registry
        );
    }

    let fixture = Fixture::new(OriginTool::Dotagents, false);
    let store = fixture.store();
    let (row, _) = fixture.admit(&store);
    let mut changed_scope = fixture.scope.clone();
    changed_scope
        .backing_roots
        .push(fixture.root.join("new-backing"));
    fs::create_dir_all(changed_scope.backing_roots.last().unwrap()).unwrap();
    let mut changed_service = ScopedSkillService::bind(changed_scope).unwrap();
    assert!(recover_fork_removal(&mut changed_service, &store, &row, LIMITS, TIMEOUT,).is_err());
    assert_eq!(inspect_path(&fixture.source), fixture.original_tree);
}

#[test]
fn event_change_after_holding_creation_is_checked_before_tree_move() {
    let fixture = Fixture::new(OriginTool::SkillsSh, false);
    let store = fixture.store();
    let (row, _) = fixture.admit(&store);
    let error = execute_forward_with_checkpoint(
        &mut fixture.service(),
        &store,
        &row,
        LIMITS,
        TIMEOUT,
        |checkpoint| {
            if checkpoint == ForwardCheckpoint::HoldingCreated {
                let mut payload = store.get(&row.id).unwrap().unwrap().payload;
                payload["expected_tree"] = serde_json::json!(format!("tree-v1:{}", "b".repeat(64)));
                store
                    .conn
                    .execute(
                        "UPDATE events SET payload = ?1 WHERE id = ?2",
                        [serde_json::to_string(&payload).unwrap(), row.id.clone()],
                    )
                    .unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.contains("changed or is out of order"), "{error}");
    assert_eq!(inspect_path(&fixture.source), fixture.original_tree);
    assert_eq!(
        fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
        fixture.registry
    );
}

#[test]
fn admission_refuses_stale_owner_and_quarantine_overlap() {
    let fixture = Fixture::new(OriginTool::SkillsSh, false);
    let store = fixture.store();
    let stale = ForkRemovalRequest {
        deployment_id: fixture.request.deployment_id.clone(),
        expected_owner_revision: "stale".into(),
    };
    let error = remove_fork_deployment(
        &mut fixture.service(),
        &store,
        &stale,
        LIMITS,
        TIMEOUT,
        CancellationToken::default(),
    )
    .unwrap_err();
    assert!(error.event_id.is_none());
    assert_eq!(store.list(10, None).unwrap().len(), 0);
    assert_eq!(inspect_path(&fixture.source), fixture.original_tree);

    let mut overlap_scope = fixture.scope.clone();
    let overlap = fixture.scope.home.join(".agents/.skill-studio-removing");
    fs::create_dir_all(&overlap).unwrap();
    overlap_scope.backing_roots.push(overlap);
    let error = remove_fork_deployment(
        &mut ScopedSkillService::bind(overlap_scope).unwrap(),
        &store,
        &fixture.request,
        LIMITS,
        TIMEOUT,
        CancellationToken::default(),
    )
    .unwrap_err();
    assert!(error.event_id.is_none());
    assert!(
        error.message.contains("overlaps a configured skill root"),
        "{error:?}"
    );
    assert_eq!(store.list(10, None).unwrap().len(), 0);
    assert_eq!(inspect_path(&fixture.source), fixture.original_tree);
    assert_eq!(fixture.selected.origin_tool, OriginTool::SkillsSh);
}

#[test]
#[ignore = "creates a retained fixture for native startup acceptance"]
fn generate_native_restart_fixture() {
    let parent = std::env::var_os("FORK_REMOVAL_FIXTURE_PARENT")
        .expect("set a task-owned fixture parent directory");
    let checkpoint = std::env::var("FORK_REMOVAL_CHECKPOINT").unwrap();
    let stop = match checkpoint.as_str() {
        "admitted" => None,
        "holding" => Some(ForwardCheckpoint::HoldingCreated),
        "tree" | "conflict" => Some(ForwardCheckpoint::TreeMoved),
        "published" => Some(ForwardCheckpoint::RegistryPublished),
        _ => panic!("unsupported checkpoint"),
    };
    let temp = tempfile::Builder::new()
        .prefix("native-restart-")
        .tempdir_in(parent)
        .unwrap();
    let fixture = Fixture::with_temp(OriginTool::SkillsSh, true, temp);
    let app_data = fixture
        .scope
        .home
        .join("Library/Application Support/com.skillstudio.app");
    let store = EventStore::open(&app_data).unwrap();
    let (row, intent) = fixture.admit(&store);
    if let Some(stop) = stop {
        let error = execute_forward_with_checkpoint(
            &mut fixture.service(),
            &store,
            &row,
            LIMITS,
            TIMEOUT,
            |current| {
                if current == stop {
                    Err("native checkpoint".into())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(error, "native checkpoint");
    }
    if checkpoint == "conflict" || checkpoint == "published" {
        fs::create_dir(&fixture.source).unwrap();
        fs::write(
            fixture.source.join("SKILL.md"),
            "---\nname: sample\ndescription: Replacement owner\n---\nKeep replacement.\n",
        )
        .unwrap();
    }
    fs::write(
        fixture
            .scope
            .home
            .join(".agents/skill-studio-projects.json"),
        serde_json::to_vec(&serde_json::json!({"tracked": fixture.scope.projects, "excluded": []}))
            .unwrap(),
    )
    .unwrap();
    fs::write(
        fixture.scope.home.join(".agents/skill-studio-scope.json"),
        serde_json::to_vec(&serde_json::json!({
            "backing_roots": fixture.scope.backing_roots,
            "plugin_ownership_roots": fixture.scope.plugin_ownership_roots
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        fixture.root.join("checkpoint.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "checkpoint": checkpoint, "event_id": row.id, "home": fixture.scope.home,
            "app_data": app_data, "source": fixture.source, "quarantine": intent.quarantine_path
        }))
        .unwrap(),
    )
    .unwrap();
    drop(store);
    println!("Native restart fixture: {}", fixture._temp.keep().display());
}

#[test]
fn disappearing_fork_ownership_after_holding_creation_refuses_tree_move() {
    let fixture = Fixture::new(OriginTool::SkillsSh, false);
    let store = fixture.store();
    let (row, intent) = fixture.admit(&store);
    let error = execute_forward_with_checkpoint(
        &mut fixture.service(),
        &store,
        &row,
        LIMITS,
        TIMEOUT,
        |checkpoint| {
            if checkpoint == ForwardCheckpoint::HoldingCreated {
                write_registry_value(&fixture, |registry| {
                    registry["forks"].as_object_mut().unwrap().remove("sample");
                    for key in intent.raw_trial_values.keys() {
                        registry["trials"].as_object_mut().unwrap().remove(key);
                    }
                });
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.contains("ownership disappeared"), "{error}");
    assert_eq!(inspect_path(&fixture.source), fixture.original_tree);
    assert!(!intent.quarantine_path.exists());
    let registry: serde_json::Value = serde_json::from_slice(
        &fs::read(fixture.scope.home.join(".agents/skill-studio.json")).unwrap(),
    )
    .unwrap();
    assert!(registry["forks"].get("sample").is_none());
    assert_eq!(store.get(&row.id).unwrap().unwrap().status, "pending");
}

#[test]
fn publication_between_recovery_classification_and_restore_never_restores() {
    let fixture = Fixture::new(OriginTool::SkillsSh, false);
    let store = fixture.store();
    let (row, intent) = fixture.admit(&store);
    execute_forward_with_checkpoint(
        &mut fixture.service(),
        &store,
        &row,
        LIMITS,
        TIMEOUT,
        |checkpoint| {
            if checkpoint == ForwardCheckpoint::TreeMoved {
                Err("stop before publication".into())
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    let error = recover_with_checkpoint(
        &mut fixture.service(),
        &store,
        &row,
        LIMITS,
        TIMEOUT,
        || {
            write_registry_value(&fixture, |registry| {
                registry["forks"].as_object_mut().unwrap().remove("sample");
                for key in intent.raw_trial_values.keys() {
                    registry["trials"].as_object_mut().unwrap().remove(key);
                }
            });
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.contains("ownership disappeared"), "{error}");
    assert!(!fixture.source.exists());
    assert_eq!(inspect_path(&intent.quarantine_path), fixture.original_tree);
    assert_eq!(store.get(&row.id).unwrap().unwrap().status, "pending");
    assert!(recover_fork_removal(&mut fixture.service(), &store, &row, LIMITS, TIMEOUT).unwrap());
    assert!(!fixture.source.exists());
    assert_eq!(store.get(&row.id).unwrap().unwrap().status, "done");
}
