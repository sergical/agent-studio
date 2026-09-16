use super::event_store::{EventRow, EventStore};
use super::skill_document_operation::DocumentSaveError;
use skill_studio_core::{
    skill_frontmatter_repair::BoundFrontmatterRepairRequest,
    skill_repair_execution as execution,
    skill_service::{CancellationToken, ScopedSkillService},
};
use std::time::{Duration, Instant};

pub(crate) fn is_copy_event(kind: &str) -> bool {
    is_copy_edit_event(kind)
        || matches!(
            kind,
            "repair_copy_frontmatter" | "undo_copy_frontmatter" | "redo_copy_frontmatter"
        )
}

pub(crate) fn is_copy_edit_event(kind: &str) -> bool {
    matches!(
        kind,
        "edit_copy_document" | "undo_copy_document" | "redo_copy_document"
    )
}

pub(crate) fn apply_edit(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &skill_studio_core::skill_copy_document_edit::CopyDocumentEditRequest,
    id: &str,
    cancellation: CancellationToken,
) -> Result<(), DocumentSaveError> {
    use skill_studio_core::skill_copy_document_edit::{
        execute_copy_document_edit, CopyDocumentEditPreparation, DocumentEditPreparationError,
    };
    let prepared = service
        .prepare_copy_document_edit(
            request,
            std::slice::from_ref(&store.app_data),
            Some(Duration::from_secs(30)),
            cancellation,
        )
        .map_err(|error| match error {
            DocumentEditPreparationError::Inventory(error) => DocumentSaveError::from(error),
            DocumentEditPreparationError::Content(error) if error.is_cancelled() => {
                DocumentSaveError::Cancelled
            }
            other => DocumentSaveError::from(other.to_string()),
        })?;
    let result = match prepared {
        CopyDocumentEditPreparation::Unchanged { .. } => Ok(()),
        CopyDocumentEditPreparation::Ready(prepared) => {
            execute_copy_document_edit(*prepared, store, id).map(|_| ())
        }
    };
    if let Err(error) = &result {
        use skill_studio_core::skill_copy_document_edit::{
            DocumentEditCause, DocumentEditPublication,
        };
        if error.cause == DocumentEditCause::Cancelled
            && error.publication == DocumentEditPublication::NotPublished
            && store.get(id)?.is_none()
        {
            return Err(DocumentSaveError::Cancelled);
        }
    }
    settle(
        service,
        store,
        id,
        result.map_err(|error| error.to_string()),
    )
    .map_err(Into::into)
}

fn linked_event(store: &EventStore, row: &EventRow, field: &str) -> Result<EventRow, String> {
    let id = row
        .payload
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or("Copy history is missing a source event")?;
    store
        .get(id)?
        .ok_or("Copy history source is unavailable".into())
}

pub(crate) fn apply(
    service: &mut ScopedSkillService,
    store: &EventStore,
    request: &BoundFrontmatterRepairRequest,
    id: &str,
    cancellation: CancellationToken,
) -> Result<(), String> {
    let prepared = service
        .prepare_copy_repair_selection(
            request,
            std::slice::from_ref(&store.app_data),
            Some(Duration::from_secs(30)),
            cancellation,
        )
        .map_err(|error| error.to_string())?;
    let result = execution::execute_copy_repair(prepared, store, id)
        .map(|_| ())
        .map_err(|error| error.to_string());
    settle(service, store, id, result)
}

pub(crate) fn restore(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
    force: bool,
    id: &str,
    cancellation: CancellationToken,
) -> Result<(), String> {
    if force {
        return Err("Copy restore cannot overwrite changed content or ownership".into());
    }
    if is_copy_edit_event(&row.kind) {
        let prepared = service
            .prepare_copy_document_reversal(row, store, Some(Duration::from_secs(30)), cancellation)
            .map_err(|error| error.to_string())?;
        let result = skill_studio_core::skill_copy_document_edit::execute_copy_document_reversal(
            prepared, store, id,
        )
        .map(|_| ())
        .map_err(|error| error.to_string());
        return settle(service, store, id, result);
    }
    let timeout = Some(Duration::from_secs(30));
    let result = match row.kind.as_str() {
        "repair_copy_frontmatter" | "redo_copy_frontmatter" => {
            let prepared = service
                .prepare_copy_repair_undo(row, store, timeout, cancellation)
                .map_err(|error| error.to_string())?;
            execution::execute_copy_repair_undo(prepared, store, id)
        }
        "undo_copy_frontmatter" => {
            let source = linked_event(store, row, "target_event")?;
            let prepared = service
                .prepare_copy_repair_redo(&source, row, store, timeout, cancellation)
                .map_err(|error| error.to_string())?;
            execution::execute_copy_repair_redo(prepared, store, id)
        }
        _ => return Err("Event is not a Copy repair operation".into()),
    };
    settle(
        service,
        store,
        id,
        result.map(|_| ()).map_err(|error| error.to_string()),
    )
}

fn settle(
    service: &mut ScopedSkillService,
    store: &EventStore,
    id: &str,
    result: Result<(), String>,
) -> Result<(), String> {
    let Err(error) = result else {
        return Ok(());
    };
    if let Some(row) = store.get(id)? {
        if matches!(row.status.as_str(), "pending" | "interrupted") {
            recover(service, store, &row)
                .map_err(|recovery| format!("{error}; recovery remains unresolved: {recovery}"))?;
        }
    }
    if store.get(id)?.is_some_and(|row| row.status == "done") {
        Ok(())
    } else {
        Err(error)
    }
}

pub(crate) fn recover(
    service: &mut ScopedSkillService,
    store: &EventStore,
    row: &EventRow,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let remaining = || Some(deadline.saturating_duration_since(Instant::now()));
    let token = CancellationToken::default();
    if is_copy_edit_event(&row.kind) {
        let prepared = service
            .prepare_copy_document_edit_recovery(row, store, remaining(), token)
            .map_err(|error| error.to_string())?;
        return skill_studio_core::skill_copy_document_edit::recover_copy_document_edit(
            prepared, store,
        )
        .map(|_| ())
        .map_err(|error| error.to_string());
    }
    let result = match row.kind.as_str() {
        "repair_copy_frontmatter" => {
            let prepared = service
                .prepare_copy_repair_recovery(row, store, remaining(), token)
                .map_err(|error| error.to_string())?;
            execution::recover_copy_repair(prepared, store)
        }
        "undo_copy_frontmatter" => {
            let source = linked_event(store, row, "target_event")?;
            let prepared = service
                .prepare_copy_undo_recovery(&source, row, store, remaining(), token)
                .map_err(|error| error.to_string())?;
            execution::recover_copy_undo(prepared, store)
        }
        "redo_copy_frontmatter" => {
            let source = linked_event(store, row, "source_event")?;
            let undo = linked_event(store, row, "undo_event")?;
            let prepared = service
                .prepare_copy_redo_recovery(&source, &undo, row, store, remaining(), token)
                .map_err(|error| error.to_string())?;
            execution::recover_copy_redo(prepared, store)
        }
        _ => return Err("Event is not a Copy repair operation".into()),
    };
    result.map(|_| ()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::{
        skill_deployment::{InstallScope, SkillDestination},
        skill_fork_registry::{write_fork_registry, CopyDeploymentRecord, ForkRegistry},
        skill_frontmatter_repair::FrontmatterRepairApplyMode,
        skill_service::SkillScope,
    };
    use std::{fs, path::Path};

    #[test]
    fn desktop_copy_repair_restore_redo_and_startup_recovery() {
        copy_round_trip(false, false);
    }

    #[test]
    fn project_copy_recovers_failed_completion_for_apply_undo_and_redo() {
        copy_round_trip(true, false);
    }

    fn complete_operation(
        service: &mut ScopedSkillService,
        store: &EventStore,
        scope: &SkillScope,
        id: &str,
        fail_completion: bool,
        operation: impl FnOnce(&mut ScopedSkillService) -> Result<(), String>,
    ) {
        if fail_completion {
            store
                .conn
                .execute_batch(
                    "CREATE TRIGGER reject_copy_completion BEFORE UPDATE OF status ON events
                 WHEN NEW.status = 'done'
                 BEGIN SELECT RAISE(ABORT, 'injected completion failure'); END;",
                )
                .unwrap();
        }
        let result = operation(service);
        if fail_completion {
            let error = result.unwrap_err();
            assert!(error.contains("injected completion failure"), "{error}");
            assert_eq!(store.get(id).unwrap().unwrap().status, "pending");
            store
                .conn
                .execute_batch("DROP TRIGGER reject_copy_completion")
                .unwrap();
            let interrupted = store.reconcile_at_startup().unwrap();
            assert_eq!(interrupted.len(), 1);
            assert_eq!(interrupted[0].id, id);
            super::super::skill_startup_recovery::recover_all(scope.clone(), store).unwrap();
            let count = store.list(100, None).unwrap().len();
            super::super::skill_startup_recovery::recover_all(scope.clone(), store).unwrap();
            assert_eq!(store.list(100, None).unwrap().len(), count);
        } else {
            result.unwrap();
        }
        assert_eq!(store.get(id).unwrap().unwrap().status, "done");
    }

    #[test]
    fn disabled_global_copy_repair_preserves_disabled_ownership() {
        copy_round_trip(false, true);
    }

    #[test]
    fn disabled_project_copy_repair_recovers_without_enabling() {
        copy_round_trip(true, true);
    }

    fn copy_round_trip(project_completion_failure: bool, disabled: bool) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = home.join("projects/test-project");
        let mut skill = if project_completion_failure {
            project.join(".agents/skills/sample")
        } else {
            home.join(".agents/skills/sample")
        };
        if disabled {
            skill = skill
                .parent()
                .unwrap()
                .join(".skill-studio-disabled/sample");
        }
        fs::create_dir_all(&skill).unwrap();
        let sibling = home.join(".agents/skills/sample/SKILL.md");
        if project_completion_failure {
            fs::create_dir(project.join(".git")).unwrap();
            fs::create_dir_all(sibling.parent().unwrap()).unwrap();
            fs::write(
                &sibling,
                "---\nname: sample\ndescription: Global sibling\n---\n",
            )
            .unwrap();
        }
        let sibling_before = fs::read(&sibling).ok();
        fs::create_dir(home.join(".git")).unwrap();
        let original = "---\nname: sample\ndescription: Use when: testing\n---\nbody\n";
        fs::write(skill.join("SKILL.md"), original).unwrap();
        let scope = SkillScope {
            home: home.clone(),
            projects: if project_completion_failure {
                vec![project.clone()]
            } else {
                vec![]
            },
            backing_roots: vec![],
            plugin_ownership_roots: vec![],
        };
        let mut service = ScopedSkillService::bind(scope.clone()).unwrap();
        let inventory = service.scan(None, Some(Duration::from_secs(10))).unwrap();
        let installed = inventory
            .skills
            .iter()
            .find(|installed| {
                installed
                    .deployments
                    .iter()
                    .any(|deployment| Path::new(&deployment.path) == skill)
            })
            .unwrap();
        let deployment = installed
            .deployments
            .iter()
            .find(|deployment| Path::new(&deployment.path) == skill)
            .unwrap();
        assert_eq!(deployment.disabled, disabled);
        let id = deployment.id.clone();
        let mut registry = ForkRegistry::default();
        registry.copies.insert(
            id.clone(),
            CopyDeploymentRecord {
                deployment_id: id.clone(),
                name: "sample".into(),
                path: skill.clone(),
                scope: if project_completion_failure {
                    InstallScope::Project
                } else {
                    InstallScope::Global
                },
                destination: SkillDestination::Universal,
                slot: "universal".into(),
                project_path: project_completion_failure
                    .then(|| project.to_string_lossy().into_owned()),
                content_hash: deployment.content_hash.clone(),
                disabled,
            },
        );
        write_fork_registry(&home, &registry).unwrap();
        let registry_path = home.join(".agents/skill-studio.json");
        let before_registry: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
        let preview = service
            .preview_frontmatter_repair(
                &id,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        let request = BoundFrontmatterRepairRequest {
            deployment_id: id,
            proposal_id: preview.proposal_id,
            expected_content_fingerprint: preview.expected_content_fingerprint,
            mode: FrontmatterRepairApplyMode::ApplyFix,
        };
        let store = EventStore::open(&temp.path().join("state")).unwrap();
        let cancelled = CancellationToken::default();
        cancelled.cancel();
        assert!(apply(&mut service, &store, &request, "cancelled", cancelled).is_err());
        assert!(store.get("cancelled").unwrap().is_none());
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            original
        );
        let blocker = rusqlite::Connection::open(store.app_data.join("events.sqlite3")).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let cancellation = CancellationToken::default();
        let signal = cancellation.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            signal.cancel();
        });
        let started = Instant::now();
        let failure = apply(
            &mut service,
            &store,
            &request,
            "cancelled-busy",
            cancellation,
        )
        .unwrap_err();
        let elapsed = started.elapsed();
        canceller.join().unwrap();
        blocker.execute_batch("ROLLBACK").unwrap();
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}: {failure}");
        assert!(failure.to_lowercase().contains("cancel"), "{failure}");
        assert!(store.get("cancelled-busy").unwrap().is_none());
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            original
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&registry_path).unwrap())
                .unwrap(),
            before_registry
        );
        assert_eq!(
            store
                .conn
                .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            5000
        );
        complete_operation(
            &mut service,
            &store,
            &scope,
            "repair",
            project_completion_failure,
            |service| {
                apply(
                    service,
                    &store,
                    &request,
                    "repair",
                    CancellationToken::default(),
                )
            },
        );
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            preview.proposed_content
        );
        store
            .conn
            .execute(
                "UPDATE events SET status = 'interrupted' WHERE id = 'repair'",
                [],
            )
            .unwrap();
        super::super::skill_startup_recovery::recover_all(scope.clone(), &store).unwrap();
        let repair = store.get("repair").unwrap().unwrap();
        assert_eq!(repair.status, "done");
        assert!(restore(
            &mut service,
            &store,
            &repair,
            true,
            "forced",
            CancellationToken::default()
        )
        .is_err());
        assert!(store.get("forced").unwrap().is_none());
        complete_operation(
            &mut service,
            &store,
            &scope,
            "undo",
            project_completion_failure,
            |service| {
                restore(
                    service,
                    &store,
                    &repair,
                    false,
                    "undo",
                    CancellationToken::default(),
                )
            },
        );
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            original
        );
        let after_registry: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
        assert_eq!(after_registry, before_registry);
        let undo = store.get("undo").unwrap().unwrap();
        complete_operation(
            &mut service,
            &store,
            &scope,
            "redo",
            project_completion_failure,
            |service| {
                restore(
                    service,
                    &store,
                    &undo,
                    false,
                    "redo",
                    CancellationToken::default(),
                )
            },
        );
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            preview.proposed_content
        );
        let redo = store.get("redo").unwrap().unwrap();
        let current_registry: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
        let record = &current_registry["copies"][&request.deployment_id];
        let inventory = service.scan(None, Some(Duration::from_secs(10))).unwrap();
        let deployment = inventory
            .skills
            .iter()
            .flat_map(|skill| &skill.deployments)
            .find(|deployment| deployment.id == request.deployment_id)
            .unwrap();
        assert_eq!(record["content_hash"], deployment.content_hash);
        assert_eq!(
            deployment.owner_kind,
            skill_studio_core::skill_ownership::LifecycleOwnerKind::Copy
        );
        let registry: ForkRegistry =
            serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
        assert_eq!(registry.copies.values().next().unwrap().disabled, disabled);
        if project_completion_failure {
            assert_eq!(fs::read(&sibling).ok(), sibling_before);
        }
        fs::write(skill.join("SKILL.md"), "user changes").unwrap();
        assert!(restore(
            &mut service,
            &store,
            &redo,
            false,
            "drift",
            CancellationToken::default()
        )
        .is_err());
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            "user changes"
        );
        assert!(store.get("drift").unwrap().is_none());
        assert_eq!(
            store.get("repair").unwrap().unwrap().reverted_by.as_deref(),
            Some("undo")
        );
        assert_eq!(
            store.get("undo").unwrap().unwrap().reverted_by.as_deref(),
            Some("redo")
        );
    }
}
