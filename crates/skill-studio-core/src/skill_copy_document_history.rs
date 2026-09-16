use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyDocumentEditSourceReference {
    pub(crate) event_id: String,
    fingerprint: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CopyDocumentReversalIntent {
    source: CopyDocumentEditSourceReference,
    edit: CopyDocumentEditIntent,
}

fn source_fingerprint(row: &EventRow) -> Result<String, String> {
    let mut value = serde_json::to_value(row).map_err(|error| error.to_string())?;
    value["reverted_by"] = serde_json::Value::Null;
    Ok(content_fingerprint(
        &serde_json::to_vec(&value).map_err(|error| error.to_string())?,
    ))
}

pub(super) fn decode_edit_event(
    row: &EventRow,
) -> Result<
    (
        CopyDocumentEditIntent,
        Option<CopyDocumentEditSourceReference>,
    ),
    String,
> {
    if !valid_id(&row.id) || row.restorable || row.inverse.is_some() {
        return Err("Event is not a Copy document change".into());
    }
    let (intent, source) = match row.kind.as_str() {
        "edit_copy_document" => (
            serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?,
            None,
        ),
        "undo_copy_document" | "redo_copy_document" => {
            let reversal: CopyDocumentReversalIntent =
                serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?;
            if !valid_id(&reversal.source.event_id) || reversal.source.event_id == row.id {
                return Err("Invalid Copy edit source reference".into());
            }
            (reversal.edit, Some(reversal.source))
        }
        _ => return Err("Unsupported Copy document event".into()),
    };
    intent.validate_record()?;
    let selected = crate::skill_deployment::parse_deployment_id(&intent.request().deployment_id)
        .ok_or("Invalid edit deployment")?;
    let payload = match &source {
        Some(reference) => serde_json::json!({"source": reference, "edit": &intent}),
        None => serde_json::to_value(&intent).map_err(|error| error.to_string())?,
    };
    if row.skill != intent.transition().before().name
        || row.scope.as_deref() != Some(selected.scope.as_str())
        || row.project_path != selected.project_path
        || row.harness.is_some()
        || row.backup_dir.as_deref() != Some(format!("backups/{}", row.id).as_str())
        || payload != row.payload
    {
        return Err("Copy document event and intent do not match".into());
    }
    Ok((intent, source))
}

fn reversal_kind(kind: &str) -> Result<&'static str, String> {
    match kind {
        "edit_copy_document" | "redo_copy_document" => Ok("undo_copy_document"),
        "undo_copy_document" => Ok("redo_copy_document"),
        _ => Err("Unsupported Copy document source".into()),
    }
}

fn validate_inverse(
    source: &CopyDocumentEditIntent,
    inverse: &CopyDocumentEditIntent,
) -> Result<(), String> {
    source.validate_record()?;
    inverse.validate_record()?;
    if inverse.registry_path() != source.registry_path()
        || inverse.request().deployment_id != source.request().deployment_id
        || inverse.request().expected_content_fingerprint != source.proposed_content_fingerprint
        || inverse.proposed_content_fingerprint != source.request().expected_content_fingerprint
        || inverse.transition().before() != source.transition().after()
        || inverse.transition().after() != source.transition().before()
    {
        return Err("Copy document reversal does not invert its source".into());
    }
    Ok(())
}

impl CopyDocumentEditSourceReference {
    pub(crate) fn validate_claim(
        &self,
        source: &EventRow,
        event_id: &str,
        kind: &str,
        intent: &CopyDocumentEditIntent,
    ) -> Result<(), String> {
        if source.id != self.event_id
            || source.status != "done"
            || source.reverted_by.as_deref() != Some(event_id)
            || source_fingerprint(source)? != self.fingerprint
            || reversal_kind(&source.kind)? != kind
        {
            return Err("Copy document history claim changed".into());
        }
        let (source_intent, _) = decode_edit_event(source)?;
        validate_inverse(&source_intent, intent)
    }
}

pub struct CopyDocumentEditSource {
    snapshot: EventRow,
    intent: CopyDocumentEditIntent,
    origin: Option<CopyDocumentEditSourceReference>,
}
impl CopyDocumentEditSource {
    pub fn from_row(row: &EventRow) -> Result<Self, String> {
        if row.status != "done" || row.reverted_by.is_some() {
            return Err("Copy document event is not available to reverse".into());
        }
        let (intent, origin) = decode_edit_event(row)?;
        Ok(Self {
            snapshot: row.clone(),
            intent,
            origin,
        })
    }
    pub(crate) fn snapshot(&self) -> &EventRow {
        &self.snapshot
    }
    pub(crate) fn origin(&self) -> Option<&CopyDocumentEditSourceReference> {
        self.origin.as_ref()
    }
    pub(crate) fn intent(&self) -> &CopyDocumentEditIntent {
        &self.intent
    }
    pub(crate) fn reversal_draft(
        &self,
        id: &str,
        edit: &CopyDocumentEditIntent,
    ) -> Result<EventDraft, String> {
        if !valid_id(id) || id == self.snapshot.id {
            return Err("Invalid Copy document reversal ID".into());
        }
        validate_inverse(&self.intent, edit)?;
        Ok(EventDraft {
            kind: reversal_kind(&self.snapshot.kind)?.into(),
            skill: self.snapshot.skill.clone(),
            harness: None,
            scope: self.snapshot.scope.clone(),
            project_path: self.snapshot.project_path.clone(),
            payload: serde_json::json!({"source": CopyDocumentEditSourceReference {
                event_id: self.snapshot.id.clone(), fingerprint: source_fingerprint(&self.snapshot)?,
            }, "edit": edit}),
            inverse: None,
            backup_dir: Some(format!("backups/{id}")),
            restorable: false,
        })
    }
}

pub struct PreparedCopyDocumentReversal<'scope> {
    pub(super) change: PreparedCopyDocumentEdit<'scope>,
    pub(super) source: CopyDocumentEditSource,
    pub(super) backup: VerifiedCopyRepairBackup,
}

impl ScopedSkillService {
    pub fn prepare_copy_document_reversal(
        &mut self,
        row: &EventRow,
        store: &EventStore,
        timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<PreparedCopyDocumentReversal<'_>, DocumentEditPreparationError> {
        let invalid = DocumentEditPreparationError::InvalidEdit;
        let source = CopyDocumentEditSource::from_row(row).map_err(invalid)?;
        let names = BTreeSet::from([source.intent.transition().before().name.clone()]);
        let (inventory, lease) = self
            .prepare_write_inventory(
                Some(&names),
                std::slice::from_ref(&store.app_data),
                timeout,
                cancellation.clone(),
            )
            .map_err(DocumentEditPreparationError::Inventory)?;
        let events = GuardedEventStore::bind(store, &lease).map_err(invalid)?;
        events.require_recovered(&lease).map_err(invalid)?;
        events
            .validate_copy_document_source(&lease, &source)
            .map_err(invalid)?;
        let backup =
            VerifiedCopyRepairBackup::read_edit(&store.app_data, &row.id, &source.intent, &lease)
                .map_err(invalid)?;
        let (original, _) = backup.originals();
        let request = CopyDocumentEditRequest {
            deployment_id: source.intent.request().deployment_id.clone(),
            expected_owner_revision: RegistryOwnerRecord::Copy(source.intent.transition().after())
                .revision()
                .ok_or_else(|| invalid("Copy edit owner revision is missing".into()))?,
            expected_content_fingerprint: source.intent.proposed_content_fingerprint.clone(),
            proposed_content: String::from_utf8(original.to_vec())
                .map_err(|error| invalid(error.to_string()))?,
        };
        let CopyDocumentEditPreparation::Ready(change) =
            prepare_copy_edit_with_inventory(&request, inventory, lease, &cancellation)?
        else {
            return Err(invalid(
                "Copy document reversal unexpectedly made no change".into(),
            ));
        };
        validate_inverse(&source.intent, change.intent()).map_err(invalid)?;
        backup.revalidate(&change.lease).map_err(invalid)?;
        Ok(PreparedCopyDocumentReversal {
            change: *change,
            source,
            backup,
        })
    }
}

pub fn execute_copy_document_reversal(
    prepared: PreparedCopyDocumentReversal<'_>,
    store: &EventStore,
    event_id: &str,
) -> Result<DocumentEditReceipt, DocumentEditError> {
    execute_reversal_with(prepared, store, event_id, |_| Ok(()))
}

pub(super) fn execute_reversal_with(
    prepared: PreparedCopyDocumentReversal<'_>,
    store: &EventStore,
    event_id: &str,
    checkpoint: impl FnMut(DocumentEditStage) -> Result<(), String>,
) -> Result<DocumentEditReceipt, DocumentEditError> {
    prepared
        .backup
        .revalidate(&prepared.change.lease)
        .map_err(|message| DocumentEditError {
            event_id: event_id.into(),
            cause: DocumentEditCause::Failure,
            stage: DocumentEditStage::Prepare,
            publication: DocumentEditPublication::NotPublished,
            message,
        })?;
    execute_change(
        prepared.change,
        store,
        event_id,
        Some(prepared.source),
        checkpoint,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_copy_document_edit::preparation_tests::Fixture;
    use std::fs;

    fn apply(fixture: &Fixture, service: &mut ScopedSkillService, store: &EventStore) {
        let CopyDocumentEditPreparation::Ready(prepared) = service
            .prepare_copy_document_edit(
                &fixture.request,
                std::slice::from_ref(&store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap()
        else {
            panic!("expected changed edit")
        };
        execute_copy_document_edit(*prepared, store, "edit-source").unwrap();
    }

    fn reverse(service: &mut ScopedSkillService, store: &EventStore, source: &str, id: &str) {
        let row = store.get(source).unwrap().unwrap();
        let prepared = service
            .prepare_copy_document_reversal(
                &row,
                store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        execute_copy_document_reversal(prepared, store, id).unwrap();
    }

    #[test]
    fn copy_document_history_repeats_undo_redo_across_scopes_and_destinations() {
        for project in [false, true] {
            for per_harness in [false, true] {
                let fixture = Fixture::new(project, per_harness, project && per_harness);
                let before = fixture.bytes();
                let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
                let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                apply(&fixture, &mut service, &store);
                let after = fixture.bytes();
                let mut source = "edit-source".to_string();
                let mut claims = Vec::new();
                for index in 0..4 {
                    let id = format!("reverse-{index}");
                    reverse(&mut service, &store, &source, &id);
                    let row = store.get(&id).unwrap().unwrap();
                    assert_eq!(row.status, "done");
                    assert_eq!(
                        row.kind,
                        if index % 2 == 0 {
                            "undo_copy_document"
                        } else {
                            "redo_copy_document"
                        }
                    );
                    let expected = if index % 2 == 0 { &before } else { &after };
                    let current = fixture.bytes();
                    assert_eq!(current.0, expected.0);
                    assert_eq!(
                        serde_json::from_slice::<serde_json::Value>(&current.1).unwrap(),
                        serde_json::from_slice::<serde_json::Value>(&expected.1).unwrap()
                    );
                    assert_eq!(current.2, before.2);
                    let inventory = service.scan(None, Some(Duration::from_secs(10))).unwrap();
                    let deployment = inventory
                        .skills
                        .iter()
                        .flat_map(|s| &s.deployments)
                        .find(|d| d.id == fixture.request.deployment_id)
                        .unwrap();
                    assert_eq!(deployment.owner_kind, LifecycleOwnerKind::Copy);
                    claims.push((source, id.clone()));
                    for (from, to) in &claims {
                        assert_eq!(
                            store.get(from).unwrap().unwrap().reverted_by.as_ref(),
                            Some(to)
                        );
                    }
                    source = id;
                }
            }
        }
    }

    #[test]
    fn copy_document_history_recovers_undo_and_redo_publication_boundaries() {
        for redo in [false, true] {
            for stage in [
                DocumentEditStage::Backup,
                DocumentEditStage::Intent,
                DocumentEditStage::Document,
                DocumentEditStage::Registry,
            ] {
                let fixture = Fixture::new(true, true, false);
                let initial = fixture.bytes();
                let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
                let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
                apply(&fixture, &mut service, &store);
                let edited = fixture.bytes();
                if redo {
                    reverse(&mut service, &store, "edit-source", "first-undo");
                }
                let source_id = if redo { "first-undo" } else { "edit-source" };
                let source = store.get(source_id).unwrap().unwrap();
                let before = fixture.bytes();
                let prepared = service
                    .prepare_copy_document_reversal(
                        &source,
                        &store,
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                let error =
                    execute_reversal_with(prepared, &store, "interrupted-reverse", |current| {
                        if current == stage {
                            Err("injected interruption".into())
                        } else {
                            Ok(())
                        }
                    })
                    .unwrap_err();
                assert_eq!(error.stage, stage);
                if stage == DocumentEditStage::Backup {
                    assert_eq!(error.publication, DocumentEditPublication::NotPublished);
                    assert!(store.get("interrupted-reverse").unwrap().is_none());
                    assert!(store.get(source_id).unwrap().unwrap().reverted_by.is_none());
                    assert_eq!(fixture.bytes(), before);
                    continue;
                }
                assert_eq!(
                    store
                        .get(source_id)
                        .unwrap()
                        .unwrap()
                        .reverted_by
                        .as_deref(),
                    Some("interrupted-reverse")
                );
                assert_eq!(store.reconcile_at_startup().unwrap().len(), 1);
                let row = store.get("interrupted-reverse").unwrap().unwrap();
                let prepared = service
                    .prepare_copy_document_edit_recovery(
                        &row,
                        &store,
                        Some(Duration::from_secs(10)),
                        CancellationToken::default(),
                    )
                    .unwrap();
                let outcome = recover_copy_document_edit(prepared, &store).unwrap();
                if stage == DocumentEditStage::Intent {
                    assert_eq!(outcome, CopyDocumentEditObservedState::Original);
                    assert_eq!(store.get(&row.id).unwrap().unwrap().status, "failed");
                    assert!(store.get(source_id).unwrap().unwrap().reverted_by.is_none());
                    assert_eq!(fixture.bytes(), before);
                    reverse(&mut service, &store, source_id, "retry-reverse");
                } else {
                    assert_eq!(outcome, CopyDocumentEditObservedState::Applied);
                    assert_eq!(store.get(&row.id).unwrap().unwrap().status, "done");
                    assert_eq!(
                        store
                            .get(source_id)
                            .unwrap()
                            .unwrap()
                            .reverted_by
                            .as_deref(),
                        Some("interrupted-reverse")
                    );
                }
                let expected = if redo { &edited } else { &initial };
                assert_eq!(fixture.bytes().0, expected.0);
                assert_eq!(fixture.bytes().2, initial.2);
                assert!(store.reconcile_at_startup().unwrap().is_empty());
            }
        }
    }

    #[test]
    fn copy_document_history_claim_and_failed_recovery_release_are_atomic() {
        let fixture = Fixture::new(false, false, false);
        let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        apply(&fixture, &mut service, &store);
        let before = fixture.bytes();
        store.conn.execute_batch("CREATE TRIGGER reject_claim BEFORE UPDATE OF reverted_by ON events WHEN NEW.reverted_by IS NOT NULL BEGIN SELECT RAISE(ABORT, 'injected claim failure'); END;").unwrap();
        let source = store.get("edit-source").unwrap().unwrap();
        let prepared = service
            .prepare_copy_document_reversal(
                &source,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        assert!(execute_copy_document_reversal(prepared, &store, "failed-claim").is_err());
        assert!(store.get("failed-claim").unwrap().is_none());
        assert!(store
            .get("edit-source")
            .unwrap()
            .unwrap()
            .reverted_by
            .is_none());
        assert_eq!(fixture.bytes(), before);
        store
            .conn
            .execute_batch("DROP TRIGGER reject_claim")
            .unwrap();
        let prepared = service
            .prepare_copy_document_reversal(
                &source,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        execute_reversal_with(prepared, &store, "pending-undo", |stage| {
            if stage == DocumentEditStage::Intent {
                Err("interrupted".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        store.conn.execute_batch("CREATE TRIGGER reject_release BEFORE UPDATE OF reverted_by ON events WHEN NEW.reverted_by IS NULL BEGIN SELECT RAISE(ABORT, 'injected release failure'); END;").unwrap();
        let row = store.get("pending-undo").unwrap().unwrap();
        let prepared = service
            .prepare_copy_document_edit_recovery(
                &row,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        assert!(recover_copy_document_edit(prepared, &store).is_err());
        assert_eq!(store.get(&row.id).unwrap().unwrap().status, "pending");
        assert_eq!(
            store
                .get("edit-source")
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some("pending-undo")
        );
        assert_eq!(fixture.bytes(), before);
        store
            .conn
            .execute_batch("DROP TRIGGER reject_release")
            .unwrap();
        let prepared = service
            .prepare_copy_document_edit_recovery(
                &row,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(
            recover_copy_document_edit(prepared, &store).unwrap(),
            CopyDocumentEditObservedState::Original
        );
        assert!(store
            .get("edit-source")
            .unwrap()
            .unwrap()
            .reverted_by
            .is_none());
        reverse(&mut service, &store, "edit-source", "retry-undo");
    }

    #[test]
    fn copy_document_history_refuses_duplicate_source_and_later_edit() {
        let fixture = Fixture::new(false, false, false);
        let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
        let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
        apply(&fixture, &mut service, &store);
        let old = store.get("edit-source").unwrap().unwrap();
        reverse(&mut service, &store, "edit-source", "undo");
        let before = fixture.bytes();
        assert!(service
            .prepare_copy_document_reversal(
                &old,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .is_err());
        assert_eq!(fixture.bytes(), before);
        reverse(&mut service, &store, "undo", "redo");
        let redo = store.get("redo").unwrap().unwrap();
        let registry: crate::skill_fork_registry::ForkRegistry =
            serde_json::from_slice(&fs::read(&fixture.registry).unwrap()).unwrap();
        let mut next = fixture.request.clone();
        next.expected_owner_revision =
            RegistryOwnerRecord::Copy(&registry.copies[&next.deployment_id])
                .revision()
                .unwrap();
        next.expected_content_fingerprint =
            content_fingerprint(fixture.request.proposed_content.as_bytes());
        next.proposed_content.push_str("Later edit\n");
        let CopyDocumentEditPreparation::Ready(prepared) = service
            .prepare_copy_document_edit(
                &next,
                std::slice::from_ref(&store.app_data),
                Some(Duration::from_secs(10)),
                CancellationToken::default(),
            )
            .unwrap()
        else {
            panic!("expected edit")
        };
        execute_copy_document_edit(*prepared, &store, "later-edit").unwrap();
        let before = fixture.bytes();
        assert!(service
            .prepare_copy_document_reversal(
                &redo,
                &store,
                Some(Duration::from_secs(10)),
                CancellationToken::default()
            )
            .is_err());
        assert_eq!(fixture.bytes(), before);
        assert!(store.get("redo").unwrap().unwrap().reverted_by.is_none());
    }

    #[test]
    fn copy_document_history_refuses_changed_claim_during_recovery() {
        for changed in ["claim", "payload", "backup"] {
            let fixture = Fixture::new(false, false, false);
            let store = EventStore::open(&fixture.scope.home.join("state")).unwrap();
            let mut service = ScopedSkillService::bind(fixture.scope.clone()).unwrap();
            apply(&fixture, &mut service, &store);
            let source = store.get("edit-source").unwrap().unwrap();
            let prepared = service
                .prepare_copy_document_reversal(
                    &source,
                    &store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            execute_reversal_with(prepared, &store, "pending-undo", |stage| {
                if stage == DocumentEditStage::Document {
                    Err("interrupted".into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            let row = store.get("pending-undo").unwrap().unwrap();
            let prepared = service
                .prepare_copy_document_edit_recovery(
                    &row,
                    &store,
                    Some(Duration::from_secs(10)),
                    CancellationToken::default(),
                )
                .unwrap();
            match changed {
                "claim" => {
                    store
                        .conn
                        .execute(
                            "UPDATE events SET reverted_by = 'other' WHERE id = 'edit-source'",
                            [],
                        )
                        .unwrap();
                }
                "payload" => {
                    store
                        .conn
                        .execute(
                            "UPDATE events SET skill = 'changed' WHERE id = 'edit-source'",
                            [],
                        )
                        .unwrap();
                }
                _ => fs::write(
                    store.app_data.join("backups/pending-undo/0-SKILL.md"),
                    "changed",
                )
                .unwrap(),
            }
            let before = fixture.bytes();
            assert!(
                recover_copy_document_edit(prepared, &store).is_err(),
                "{changed}"
            );
            assert_eq!(fixture.bytes(), before);
            assert_eq!(store.get(&row.id).unwrap().unwrap().status, "pending");
        }
    }
}
