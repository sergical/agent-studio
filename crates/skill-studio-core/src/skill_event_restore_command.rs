use super::*;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "transition", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RestoreOperation {
    Record {
        #[serde(with = "EventRowWire")]
        source: EventRow,
        record: RecordPendingRequest,
    },
    Finish {
        #[serde(with = "EventRowWire")]
        source: EventRow,
        completion: RecoveryRequest,
    },
}

pub(super) struct PreparedRestoreCommand {
    operation: RestoreOperation,
    command_id: String,
    event_id: String,
    pub(super) digest: Vec<u8>,
}

impl PreparedRestoreCommand {
    pub(super) fn new(
        command_id: &str,
        event_id: &str,
        operation: RestoreOperation,
    ) -> Result<Self, String> {
        validate_ids(command_id, event_id)?;
        let source = match &operation {
            RestoreOperation::Record { source, record } => {
                if source.reverted_by.is_some()
                    || record.draft.kind != "restore"
                    || record.draft.payload["target_event"] != source.id
                {
                    return Err("Restore requires an unclaimed source and linked intent".into());
                }
                PreparedRecord::new(
                    command_id,
                    event_id,
                    &record.timestamp,
                    record.draft.clone(),
                )?;
                source
            }
            RestoreOperation::Finish { source, completion } => {
                if source.reverted_by.as_deref() != Some(event_id)
                    || completion.snapshot.kind != "restore"
                    || completion.snapshot.payload["target_event"] != source.id
                {
                    return Err("Restore completion requires its unchanged source claim".into());
                }
                PreparedRecovery::new(
                    command_id,
                    event_id,
                    completion.snapshot.clone(),
                    match completion.status {
                        FinishStatus::Done => EventStatus::Done,
                        FinishStatus::Failed => EventStatus::Failed,
                    },
                    completion.inverse.clone(),
                )?;
                source
            }
        };
        if source.id == event_id
            || source.status != "done"
            || !source.restorable
            || source.inverse.is_none()
        {
            return Err("Restore source is not a completed restorable event".into());
        }
        validate_ids(&source.id, event_id)?;
        let encoded = serde_json::to_vec(&("event-restore-v1", event_id, &operation))
            .map_err(|error| error.to_string())?;
        if encoded.len() > 1024 * 1024 {
            return Err("Command exceeds the 1 MiB limit".into());
        }
        Ok(Self {
            operation,
            command_id: command_id.into(),
            event_id: event_id.into(),
            digest: Sha256::digest(encoded).to_vec(),
        })
    }

    pub(super) fn apply(&self, connection: &mut Connection) -> Result<CommandReceipt, String> {
        apply(
            connection,
            &self.command_id,
            &self.event_id,
            &self.digest,
            |transaction| {
                let source = match &self.operation {
                    RestoreOperation::Record { source, .. }
                    | RestoreOperation::Finish { source, .. } => source,
                };
                let current = transaction
                    .query_row(
                        "SELECT * FROM events WHERE id = ?1",
                        [&source.id],
                        crate::skill_event_statements::row_from,
                    )
                    .optional()
                    .map_err(|error| error.to_string())?
                    .ok_or("Restore source is missing")?;
                if serde_json::to_value(current).map_err(|error| error.to_string())?
                    != serde_json::to_value(source).map_err(|error| error.to_string())?
                {
                    return Err("Restore source changed".into());
                }
                match &self.operation {
                    RestoreOperation::Record { record, .. } => {
                        crate::skill_event_statements::require_recovered(transaction)?;
                        crate::skill_event_statements::insert_pending(
                            transaction,
                            &self.event_id,
                            &record.timestamp,
                            record.draft.clone(),
                        )?;
                        let changed = transaction.execute("UPDATE events SET reverted_by = ?1 WHERE id = ?2 AND reverted_by IS NULL", params![self.event_id, source.id]).map_err(|error| error.to_string())?;
                        if changed != 1 {
                            return Err("Restore source was already claimed".into());
                        }
                    }
                    RestoreOperation::Finish { completion, .. } => {
                        let inverse = completion
                            .inverse
                            .as_ref()
                            .map(serde_json::to_string)
                            .transpose()
                            .map_err(|error| error.to_string())?;
                        let status = match completion.status {
                            FinishStatus::Done => EventStatus::Done,
                            FinishStatus::Failed => EventStatus::Failed,
                        };
                        crate::skill_event_statements::finish_recovery_snapshot(
                            transaction,
                            &completion.snapshot,
                            status,
                            inverse.as_deref(),
                        )?;
                        if matches!(status, EventStatus::Failed) {
                            let changed = transaction.execute("UPDATE events SET reverted_by = NULL WHERE id = ?1 AND reverted_by = ?2", params![source.id, self.event_id]).map_err(|error| error.to_string())?;
                            if changed != 1 {
                                return Err("Restore claim changed".into());
                            }
                        }
                    }
                }
                Ok(())
            },
        )
    }
}

#[test]
fn restore_claim_intent_completion_and_receipts_commit_atomically() {
    let receipt_count = |connection: &Connection| {
        connection.query_row("SELECT count(*) FROM event_command_receipts", [], |row| {
            row.get::<_, i64>(0)
        })
    };
    let temp = tempfile::tempdir().unwrap();
    let mut store = crate::skill_event_store::EventStore::open(temp.path()).unwrap();
    let source_draft = EventDraft {
        kind: "fixture".into(),
        skill: "alpha".into(),
        harness: None,
        scope: None,
        project_path: None,
        payload: serde_json::json!({}),
        inverse: Some(serde_json::json!({"fixture":true})),
        backup_dir: None,
        restorable: true,
    };
    store.record("source", source_draft.clone()).unwrap();
    store.finish("source", EventStatus::Done).unwrap();
    let source = store.get("source").unwrap().unwrap();
    let draft = EventDraft {
        kind: "restore".into(),
        payload: serde_json::json!({"target_event":"source"}),
        ..source_draft
    };
    let record = PreparedEventExchange::record_restore(
        "record-exchange".into(),
        "record-command".into(),
        "undo".into(),
        source,
        chrono::Utc::now().to_rfc3339(),
        draft,
    )
    .unwrap();
    store.conn.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON event_command_receipts BEGIN SELECT RAISE(ABORT, 'receipt failure'); END;").unwrap();
    assert!(record.command.apply(&mut store.conn).is_err());
    assert!(store.get("undo").unwrap().is_none());
    assert!(store.get("source").unwrap().unwrap().reverted_by.is_none());
    assert_eq!(receipt_count(&store.conn).unwrap(), 0);
    store
        .conn
        .execute_batch("DROP TRIGGER fail_receipt;")
        .unwrap();
    record.command.apply(&mut store.conn).unwrap();
    record.command.apply(&mut store.conn).unwrap();
    let claimed = store.get("source").unwrap().unwrap();
    assert_eq!(claimed.reverted_by.as_deref(), Some("undo"));
    let pending = store.get("undo").unwrap().unwrap();
    let finish = PreparedEventExchange::finish_restore(
        "finish-exchange".into(),
        "finish-command".into(),
        claimed,
        pending,
        EventStatus::Failed,
        None,
    )
    .unwrap();
    store
        .conn
        .execute(
            "UPDATE events SET skill = 'changed' WHERE id = 'source'",
            [],
        )
        .unwrap();
    assert!(finish.command.apply(&mut store.conn).is_err());
    assert_eq!(store.get("undo").unwrap().unwrap().status, "pending");
    store
        .conn
        .execute("UPDATE events SET skill = 'alpha' WHERE id = 'source'", [])
        .unwrap();
    store.conn.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON event_command_receipts BEGIN SELECT RAISE(ABORT, 'receipt failure'); END;").unwrap();
    assert!(finish.command.apply(&mut store.conn).is_err());
    assert_eq!(store.get("undo").unwrap().unwrap().status, "pending");
    assert_eq!(
        store.get("source").unwrap().unwrap().reverted_by.as_deref(),
        Some("undo")
    );
    assert_eq!(receipt_count(&store.conn).unwrap(), 1);
    store
        .conn
        .execute_batch("DROP TRIGGER fail_receipt;")
        .unwrap();
    finish.command.apply(&mut store.conn).unwrap();
    finish.command.apply(&mut store.conn).unwrap();
    assert_eq!(store.get("undo").unwrap().unwrap().status, "failed");
    assert!(store.get("source").unwrap().unwrap().reverted_by.is_none());
    assert_eq!(receipt_count(&store.conn).unwrap(), 2);
}
