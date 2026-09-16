use super::event_store::EventStore;
use super::skill_document_operation::DocumentSaveError;
use skill_studio_core::{
    skill_copy_document_edit::{CopyDocumentEditRequest, MAX_COPY_DOCUMENT_EDIT_BYTES},
    skill_deployment::parse_deployment_id,
    skill_document_target::SkillDocumentTarget,
    skill_event_operations::GuardedEventStore,
    skill_frontmatter_repair::content_fingerprint,
    skill_inventory::Deployment,
    skill_ownership::LifecycleOwnerKind,
    skill_service::{CancellationToken, ScopedSkillService, SkillScope},
};
use std::{collections::BTreeSet, path::Path, time::Duration};

pub(crate) fn selected_deployment(
    skills: &[skill_studio_core::skill_inventory::InstalledSkill],
    path: &Path,
) -> Result<Deployment, String> {
    if path.file_name() != Some(std::ffi::OsStr::new("SKILL.md")) {
        return Err("The editor can only save an installed SKILL.md".into());
    }
    let exact: Vec<_> = skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| Path::new(&deployment.path).join("SKILL.md") == path)
        .collect();
    if let [deployment] = exact.as_slice() {
        return Ok((*deployment).clone());
    }
    if !exact.is_empty() {
        return Err("The selected skill path is ambiguous".into());
    }
    let canonical = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let mut matches = skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| {
            std::fs::canonicalize(Path::new(&deployment.path).join("SKILL.md"))
                .is_ok_and(|candidate| candidate == canonical)
        });
    let deployment = matches.next().ok_or("Path is not an installed skill")?;
    if matches.next().is_some() {
        return Err("Select an exact deployment before saving this shared path".into());
    }
    Ok(deployment.clone())
}

pub(crate) fn save(
    scope: SkillScope,
    store: &EventStore,
    deployment_id: &str,
    expected: Option<&str>,
    proposed: &str,
    cancellation: CancellationToken,
) -> Result<(), DocumentSaveError> {
    if proposed.len() > MAX_COPY_DOCUMENT_EDIT_BYTES {
        return Err("SKILL.md is too large to save".into());
    }
    let _transaction = super::skill_md_write::begin_skill_md_write_transaction()?;
    let mut service = ScopedSkillService::bind(scope).map_err(|error| error.to_string())?;
    let selected = parse_deployment_id(deployment_id).ok_or("Invalid editor deployment")?;
    let names = BTreeSet::from([selected.name]);
    let (inventory, mut lease) = service
        .prepare_write_inventory(
            Some(&names),
            std::slice::from_ref(&store.app_data),
            Some(Duration::from_secs(30)),
            cancellation.clone(),
        )
        .map_err(DocumentSaveError::from)?;
    let mut matches = inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .filter(|deployment| deployment.id == deployment_id);
    let deployment = matches
        .next()
        .ok_or("The selected skill is no longer installed")?;
    if matches.next().is_some()
        || deployment.plugin.is_some()
        || matches!(
            deployment.owner_kind,
            LifecycleOwnerKind::Plugin
                | LifecycleOwnerKind::Unknown
                | LifecycleOwnerKind::Ambiguous
        )
    {
        return Err("Skill ownership does not permit editing this deployment".into());
    }
    GuardedEventStore::bind_prepared(store, &lease)?.require_recovered_prepared(&lease)?;
    let directory = Path::new(&deployment.path);
    let original = lease
        .read(&directory.join("SKILL.md"), MAX_COPY_DOCUMENT_EDIT_BYTES)
        .map_err(skill_studio_core::skill_service::PreparedContentError::from)?;
    if expected.is_some_and(|text| text.as_bytes() != original) {
        return Err("SKILL.md changed on disk since it was loaded".into());
    }
    if deployment.owner_kind == LifecycleOwnerKind::Copy {
        let request = CopyDocumentEditRequest {
            deployment_id: deployment_id.into(),
            expected_owner_revision: deployment
                .owner_revision
                .clone()
                .ok_or("Copy owner revision is unavailable")?,
            expected_content_fingerprint: content_fingerprint(&original),
            proposed_content: proposed.into(),
        };
        drop(lease);
        return super::skill_copy_repair::apply_edit(
            &mut service,
            store,
            &request,
            &super::event_store::allocate_id(),
            cancellation,
        );
    }
    if original == proposed.as_bytes() {
        return Ok(());
    }
    SkillDocumentTarget::bind(directory)?
        .replace(&mut lease, &original, proposed.as_bytes())
        .map_err(|error| DocumentSaveError::from(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::{
        skill_deployment::{InstallScope, SkillDestination},
        skill_fork_registry::{CopyDeploymentRecord, ForkRegistry},
    };
    use std::{fs, path::PathBuf};

    struct Fixture {
        _temp: tempfile::TempDir,
        scope: SkillScope,
        id: String,
        skill: PathBuf,
        sibling: PathBuf,
        original: String,
        disabled: bool,
    }
    impl Fixture {
        fn new(project: bool, per_harness: bool) -> Self {
            Self::with_disabled(project, per_harness, false)
        }

        fn with_disabled(project: bool, per_harness: bool, disabled: bool) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let home = root.join("home");
            let project_path = root.join("outside-project");
            let active_suffix = if per_harness {
                ".codex/skills/sample"
            } else {
                ".agents/skills/sample"
            };
            let suffix = if disabled {
                active_suffix.replace("/sample", "/.skill-studio-disabled/sample")
            } else {
                active_suffix.into()
            };
            let skill = if project { &project_path } else { &home }.join(suffix);
            let sibling = if project { &home } else { &project_path }
                .join(active_suffix)
                .join("SKILL.md");
            fs::create_dir_all(&skill).unwrap();
            fs::create_dir_all(sibling.parent().unwrap()).unwrap();
            fs::create_dir(home.join(".git")).unwrap();
            fs::create_dir(project_path.join(".git")).unwrap();
            fs::create_dir_all(home.join(".agents")).unwrap();
            let original = "---\nname: sample\ndescription: Original\n---\nBody\n".to_string();
            fs::write(skill.join("SKILL.md"), &original).unwrap();
            fs::write(
                &sibling,
                "---\nname: sample\ndescription: Sibling\n---\nUntouched\n",
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
                .flat_map(|s| &s.deployments)
                .find(|d| Path::new(&d.path) == skill)
                .unwrap();
            let id = deployment.id.clone();
            let record = CopyDeploymentRecord {
                deployment_id: id.clone(),
                name: "sample".into(),
                path: skill.clone(),
                scope: if project {
                    InstallScope::Project
                } else {
                    InstallScope::Global
                },
                destination: if per_harness {
                    SkillDestination::PerHarness
                } else {
                    SkillDestination::Universal
                },
                slot: if per_harness { "codex" } else { "universal" }.into(),
                project_path: project.then(|| project_path.to_string_lossy().into_owned()),
                content_hash: deployment.content_hash.clone(),
                disabled,
            };
            let mut registry = ForkRegistry::default();
            registry.copies.insert(id.clone(), record);
            fs::write(
                home.join(".agents/skill-studio.json"),
                serde_json::to_vec_pretty(&registry).unwrap(),
            )
            .unwrap();
            Self {
                _temp: temp,
                scope,
                id,
                skill,
                sibling,
                original,
                disabled,
            }
        }
        fn assert_copy(&self) {
            let mut service = ScopedSkillService::bind(self.scope.clone()).unwrap();
            let inventory = service.scan(None, Some(Duration::from_secs(10))).unwrap();
            let deployment = inventory
                .skills
                .iter()
                .flat_map(|s| &s.deployments)
                .find(|d| d.id == self.id)
                .unwrap();
            assert_eq!(deployment.owner_kind, LifecycleOwnerKind::Copy);
            assert_eq!(deployment.disabled, self.disabled);
        }
    }

    fn interrupt_save(f: &Fixture, store: &EventStore, edited: &str) -> String {
        store.conn.execute_batch("CREATE TRIGGER reject_completion BEFORE UPDATE OF status ON events WHEN NEW.status = 'done' BEGIN SELECT RAISE(ABORT, 'injected completion failure'); END;").unwrap();
        let error = save(
            f.scope.clone(),
            store,
            &f.id,
            Some(&f.original),
            edited,
            CancellationToken::default(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("recovery remains unresolved"),
            "{error}"
        );
        let event = store.list(10, None).unwrap().pop().unwrap();
        assert_eq!(event.status, "pending");
        store
            .conn
            .execute_batch("DROP TRIGGER reject_completion")
            .unwrap();
        assert_eq!(store.reconcile_at_startup().unwrap().len(), 1);
        event.id
    }

    #[test]
    fn desktop_copy_document_save_undo_redo_and_unconditional_save_keep_ownership() {
        for project in [false, true] {
            for per_harness in [false, true] {
                let f = Fixture::new(project, per_harness);
                let store = EventStore::open(&f.scope.home.join("state")).unwrap();
                let sibling = fs::read(&f.sibling).unwrap();
                let edited = f.original.clone() + "Edited\n";
                save(
                    f.scope.clone(),
                    &store,
                    &f.id,
                    Some(&f.original),
                    &edited,
                    CancellationToken::default(),
                )
                .unwrap();
                f.assert_copy();
                let event = store.list(10, None).unwrap().pop().unwrap();
                assert_eq!(event.kind, "edit_copy_document");
                save(
                    f.scope.clone(),
                    &store,
                    &f.id,
                    Some(&edited),
                    &edited,
                    CancellationToken::default(),
                )
                .unwrap();
                assert_eq!(store.list(10, None).unwrap().len(), 1);
                assert!(save(
                    f.scope.clone(),
                    &store,
                    &f.id,
                    Some(&f.original),
                    "stale",
                    CancellationToken::default()
                )
                .unwrap_err()
                .to_string()
                .contains("changed on disk since it was loaded"));
                let mut service = ScopedSkillService::bind(f.scope.clone()).unwrap();
                {
                    let _transaction =
                        super::super::skill_md_write::begin_skill_md_write_transaction().unwrap();
                    super::super::skill_copy_repair::restore(
                        &mut service,
                        &store,
                        &event,
                        false,
                        "undo-edit",
                        CancellationToken::default(),
                    )
                    .unwrap();
                }
                assert_eq!(
                    fs::read_to_string(f.skill.join("SKILL.md")).unwrap(),
                    f.original
                );
                f.assert_copy();
                let undo = store.get("undo-edit").unwrap().unwrap();
                assert_eq!(undo.kind, "undo_copy_document");
                {
                    let _transaction =
                        super::super::skill_md_write::begin_skill_md_write_transaction().unwrap();
                    super::super::skill_copy_repair::restore(
                        &mut service,
                        &store,
                        &undo,
                        false,
                        "redo-edit",
                        CancellationToken::default(),
                    )
                    .unwrap();
                }
                assert_eq!(
                    fs::read_to_string(f.skill.join("SKILL.md")).unwrap(),
                    edited
                );
                let unconditional = edited + "Unconditional save\n";
                save(
                    f.scope.clone(),
                    &store,
                    &f.id,
                    None,
                    &unconditional,
                    CancellationToken::default(),
                )
                .unwrap();
                f.assert_copy();
                assert_eq!(store.list(10, None).unwrap().len(), 4);
                assert_eq!(fs::read(&f.sibling).unwrap(), sibling);
            }
        }
    }

    #[test]
    fn desktop_copy_save_cancels_under_database_contention_and_can_retry() {
        for project in [false, true] {
            for per_harness in [false, true] {
                let f = Fixture::new(project, per_harness);
                let store = EventStore::open(&f.scope.home.join("state")).unwrap();
                let registry_path = f.scope.home.join(".agents/skill-studio.json");
                let registry_before = fs::read(&registry_path).unwrap();
                let sibling_before = fs::read(&f.sibling).unwrap();
                let blocker =
                    rusqlite::Connection::open(store.app_data.join("events.sqlite3")).unwrap();
                blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
                let cancellation = CancellationToken::default();
                let signal = cancellation.clone();
                let canceller = std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(100));
                    signal.cancel();
                });
                let edited = f.original.clone() + "Retry after cancellation\n";
                let started = std::time::Instant::now();
                let error = save(
                    f.scope.clone(),
                    &store,
                    &f.id,
                    Some(&f.original),
                    &edited,
                    cancellation,
                )
                .unwrap_err();
                let elapsed = started.elapsed();
                canceller.join().unwrap();
                assert!(elapsed < Duration::from_secs(2), "{elapsed:?}: {error}");
                assert!(matches!(error, DocumentSaveError::Cancelled), "{error}");
                assert!(!blocker.is_autocommit());
                assert_eq!(
                    fs::read(f.skill.join("SKILL.md")).unwrap(),
                    f.original.as_bytes()
                );
                assert_eq!(fs::read(&registry_path).unwrap(), registry_before);
                assert_eq!(fs::read(&f.sibling).unwrap(), sibling_before);
                assert!(store.list(10, None).unwrap().is_empty());
                blocker.execute_batch("ROLLBACK").unwrap();

                save(
                    f.scope.clone(),
                    &store,
                    &f.id,
                    Some(&f.original),
                    &edited,
                    CancellationToken::default(),
                )
                .unwrap();
                assert_eq!(
                    fs::read_to_string(f.skill.join("SKILL.md")).unwrap(),
                    edited
                );
                assert_eq!(fs::read(&f.sibling).unwrap(), sibling_before);
                f.assert_copy();
                let events = store.list(10, None).unwrap();
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].kind, "edit_copy_document");
                assert_eq!(events[0].status, "done");
            }
        }
    }

    #[test]
    fn desktop_copy_document_startup_recovers_rejected_completion() {
        let f = Fixture::new(true, false);
        let store = EventStore::open(&f.scope.home.join("state")).unwrap();
        let edited = f.original.clone() + "Edited\n";
        let event_id = interrupt_save(&f, &store, &edited);
        super::super::skill_startup_recovery::recover_all(f.scope.clone(), &store).unwrap();
        assert_eq!(store.get(&event_id).unwrap().unwrap().status, "done");
        super::super::skill_startup_recovery::recover_all(f.scope.clone(), &store).unwrap();
        assert_eq!(store.list(10, None).unwrap().len(), 1);
        f.assert_copy();
    }

    #[test]
    fn registered_project_copy_recovers_through_the_production_startup_scope() {
        let f = Fixture::new(true, false);
        let project = f.scope.projects[0].clone();
        assert!(!project.starts_with(&f.scope.home));
        assert!(
            !super::super::project_discovery::discover_skill_projects(&f.scope.home)
                .contains(&project)
        );
        super::super::skill_project_authority::track(
            &f.scope.home,
            vec![project.to_string_lossy().into_owned()],
        )
        .unwrap();
        let store = EventStore::open(&f.scope.home.join("state")).unwrap();
        let edited = f.original.clone() + "Edited\n";
        let event_id = interrupt_save(&f, &store, &edited);

        super::super::skill_startup_recovery::recover_at_startup(&store, &f.scope.home).unwrap();

        assert_eq!(store.get(&event_id).unwrap().unwrap().status, "done");
        assert_eq!(
            fs::read_to_string(f.skill.join("SKILL.md")).unwrap(),
            edited
        );
        f.assert_copy();
        let projects =
            super::super::skill_project_authority::scoped_projects(&f.scope.home, []).unwrap();
        let scope = super::super::skill_scope_config::desktop_skill_scope(&f.scope.home, &projects)
            .unwrap();
        let mut service = ScopedSkillService::bind(scope).unwrap();
        let edit = store.get(&event_id).unwrap().unwrap();
        super::super::skill_copy_repair::restore(
            &mut service,
            &store,
            &edit,
            false,
            "registered-undo",
            CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(f.skill.join("SKILL.md")).unwrap(),
            f.original
        );
        let undo = store.get("registered-undo").unwrap().unwrap();
        super::super::skill_copy_repair::restore(
            &mut service,
            &store,
            &undo,
            false,
            "registered-redo",
            CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(f.skill.join("SKILL.md")).unwrap(),
            edited
        );
    }

    #[test]
    fn revoked_event_project_path_does_not_authorize_startup_recovery() {
        let f = Fixture::new(true, false);
        let project = f.scope.projects[0].clone();
        super::super::skill_project_authority::track(
            &f.scope.home,
            vec![project.to_string_lossy().into_owned()],
        )
        .unwrap();
        let store = EventStore::open(&f.scope.home.join("state")).unwrap();
        let edited = f.original.clone() + "Edited\n";
        let event_id = interrupt_save(&f, &store, &edited);
        super::super::skill_project_authority::exclude(&f.scope.home, project.to_str().unwrap())
            .unwrap();

        let error = super::super::skill_startup_recovery::recover_at_startup(&store, &f.scope.home)
            .unwrap_err();

        assert!(error.contains("recovery deployment is absent"), "{error}");
        assert_eq!(store.get(&event_id).unwrap().unwrap().status, "interrupted");
    }

    #[test]
    fn disabled_copy_edit_recovery_and_history_preserve_disabled_state() {
        let f = Fixture::with_disabled(true, false, true);
        let store = EventStore::open(&f.scope.home.join("state")).unwrap();
        let edited = f.original.clone() + "Edited\n";
        let event_id = interrupt_save(&f, &store, &edited);

        super::super::skill_startup_recovery::recover_all(f.scope.clone(), &store).unwrap();
        let edit = store.get(&event_id).unwrap().unwrap();
        assert_eq!(edit.status, "done");
        f.assert_copy();
        let mut service = ScopedSkillService::bind(f.scope.clone()).unwrap();
        super::super::skill_copy_repair::restore(
            &mut service,
            &store,
            &edit,
            false,
            "disabled-undo",
            CancellationToken::default(),
        )
        .unwrap();
        f.assert_copy();
        let undo = store.get("disabled-undo").unwrap().unwrap();
        super::super::skill_copy_repair::restore(
            &mut service,
            &store,
            &undo,
            false,
            "disabled-redo",
            CancellationToken::default(),
        )
        .unwrap();
        f.assert_copy();
        assert_eq!(
            fs::read_to_string(f.skill.join("SKILL.md")).unwrap(),
            edited
        );
    }

    #[test]
    fn desktop_document_save_preserves_direct_owner_and_rejects_cancellation() {
        let f = Fixture::new(false, false);
        fs::write(
            f.scope.home.join(".agents/skill-studio.json"),
            serde_json::to_vec_pretty(&ForkRegistry::default()).unwrap(),
        )
        .unwrap();
        let store = EventStore::open(&f.scope.home.join("state")).unwrap();
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert!(save(
            f.scope.clone(),
            &store,
            &f.id,
            Some(&f.original),
            "cancelled",
            cancelled
        )
        .is_err());
        assert_eq!(
            fs::read_to_string(f.skill.join("SKILL.md")).unwrap(),
            f.original
        );
        let edited = f.original.clone() + "Direct edit\n";
        save(
            f.scope.clone(),
            &store,
            &f.id,
            Some(&f.original),
            &edited,
            CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(f.skill.join("SKILL.md")).unwrap(),
            edited
        );
        assert!(store.list(10, None).unwrap().is_empty());
    }

    #[test]
    fn desktop_document_selection_prefers_exact_deployment_and_refuses_ambiguous_alias() {
        let f = Fixture::new(false, false);
        let alias = f.scope.home.join(".claude/skills/sample");
        fs::create_dir_all(alias.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&f.skill, &alias).unwrap();
        let root_alias = f.scope.home.parent().unwrap().join("home-alias");
        std::os::unix::fs::symlink(&f.scope.home, &root_alias).unwrap();
        let mut service = ScopedSkillService::bind(f.scope.clone()).unwrap();
        let inventory = service.scan(None, Some(Duration::from_secs(10))).unwrap();
        let selected = selected_deployment(&inventory.skills, &alias.join("SKILL.md")).unwrap();
        assert_eq!(Path::new(&selected.path), alias);
        assert!(selected_deployment(
            &inventory.skills,
            &root_alias.join(".agents/skills/sample/SKILL.md")
        )
        .is_err());
    }
}
