//! Typed database commands and receipt validation for an event worker.
//!
//! The caller owns database authority, schema initialization, connection policy,
//! process cleanup and recovery. Preparing a request grants no filesystem access.
//! Execute only on the worker's authorized connection; event paths are inert data.
//! This module does not apply file mutations or enable CLI/MCP write adapters.
use crate::skill_event::{EventDraft, EventRow, EventStatus};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

#[path = "skill_event_copy_command.rs"]
mod copy_commands;
#[path = "skill_event_restore_command.rs"]
mod restore_commands;
pub use copy_commands::CopyHistoryTransition;

struct PreparedFinish {
    command_id: String,
    event_id: String,
    status: EventStatus,
    inverse: Option<String>,
    digest: Vec<u8>,
}
impl PreparedFinish {
    fn new(
        command_id: &str,
        event_id: &str,
        status: EventStatus,
        inverse: Option<serde_json::Value>,
    ) -> Result<Self, String> {
        validate_ids(command_id, event_id)?;
        let inverse = inverse
            .map(|value| serde_json::to_string(&value))
            .transpose()
            .map_err(|e| e.to_string())?;
        // The versioned tuple hashes exact prepared inverse bytes, not wire formatting.
        let encoded = serde_json::to_vec(&("event-finish-v1", event_id, status.as_str(), &inverse))
            .map_err(|e| e.to_string())?;
        if encoded.len() > 1024 * 1024 {
            return Err("Command exceeds the 1 MiB limit".into());
        }
        let digest = Sha256::digest(&encoded).to_vec();
        Ok(Self {
            command_id: command_id.into(),
            event_id: event_id.into(),
            status,
            inverse,
            digest,
        })
    }
}

fn validate_ids(command_id: &str, event_id: &str) -> Result<(), String> {
    for id in [command_id, event_id] {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("Invalid command or event identity".into());
        }
    }
    Ok(())
}

struct PreparedRecord {
    command_id: String,
    event_id: String,
    timestamp: String,
    draft: EventDraft,
    digest: Vec<u8>,
}
impl PreparedRecord {
    fn new(
        command_id: &str,
        event_id: &str,
        timestamp: &str,
        draft: EventDraft,
    ) -> Result<Self, String> {
        validate_ids(command_id, event_id)?;
        chrono::DateTime::parse_from_rfc3339(timestamp).map_err(|e| e.to_string())?;
        let encoded = serde_json::to_vec(&(
            "event-record-v1",
            event_id,
            timestamp,
            &draft.kind,
            &draft.skill,
            &draft.harness,
            &draft.scope,
            &draft.project_path,
            &draft.payload,
            &draft.inverse,
            &draft.backup_dir,
            draft.restorable,
        ))
        .map_err(|e| e.to_string())?;
        // Reserve space for completion fingerprints so the read worker can retrieve recovery evidence.
        if encoded.len() > crate::skill_history::MAX_HISTORY_RECORD_BYTES - 1024 {
            return Err("Pending event exceeds the history recovery read budget".into());
        }
        let digest = Sha256::digest(&encoded).to_vec();
        Ok(Self {
            command_id: command_id.into(),
            event_id: event_id.into(),
            timestamp: timestamp.into(),
            draft,
            digest,
        })
    }
}

struct PreparedRecovery {
    command_id: String,
    snapshot: EventRow,
    status: EventStatus,
    inverse: Option<String>,
    digest: Vec<u8>,
}
impl PreparedRecovery {
    fn new(
        command_id: &str,
        event_id: &str,
        snapshot: EventRow,
        status: EventStatus,
        inverse: Option<serde_json::Value>,
    ) -> Result<Self, String> {
        validate_ids(command_id, event_id)?;
        if snapshot.id != event_id
            || !matches!(snapshot.status.as_str(), "pending" | "interrupted")
            || snapshot.reverted_by.is_some()
        {
            return Err("Recovery requires an unchanged unresolved event".into());
        }
        let inverse = inverse
            .map(|value| serde_json::to_string(&value))
            .transpose()
            .map_err(|error| error.to_string())?;
        let encoded =
            serde_json::to_vec(&("event-recovery-v1", &snapshot, status.as_str(), &inverse))
                .map_err(|error| error.to_string())?;
        if encoded.len() > 1024 * 1024 {
            return Err("Command exceeds the 1 MiB limit".into());
        }
        Ok(Self {
            command_id: command_id.into(),
            snapshot,
            status,
            inverse,
            digest: Sha256::digest(encoded).to_vec(),
        })
    }
}

#[derive(Debug, PartialEq)]
struct CommandReceipt {
    command_id: String,
    event_id: String,
    digest: Vec<u8>,
}

fn finish(connection: &mut Connection, command: &PreparedFinish) -> Result<CommandReceipt, String> {
    apply(
        connection,
        &command.command_id,
        &command.event_id,
        &command.digest,
        |transaction| {
            crate::skill_event_statements::finish_pending(
                transaction,
                &command.event_id,
                command.status,
                command.inverse.as_deref(),
            )
        },
    )
}
fn record(connection: &mut Connection, command: &PreparedRecord) -> Result<CommandReceipt, String> {
    apply(
        connection,
        &command.command_id,
        &command.event_id,
        &command.digest,
        |transaction| {
            crate::skill_event_statements::require_recovered(transaction)?;
            crate::skill_event_statements::insert_pending(
                transaction,
                &command.event_id,
                &command.timestamp,
                command.draft.clone(),
            )
        },
    )
}
fn finish_recovery(
    connection: &mut Connection,
    command: &PreparedRecovery,
) -> Result<CommandReceipt, String> {
    apply(
        connection,
        &command.command_id,
        &command.snapshot.id,
        &command.digest,
        |transaction| {
            crate::skill_event_statements::finish_recovery_snapshot(
                transaction,
                &command.snapshot,
                command.status,
                command.inverse.as_deref(),
            )
        },
    )
}

fn lookup_receipt(
    connection: &Connection,
    command_id: &str,
    event_id: &str,
    digest: &[u8],
) -> Result<Option<CommandReceipt>, String> {
    let existing: Option<(String, Vec<u8>)> = connection
        .query_row(
            "SELECT event_id, digest FROM event_command_receipts WHERE command_id = ?1",
            [command_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((saved_event, saved_digest)) = existing else {
        return Ok(None);
    };
    if saved_event != event_id || saved_digest != digest {
        return Err("Command identity was already used for different data".into());
    }
    Ok(Some(CommandReceipt {
        command_id: command_id.into(),
        event_id: saved_event,
        digest: saved_digest,
    }))
}

fn apply(
    connection: &mut Connection,
    command_id: &str,
    event_id: &str,
    digest: &[u8],
    transition: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<(), String>,
) -> Result<CommandReceipt, String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if lookup_receipt(&transaction, command_id, event_id, digest)?.is_none() {
        transition(&transaction)?;
        transaction
            .execute(
                "INSERT INTO event_command_receipts(command_id,event_id,digest) VALUES (?1,?2,?3)",
                params![command_id, event_id, digest],
            )
            .map_err(|e| e.to_string())?;
    }
    transaction.commit().map_err(|e| e.to_string())?;
    Ok(CommandReceipt {
        command_id: command_id.into(),
        event_id: event_id.into(),
        digest: digest.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{skill_event::EventDraft, skill_event_store::EventStore};
    fn seed(store: &EventStore) {
        store
            .record(
                "event",
                EventDraft {
                    kind: "fixture".into(),
                    skill: "fixture".into(),
                    harness: None,
                    scope: None,
                    project_path: None,
                    payload: serde_json::json!({}),
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
    }
    #[test]
    fn receipt_replays_after_reopen_and_rejects_identity_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = EventStore::open(temp.path()).unwrap();
        seed(&store);
        let command = PreparedFinish::new(
            "finish",
            "event",
            EventStatus::Done,
            Some(serde_json::json!({"post":"value"})),
        )
        .unwrap();
        let receipt = finish(&mut store.conn, &command).unwrap();
        drop(store);
        let mut store = EventStore::open(temp.path()).unwrap();
        assert_eq!(finish(&mut store.conn, &command).unwrap(), receipt);
        for conflicting in [
            PreparedFinish::new(
                "finish",
                "event",
                EventStatus::Failed,
                Some(serde_json::json!({"post":"value"})),
            )
            .unwrap(),
            PreparedFinish::new("finish", "event", EventStatus::Done, None).unwrap(),
            PreparedFinish::new("finish", "other", EventStatus::Done, None).unwrap(),
        ] {
            assert!(finish(&mut store.conn, &conflicting).is_err());
        }
        let event = store.get("event").unwrap().unwrap();
        assert_eq!(event.status, "done");
        assert_eq!(event.inverse, Some(serde_json::json!({"post":"value"})));
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM event_command_receipts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn receipt_insert_failure_rolls_back_the_transition() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = EventStore::open(temp.path()).unwrap();
        seed(&store);
        store.conn.execute_batch("CREATE TRIGGER refuse_receipt BEFORE INSERT ON event_command_receipts BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;").unwrap();
        let command = PreparedFinish::new("finish", "event", EventStatus::Done, None).unwrap();
        assert!(finish(&mut store.conn, &command).is_err());
        assert!(store.conn.is_autocommit());
        assert_eq!(store.get("event").unwrap().unwrap().status, "pending");
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM event_command_receipts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        store
            .conn
            .execute_batch("DROP TRIGGER refuse_receipt")
            .unwrap();
        finish(&mut store.conn, &command).unwrap();
        let missing = PreparedFinish::new("missing", "absent", EventStatus::Done, None).unwrap();
        assert!(finish(&mut store.conn, &missing).is_err());
        let stale = PreparedFinish::new("second", "event", EventStatus::Failed, None).unwrap();
        assert!(finish(&mut store.conn, &stale).is_err());
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM event_command_receipts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn record_receipt_replays_pending_intent_and_blocks_competitors() {
    use crate::skill_event_store::EventStore;
    let temp = tempfile::tempdir().unwrap();
    let mut store = EventStore::open(temp.path()).unwrap();
    let draft = EventDraft {
        kind: "fixture".into(),
        skill: "fixture".into(),
        harness: None,
        scope: None,
        project_path: None,
        payload: serde_json::json!({"intent":1}),
        inverse: None,
        backup_dir: None,
        restorable: false,
    };
    let command =
        PreparedRecord::new("record", "event", "2026-09-12T00:00:00Z", draft.clone()).unwrap();
    let receipt = record(&mut store.conn, &command).unwrap();
    drop(store);
    let mut store = EventStore::open(temp.path()).unwrap();
    assert_eq!(record(&mut store.conn, &command).unwrap(), receipt);
    let competitor = PreparedRecord::new(
        "other-command",
        "other-event",
        "2026-09-12T00:00:00Z",
        draft.clone(),
    )
    .unwrap();
    assert!(record(&mut store.conn, &competitor).is_err());
    assert!(store.get("other-event").unwrap().is_none());
    let changed = PreparedRecord::new("record", "event", "2026-09-12T00:00:01Z", draft).unwrap();
    assert!(record(&mut store.conn, &changed).is_err());
    let collision = PreparedFinish::new("record", "event", EventStatus::Done, None).unwrap();
    assert!(finish(&mut store.conn, &collision).is_err());
    let completion = PreparedFinish::new("finish", "event", EventStatus::Done, None).unwrap();
    finish(&mut store.conn, &completion).unwrap();
    assert_eq!(record(&mut store.conn, &command).unwrap(), receipt);
    assert_eq!(store.get("event").unwrap().unwrap().status, "done");
    record(&mut store.conn, &competitor).unwrap();
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM event_command_receipts", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) fn exercise_native_receipts(connection: &mut Connection) -> Result<i64, String> {
    let draft = EventDraft {
        kind: "fixture".into(),
        skill: "native-receipt".into(),
        harness: None,
        scope: None,
        project_path: None,
        payload: serde_json::json!({"intent": 1}),
        inverse: None,
        backup_dir: None,
        restorable: false,
    };
    let intent = PreparedRecord::new(
        "native-record",
        "native-event",
        "2026-09-12T00:00:00Z",
        draft,
    )?;
    let completion = PreparedFinish::new(
        "native-finish",
        "native-event",
        EventStatus::Done,
        Some(serde_json::json!({"post": 42})),
    )?;
    let recorded = record(connection, &intent)?;
    let finished = finish(connection, &completion)?;
    assert_eq!(record(connection, &intent)?, recorded);
    assert_eq!(finish(connection, &completion)?, finished);
    let state: (String, String, String) = connection
        .query_row(
            "SELECT status, ts, inverse FROM events WHERE id='native-event'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| e.to_string())?;
    assert_eq!(
        state,
        (
            "done".into(),
            "2026-09-12T00:00:00Z".into(),
            "{\"post\":42}".into()
        )
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    connection
        .query_row("SELECT count(*) FROM event_command_receipts", [], |row| {
            row.get(0)
        })
        .map_err(|e| e.to_string())
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventRequest {
    action: EventAction,
    version: u16,
    exchange_id: String,
    command_id: String,
    event_id: String,
    operation: EventOperation,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum EventOperation {
    CopyHistory(Box<CopyHistoryTransition>),
    Restore(Box<restore_commands::RestoreOperation>),
    Initialize,
    Preflight,
    FinishPending {
        status: FinishStatus,
        inverse: Option<serde_json::Value>,
    },
    RecordPending(Box<RecordPendingRequest>),
    FinishRecovery(Box<RecoveryRequest>),
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordPendingRequest {
    timestamp: String,
    #[serde(with = "EventDraftWire")]
    draft: EventDraft,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryRequest {
    #[serde(with = "EventRowWire")]
    snapshot: EventRow,
    status: FinishStatus,
    inverse: Option<serde_json::Value>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(remote = "EventRow", deny_unknown_fields)]
struct EventRowWire {
    id: String,
    ts: String,
    kind: String,
    skill: String,
    harness: Option<String>,
    scope: Option<String>,
    project_path: Option<String>,
    payload: serde_json::Value,
    inverse: Option<serde_json::Value>,
    backup_dir: Option<String>,
    status: String,
    reverted_by: Option<String>,
    restorable: bool,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(remote = "EventDraft", deny_unknown_fields)]
struct EventDraftWire {
    kind: String,
    skill: String,
    harness: Option<String>,
    scope: Option<String>,
    project_path: Option<String>,
    payload: serde_json::Value,
    inverse: Option<serde_json::Value>,
    backup_dir: Option<String>,
    restorable: bool,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum FinishStatus {
    Done,
    Failed,
}
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum EventAction {
    Apply,
    LookupReceipt,
}
pub struct PreparedEventExchange {
    request: EventRequest,
    command: PreparedEventCommand,
}
enum PreparedEventCommand {
    CopyHistory(Box<copy_commands::PreparedCopyCommand>),
    Restore(Box<restore_commands::PreparedRestoreCommand>),
    Initialize {
        command_id: String,
        event_id: String,
        digest: [u8; 32],
    },
    Preflight([u8; 32]),
    Finish(PreparedFinish),
    Record(Box<PreparedRecord>),
    Recovery(Box<PreparedRecovery>),
}
impl PreparedEventCommand {
    fn digest(&self) -> &[u8] {
        match self {
            Self::Initialize { digest, .. } => digest,
            Self::Preflight(digest) => digest,
            Self::Restore(command) => &command.digest,
            Self::CopyHistory(command) => &command.digest,
            Self::Finish(command) => &command.digest,
            Self::Record(command) => &command.digest,
            Self::Recovery(command) => &command.digest,
        }
    }
    fn apply(&self, connection: &mut Connection) -> Result<CommandReceipt, String> {
        match self {
            Self::Initialize {
                command_id,
                event_id,
                digest,
            } => apply(connection, command_id, event_id, digest, |_| Ok(())),
            Self::Restore(command) => command.apply(connection),
            Self::CopyHistory(command) => command.apply(connection),
            Self::Preflight(_) => Err("Preflight has no mutation receipt".into()),
            Self::Finish(command) => finish(connection, command),
            Self::Record(command) => record(connection, command),
            Self::Recovery(command) => finish_recovery(connection, command),
        }
    }
}
/// Reads one bounded request and requires EOF before authorizing database work.
/// The caller must impose a deadline on the reader and open SQLite only on success.
pub fn read_event_request(
    input: &mut impl std::io::Read,
) -> Result<PreparedEventExchange, EventReply> {
    use crate::skill_history_worker_frame::{finish_history_frames, read_json_frame};
    let rejected = |code| EventReply(ReplyWire::RejectedBeforeOpen { version: 1, code });
    let request =
        read_json_frame(input, 1024 * 1024).map_err(|_| rejected(RejectionCode::InvalidFrame))?;
    finish_history_frames(input).map_err(|_| rejected(RejectionCode::TrailingRequest))?;
    prepare_event_request(request).map_err(|_| rejected(RejectionCode::InvalidCommand))
}

pub fn prepare_event_request(request: EventRequest) -> Result<PreparedEventExchange, String> {
    if request.version != 1 {
        return Err("Unsupported event command version".into());
    }
    validate_ids(&request.exchange_id, &request.command_id)?;
    let command = match &request.operation {
        EventOperation::CopyHistory(transition) => {
            PreparedEventCommand::CopyHistory(Box::new(copy_commands::PreparedCopyCommand::new(
                &request.command_id,
                &request.event_id,
                *transition.clone(),
            )?))
        }
        EventOperation::Restore(operation) => {
            PreparedEventCommand::Restore(Box::new(restore_commands::PreparedRestoreCommand::new(
                &request.command_id,
                &request.event_id,
                *operation.clone(),
            )?))
        }
        EventOperation::Initialize => {
            validate_ids(&request.command_id, &request.event_id)?;
            let bytes = serde_json::to_vec(&("event-initialize-v1", &request.event_id))
                .map_err(|error| error.to_string())?;
            PreparedEventCommand::Initialize {
                command_id: request.command_id.clone(),
                event_id: request.event_id.clone(),
                digest: Sha256::digest(bytes).into(),
            }
        }
        EventOperation::FinishRecovery(recovery) => {
            PreparedEventCommand::Recovery(Box::new(PreparedRecovery::new(
                &request.command_id,
                &request.event_id,
                recovery.snapshot.clone(),
                match recovery.status {
                    FinishStatus::Done => EventStatus::Done,
                    FinishStatus::Failed => EventStatus::Failed,
                },
                recovery.inverse.clone(),
            )?))
        }
        EventOperation::Preflight => {
            if request.action == EventAction::LookupReceipt {
                return Err("Preflight must observe current state".into());
            }
            validate_ids(&request.command_id, &request.event_id)?;
            let encoded = serde_json::to_vec(&("event-preflight-v1", &request.event_id))
                .map_err(|error| error.to_string())?;
            PreparedEventCommand::Preflight(Sha256::digest(encoded).into())
        }
        EventOperation::FinishPending { status, inverse } => {
            PreparedEventCommand::Finish(PreparedFinish::new(
                &request.command_id,
                &request.event_id,
                match status {
                    FinishStatus::Done => EventStatus::Done,
                    FinishStatus::Failed => EventStatus::Failed,
                },
                inverse.clone(),
            )?)
        }
        EventOperation::RecordPending(record) => {
            PreparedEventCommand::Record(Box::new(PreparedRecord::new(
                &request.command_id,
                &request.event_id,
                &record.timestamp,
                record.draft.clone(),
            )?))
        }
    };
    Ok(PreparedEventExchange { request, command })
}
impl PreparedEventExchange {
    pub fn copy_history(
        exchange_id: String,
        command_id: String,
        transition: CopyHistoryTransition,
    ) -> Result<Self, String> {
        prepare_event_request(EventRequest {
            action: EventAction::Apply,
            version: 1,
            exchange_id,
            command_id,
            event_id: transition.event().id.clone(),
            operation: EventOperation::CopyHistory(Box::new(transition)),
        })
    }

    pub fn record_restore(
        exchange_id: String,
        command_id: String,
        event_id: String,
        source: EventRow,
        timestamp: String,
        draft: EventDraft,
    ) -> Result<Self, String> {
        prepare_event_request(EventRequest {
            action: EventAction::Apply,
            version: 1,
            exchange_id,
            command_id,
            event_id,
            operation: EventOperation::Restore(Box::new(
                restore_commands::RestoreOperation::Record {
                    source,
                    record: RecordPendingRequest { timestamp, draft },
                },
            )),
        })
    }

    pub fn finish_restore(
        exchange_id: String,
        command_id: String,
        source: EventRow,
        snapshot: EventRow,
        status: EventStatus,
        inverse: Option<serde_json::Value>,
    ) -> Result<Self, String> {
        prepare_event_request(EventRequest {
            action: EventAction::Apply,
            version: 1,
            exchange_id,
            command_id,
            event_id: snapshot.id.clone(),
            operation: EventOperation::Restore(Box::new(
                restore_commands::RestoreOperation::Finish {
                    source,
                    completion: RecoveryRequest {
                        snapshot,
                        status: match status {
                            EventStatus::Done => FinishStatus::Done,
                            EventStatus::Failed => FinishStatus::Failed,
                        },
                        inverse,
                    },
                },
            )),
        })
    }

    pub fn initialize(
        exchange_id: String,
        command_id: String,
        event_id: String,
    ) -> Result<Self, String> {
        prepare_event_request(EventRequest {
            action: EventAction::Apply,
            version: 1,
            exchange_id,
            command_id,
            event_id,
            operation: EventOperation::Initialize,
        })
    }

    pub fn requires_initialization(&self) -> bool {
        self.request.action == EventAction::Apply
            && matches!(self.command, PreparedEventCommand::Initialize { .. })
    }

    pub fn finish_recovery(
        exchange_id: String,
        command_id: String,
        snapshot: EventRow,
        status: EventStatus,
        inverse: Option<serde_json::Value>,
    ) -> Result<Self, String> {
        prepare_event_request(EventRequest {
            action: EventAction::Apply,
            version: 1,
            exchange_id,
            command_id,
            event_id: snapshot.id.clone(),
            operation: EventOperation::FinishRecovery(Box::new(RecoveryRequest {
                snapshot,
                status: match status {
                    EventStatus::Done => FinishStatus::Done,
                    EventStatus::Failed => FinishStatus::Failed,
                },
                inverse,
            })),
        })
    }

    pub fn preflight(
        exchange_id: String,
        command_id: String,
        event_id: String,
    ) -> Result<Self, String> {
        prepare_event_request(EventRequest {
            action: EventAction::Apply,
            version: 1,
            exchange_id,
            command_id,
            event_id,
            operation: EventOperation::Preflight,
        })
    }

    pub fn record_pending(
        exchange_id: String,
        command_id: String,
        event_id: String,
        timestamp: String,
        draft: EventDraft,
    ) -> Result<Self, String> {
        prepare_event_request(EventRequest {
            action: EventAction::Apply,
            version: 1,
            exchange_id,
            command_id,
            event_id,
            operation: EventOperation::RecordPending(Box::new(RecordPendingRequest {
                timestamp,
                draft,
            })),
        })
    }

    pub fn finish_pending(
        exchange_id: String,
        command_id: String,
        event_id: String,
        status: EventStatus,
        inverse: Option<serde_json::Value>,
    ) -> Result<Self, String> {
        let status = match status {
            EventStatus::Done => FinishStatus::Done,
            EventStatus::Failed => FinishStatus::Failed,
        };
        prepare_event_request(EventRequest {
            action: EventAction::Apply,
            version: 1,
            exchange_id,
            command_id,
            event_id,
            operation: EventOperation::FinishPending { status, inverse },
        })
    }

    pub fn receipt_lookup(&self, exchange_id: &str) -> Result<Self, String> {
        if matches!(self.command, PreparedEventCommand::Preflight(_)) {
            return Err("Preflight must observe current state".into());
        }
        let mut request = self.request.clone();
        request.action = EventAction::LookupReceipt;
        request.exchange_id = exchange_id.into();
        prepare_event_request(request)
    }
    pub fn request(&self) -> &EventRequest {
        &self.request
    }
    pub fn outcome_unknown(&self) -> serde_json::Value {
        serde_json::json!({"version":1,"outcome":"unknown","code":"database_operation_failed","exchange_id":self.request.exchange_id,"command_id":self.request.command_id,"event_id":self.request.event_id,"digest":self.command.digest()})
    }
    pub fn unavailable_reply(&self) -> serde_json::Value {
        let mut reply = self.outcome_unknown();
        reply["code"] = serde_json::json!("reply_unavailable");
        reply
    }
    /// Consumes the worker connection and closes it before returning a result.
    /// The host must still check native resource cleanup and confirm process exit.
    pub fn execute_and_close(
        self,
        mut connection: Connection,
    ) -> Result<serde_json::Value, String> {
        let result = (|| {
            if !connection.is_autocommit() {
                return Err("Event worker requires an idle connection".into());
            }
            if self.requires_initialization() {
                connection
                    .pragma_update(None, "synchronous", "FULL")
                    .map_err(|error| error.to_string())?;
                crate::skill_event_schema::initialize(&connection)?;
            }
            let mode: String = connection
                .pragma_query_value(None, "journal_mode", |row| row.get(0))
                .map_err(|error| error.to_string())?;
            if mode != "wal" {
                return Err("Event worker requires WAL journal mode".into());
            }
            connection
                .pragma_update(None, "synchronous", "FULL")
                .map_err(|error| error.to_string())?;
            self.execute(&mut connection)
        })();
        connection.close().map_err(|(_, error)| error.to_string())?;
        result
    }

    fn execute(self, connection: &mut Connection) -> Result<serde_json::Value, String> {
        if matches!(self.command, PreparedEventCommand::Preflight(_)) {
            let recovery_required =
                crate::skill_event_statements::unresolved_event(connection)?.is_some();
            return Ok(
                serde_json::json!({"version":1,"outcome":"preflight","exchange_id":self.request.exchange_id,"command_id":self.request.command_id,"event_id":self.request.event_id,"digest":self.command.digest(),"recovery_required":recovery_required}),
            );
        }
        let receipt = match self.request.action {
            EventAction::Apply => Some(self.command.apply(connection)?),
            EventAction::LookupReceipt => lookup_receipt(
                connection,
                &self.request.command_id,
                &self.request.event_id,
                self.command.digest(),
            )?,
        };
        let Some(receipt) = receipt else {
            return Ok(
                serde_json::json!({"version":1,"outcome":"receipt_absent","exchange_id":self.request.exchange_id,"command_id":self.request.command_id,"event_id":self.request.event_id,"digest":self.command.digest()}),
            );
        };
        Ok(
            serde_json::json!({"version":1,"outcome":"committed","exchange_id":self.request.exchange_id,"command_id":receipt.command_id,"event_id":receipt.event_id,"digest":receipt.digest}),
        )
    }
}
#[test]
fn framed_finish_validation_precedes_database_access() {
    let valid = serde_json::json!({"version":1,"action":"apply","exchange_id":"exchange","command_id":"command","event_id":"event","operation":{"kind":"finish_pending","status":"done","inverse":null}});
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("exchange_id", serde_json::json!("../outside")),
        ("command_id", serde_json::json!("")),
        ("event_id", serde_json::json!("x".repeat(129))),
    ] {
        let mut invalid = valid.clone();
        if field == "status" {
            invalid["operation"][field] = value;
        } else {
            invalid[field] = value;
        }
        assert!(prepare_event_request(serde_json::from_value(invalid).unwrap()).is_err());
    }
    for (field, value) in [
        ("sql", serde_json::json!("DROP TABLE events")),
        ("status", serde_json::json!("pending")),
        ("action", serde_json::json!("arbitrary_sql")),
    ] {
        let mut invalid = valid.clone();
        if field == "status" {
            invalid["operation"][field] = value;
        } else {
            invalid[field] = value;
        }
        assert!(serde_json::from_value::<EventRequest>(invalid).is_err());
    }
    assert!(prepare_event_request(serde_json::from_value(valid).unwrap()).is_ok());
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct EventReply(ReplyWire);
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
enum ReplyWire {
    Preflight {
        version: u16,
        exchange_id: String,
        command_id: String,
        event_id: String,
        digest: [u8; 32],
        recovery_required: bool,
    },
    ReceiptAbsent {
        version: u16,
        exchange_id: String,
        command_id: String,
        event_id: String,
        digest: [u8; 32],
    },
    Committed {
        version: u16,
        exchange_id: String,
        command_id: String,
        event_id: String,
        digest: [u8; 32],
    },
    Unknown {
        version: u16,
        exchange_id: String,
        command_id: String,
        event_id: String,
        digest: [u8; 32],
        code: DatabaseCode,
    },
    RejectedBeforeOpen {
        version: u16,
        code: RejectionCode,
    },
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum DatabaseCode {
    DatabaseOperationFailed,
    ReplyUnavailable,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum RejectionCode {
    InvalidFrame,
    TrailingRequest,
    InvalidCommand,
}
impl EventReply {
    pub fn validate(
        self,
        expected: Option<&PreparedEventExchange>,
    ) -> Result<serde_json::Value, String> {
        let preflight = expected
            .is_some_and(|request| matches!(request.command, PreparedEventCommand::Preflight(_)));
        if (matches!(self.0, ReplyWire::Preflight { .. }) && !preflight)
            || (preflight
                && matches!(
                    self.0,
                    ReplyWire::Committed { .. } | ReplyWire::ReceiptAbsent { .. }
                ))
        {
            return Err("Reply kind does not match event operation".into());
        }
        if matches!(&self.0, ReplyWire::ReceiptAbsent { .. })
            && !expected
                .is_some_and(|expected| expected.request.action == EventAction::LookupReceipt)
        {
            return Err("Receipt absence requires a lookup request".into());
        }
        match &self.0 {
            ReplyWire::Preflight {
                version,
                exchange_id,
                command_id,
                event_id,
                digest,
                ..
            }
            | ReplyWire::ReceiptAbsent {
                version,
                exchange_id,
                command_id,
                event_id,
                digest,
            }
            | ReplyWire::Committed {
                version,
                exchange_id,
                command_id,
                event_id,
                digest,
            }
            | ReplyWire::Unknown {
                version,
                exchange_id,
                command_id,
                event_id,
                digest,
                ..
            } => {
                let expected = expected.ok_or("Unsolicited event receipt")?;
                if *version != 1
                    || exchange_id != &expected.request.exchange_id
                    || command_id != &expected.request.command_id
                    || event_id != &expected.request.event_id
                    || digest.as_slice() != expected.command.digest()
                {
                    return Err("Mismatched event receipt".into());
                }
            }
            ReplyWire::RejectedBeforeOpen { version, .. } => {
                if *version != 1 {
                    return Err("Unsupported event reply version".into());
                }
            }
        }
        serde_json::to_value(self).map_err(|e| e.to_string())
    }
}
#[test]
fn parent_rejects_mismatched_and_malformed_receipts() {
    let request = prepare_event_request(serde_json::from_value(serde_json::json!({"version":1,"action":"apply","exchange_id":"exchange","command_id":"command","event_id":"event","operation":{"kind":"finish_pending","status":"done","inverse":null}})).unwrap()).unwrap();
    let unknown = request.outcome_unknown();
    assert!(serde_json::from_value::<EventReply>(unknown.clone())
        .unwrap()
        .validate(Some(&request))
        .is_ok());
    assert!(serde_json::from_value::<EventReply>(unknown.clone())
        .unwrap()
        .validate(None)
        .is_err());
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("exchange_id", serde_json::json!("other")),
        ("command_id", serde_json::json!("other")),
        ("event_id", serde_json::json!("other")),
        ("digest", serde_json::json!(vec![0; 32])),
    ] {
        let mut invalid = unknown.clone();
        invalid[field] = value;
        assert!(serde_json::from_value::<EventReply>(invalid)
            .unwrap()
            .validate(Some(&request))
            .is_err());
    }
    for (field, value) in [
        ("extra", serde_json::json!(true)),
        ("digest", serde_json::json!([0])),
        ("code", serde_json::json!("anything")),
        ("outcome", serde_json::json!("success")),
    ] {
        let mut invalid = unknown.clone();
        invalid[field] = value;
        assert!(serde_json::from_value::<EventReply>(invalid).is_err());
    }
}

#[test]
fn receipt_lookup_never_applies_a_missing_or_conflicting_command() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = crate::skill_event_store::EventStore::open(temp.path()).unwrap();
    store
        .record(
            "event",
            EventDraft {
                kind: "fixture".into(),
                skill: "fixture".into(),
                harness: None,
                scope: None,
                project_path: None,
                payload: serde_json::json!({}),
                inverse: None,
                backup_dir: None,
                restorable: false,
            },
        )
        .unwrap();
    let make = |status: &str| {
        prepare_event_request(
            serde_json::from_value(serde_json::json!({
                "version":1,"action":"apply","exchange_id":"apply","command_id":"command",
                "event_id":"event","operation":{"kind":"finish_pending","status":status,"inverse":{"restore":"content"}
            }}))
            .unwrap(),
        )
        .unwrap()
    };
    let apply = make("done");
    store.conn.execute_batch("PRAGMA query_only=ON").unwrap();
    let changes = store.conn.total_changes();
    let absent = apply
        .receipt_lookup("lookup-absent")
        .unwrap()
        .execute(&mut store.conn)
        .unwrap();
    assert_eq!(absent["outcome"], "receipt_absent");
    let reply: EventReply = serde_json::from_value(absent.clone()).unwrap();
    assert!(reply.validate(Some(&apply)).is_err());
    let expected = apply.receipt_lookup("lookup-absent").unwrap();
    assert!(serde_json::from_value::<EventReply>(absent)
        .unwrap()
        .validate(Some(&expected))
        .is_ok());
    assert_eq!(store.conn.total_changes(), changes);
    assert_eq!(store.get("event").unwrap().unwrap().status, "pending");
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM event_command_receipts", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store.conn.execute_batch("PRAGMA query_only=OFF").unwrap();
    apply.execute(&mut store.conn).unwrap();
    store.conn.execute_batch("PRAGMA query_only=ON").unwrap();
    let changes = store.conn.total_changes();
    let found = make("done")
        .receipt_lookup("lookup-found")
        .unwrap()
        .execute(&mut store.conn)
        .unwrap();
    assert_eq!(found["outcome"], "committed");
    assert!(make("failed")
        .receipt_lookup("lookup-conflict")
        .unwrap()
        .execute(&mut store.conn)
        .is_err());
    assert_eq!(store.conn.total_changes(), changes);
    assert_eq!(store.get("event").unwrap().unwrap().status, "done");
}

#[test]
fn record_wire_requires_typed_fields_and_valid_timestamp() {
    let valid = serde_json::json!({"version":1,"action":"apply","exchange_id":"exchange","command_id":"command","event_id":"event","operation":{"kind":"record_pending","timestamp":"2026-09-12T00:00:00Z","draft":{"kind":"fixture","skill":"fixture","harness":null,"scope":null,"project_path":null,"payload":{},"inverse":null,"backup_dir":null,"restorable":false}}});
    assert!(prepare_event_request(serde_json::from_value(valid.clone()).unwrap()).is_ok());
    let mut invalid = valid.clone();
    invalid["operation"]["timestamp"] = serde_json::json!("not-a-timestamp");
    assert!(prepare_event_request(serde_json::from_value(invalid).unwrap()).is_err());
    for location in ["envelope", "operation", "draft"] {
        let mut invalid = valid.clone();
        match location {
            "envelope" => invalid["sql"] = serde_json::json!("ignored?"),
            "operation" => invalid["operation"]["sql"] = serde_json::json!("ignored?"),
            _ => invalid["operation"]["draft"]["sql"] = serde_json::json!("ignored?"),
        }
        assert!(serde_json::from_value::<EventRequest>(invalid).is_err());
    }
}

#[test]
fn worker_connection_policy_refuses_non_wal_and_active_transactions() {
    let prepare = || {
        prepare_event_request(serde_json::from_value(serde_json::json!({
        "version":1,"action":"apply","exchange_id":"exchange","command_id":"command",
        "event_id":"event","operation":{"kind":"finish_pending","status":"done","inverse":null}
    })).unwrap()).unwrap()
    };
    let memory = Connection::open_in_memory().unwrap();
    assert_eq!(
        prepare().execute_and_close(memory).unwrap_err(),
        "Event worker requires WAL journal mode"
    );
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("events.sqlite3");
    let connection = crate::skill_event_store::open(&database).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE; INSERT INTO events(id,ts,kind,skill,payload,status) VALUES ('uncommitted','now','fixture','fixture','{}','pending');").unwrap();
    assert_eq!(
        prepare().execute_and_close(connection).unwrap_err(),
        "Event worker requires an idle connection"
    );
    let reopened = crate::skill_event_store::open(&database).unwrap();
    let counts: (i64, i64) = reopened
        .query_row(
            "SELECT (SELECT count(*) FROM events), (SELECT count(*) FROM event_command_receipts)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(counts, (0, 0));
}

#[test]
fn typed_commands_keep_wire_identity_and_terminal_status() {
    let record = PreparedEventExchange::record_pending(
        "record-exchange".into(),
        "record-command".into(),
        "event".into(),
        "2026-09-12T00:00:00Z".into(),
        EventDraft {
            kind: "repair_skill_frontmatter".into(),
            skill: "alpha".into(),
            harness: None,
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::json!({"intent":1}),
            inverse: None,
            backup_dir: Some("backups/event".into()),
            restorable: true,
        },
    )
    .unwrap();
    let roundtrip = prepare_event_request(
        serde_json::from_slice(&serde_json::to_vec(record.request()).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(record.command.digest(), roundtrip.command.digest());
    let finish = |status| {
        PreparedEventExchange::finish_pending(
            "finish-exchange".into(),
            "finish-command".into(),
            "event".into(),
            status,
            Some(serde_json::json!({"restore":"bytes"})),
        )
    };
    assert_eq!(
        serde_json::to_value(finish(EventStatus::Failed).unwrap().request()).unwrap()["operation"]
            ["status"],
        "failed"
    );
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("events.sqlite3");
    let first = record
        .execute_and_close(crate::skill_event_store::open(&database).unwrap())
        .unwrap();
    assert_eq!(first["outcome"], "committed");
    let done = finish(EventStatus::Done)
        .unwrap()
        .execute_and_close(crate::skill_event_store::open(&database).unwrap())
        .unwrap();
    assert_eq!(done["outcome"], "committed");
    let replay = roundtrip
        .execute_and_close(crate::skill_event_store::open(&database).unwrap())
        .unwrap();
    assert_eq!(first, replay);
    let store = crate::skill_event_store::EventStore::open(temp.path()).unwrap();
    assert_eq!(store.get("event").unwrap().unwrap().status, "done");
}

#[test]
fn preflight_observes_current_state_without_receipts_or_cached_readiness() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = crate::skill_event_store::EventStore::open(temp.path()).unwrap();
    let prepare = || {
        PreparedEventExchange::preflight(
            "exchange".into(),
            "preflight".into(),
            "future-event".into(),
        )
        .unwrap()
    };
    let inspect = |connection: &mut Connection| {
        let changes = connection.total_changes();
        let reply = prepare().execute(connection).unwrap();
        assert_eq!(connection.total_changes(), changes);
        serde_json::from_value::<EventReply>(reply)
            .unwrap()
            .validate(Some(&prepare()))
            .unwrap()
    };
    assert_eq!(inspect(&mut store.conn)["recovery_required"], false);
    store.conn.execute("INSERT INTO events(id,ts,kind,skill,payload,status) VALUES ('older','now','fixture','fixture','{}','interrupted')", []).unwrap();
    assert_eq!(inspect(&mut store.conn)["recovery_required"], true);
    store
        .conn
        .execute("UPDATE events SET status='done' WHERE id='older'", [])
        .unwrap();
    assert_eq!(inspect(&mut store.conn)["recovery_required"], false);
    assert!(prepare().receipt_lookup("lookup").is_err());
    let mut invalid = serde_json::to_value(prepare().request()).unwrap();
    invalid["action"] = serde_json::json!("lookup_receipt");
    assert!(prepare_event_request(serde_json::from_value(invalid).unwrap()).is_err());
    let mut forged = inspect(&mut store.conn);
    forged.as_object_mut().unwrap().remove("recovery_required");
    forged["outcome"] = serde_json::json!("committed");
    assert!(serde_json::from_value::<EventReply>(forged)
        .unwrap()
        .validate(Some(&prepare()))
        .is_err());
    let count: i64 = store
        .conn
        .query_row("SELECT count(*) FROM event_command_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn recovery_completion_checks_snapshot_and_commits_receipt_atomically() {
    for initial in ["pending", "interrupted"] {
        let temp = tempfile::tempdir().unwrap();
        let mut store = crate::skill_event_store::EventStore::open(temp.path()).unwrap();
        store.conn.execute("INSERT INTO events(id,ts,kind,skill,payload,status) VALUES ('event','now','fixture','original','{}',?1)", [initial]).unwrap();
        let snapshot = store.get("event").unwrap().unwrap();
        let prepare = || {
            PreparedEventExchange::finish_recovery(
                "exchange".into(),
                "recovery".into(),
                snapshot.clone(),
                EventStatus::Done,
                Some(serde_json::json!({"restore":"content"})),
            )
            .unwrap()
        };
        let mut wire = serde_json::to_value(prepare().request()).unwrap();
        wire["operation"]["snapshot"]["sql"] = serde_json::json!("unexpected");
        assert!(serde_json::from_value::<EventRequest>(wire).is_err());
        store
            .conn
            .execute("UPDATE events SET skill='changed'", [])
            .unwrap();
        assert!(prepare().execute(&mut store.conn).is_err());
        assert_eq!(store.get("event").unwrap().unwrap().status, initial);
        store
            .conn
            .execute("UPDATE events SET skill='original'", [])
            .unwrap();
        store.conn.execute_batch("CREATE TRIGGER refuse_recovery BEFORE INSERT ON event_command_receipts BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(prepare().execute(&mut store.conn).is_err());
        assert_eq!(store.get("event").unwrap().unwrap().status, initial);
        store
            .conn
            .execute_batch("DROP TRIGGER refuse_recovery")
            .unwrap();
        let committed = prepare().execute(&mut store.conn).unwrap();
        let replayed = prepare().execute(&mut store.conn).unwrap();
        assert_eq!(committed, replayed);
        assert_eq!(committed["outcome"], "committed");
        let completed = store.get("event").unwrap().unwrap();
        assert_eq!(completed.status, "done");
        assert_eq!(
            completed.inverse,
            Some(serde_json::json!({"restore":"content"}))
        );
        assert!(PreparedEventExchange::finish_recovery(
            "exchange".into(),
            "other".into(),
            completed,
            EventStatus::Done,
            None
        )
        .is_err());
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM event_command_receipts", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn pending_event_must_fit_the_recovery_reader_before_any_database_write() {
    let draft = EventDraft {
        kind: "repair_skill_frontmatter".into(),
        skill: "sample".into(),
        harness: None,
        scope: None,
        project_path: None,
        payload: serde_json::json!({"content":"x".repeat(crate::skill_history::MAX_HISTORY_RECORD_BYTES)}),
        inverse: None,
        backup_dir: None,
        restorable: false,
    };
    assert!(PreparedEventExchange::record_pending(
        "exchange".into(),
        "command".into(),
        "event".into(),
        chrono::Utc::now().to_rfc3339(),
        draft
    )
    .is_err());
}

#[test]
fn repair_and_restore_preflight_the_exact_history_record_boundary() {
    fn draft(body: usize, restore: bool) -> EventDraft {
        EventDraft {
            kind: if restore {
                "restore".into()
            } else {
                "repair_skill_frontmatter".into()
            },
            skill: "sample".into(),
            harness: None,
            scope: Some("global".into()),
            project_path: None,
            payload: if restore {
                serde_json::json!({
                    "target_event": "source",
                    "repair": { "proposed_content": "x".repeat(body) },
                    "before": "b",
                    "after": "a"
                })
            } else {
                serde_json::json!({ "repair": { "proposed_content": "x".repeat(body) } })
            },
            inverse: Some(serde_json::json!({"op":"restore_backup"})),
            backup_dir: Some("backups/event".into()),
            restorable: true,
        }
    }
    fn repair_fits(body: usize) -> bool {
        PreparedEventExchange::record_pending(
            "exchange".into(),
            "command".into(),
            "event".into(),
            "2026-09-16T12:34:56.123456789Z".into(),
            draft(body, false),
        )
        .is_ok()
    }
    fn restore_fits(body: usize) -> bool {
        let source = EventRow {
            id: "source".into(),
            ts: "2026-09-16T12:34:56.123456789Z".into(),
            kind: "repair_skill_frontmatter".into(),
            skill: "sample".into(),
            harness: None,
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::json!({ "repair": { "proposed_content": "x".repeat(body) } }),
            inverse: Some(serde_json::json!({"op":"restore_backup"})),
            backup_dir: Some("backups/source".into()),
            status: "done".into(),
            reverted_by: None,
            restorable: true,
        };
        PreparedEventExchange::record_restore(
            "exchange".into(),
            "command".into(),
            "event".into(),
            source,
            "2026-09-16T12:34:56.123456789Z".into(),
            draft(body, true),
        )
        .is_ok()
    }
    fn largest_fitting(mut fits: impl FnMut(usize) -> bool) -> usize {
        let (mut low, mut high) = (0, crate::skill_history::MAX_HISTORY_RECORD_BYTES);
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if fits(middle) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        low
    }

    let repair_max = largest_fitting(repair_fits);
    assert!(repair_fits(repair_max));
    assert!(!repair_fits(repair_max + 1));
    let restore_max = largest_fitting(restore_fits);
    assert!(restore_fits(restore_max));
    assert!(!restore_fits(restore_max + 1));
    assert!(restore_max < repair_max);
    assert!(!restore_fits(repair_max));
}
