//! Guarded event statements on an already-open compatibility store. This does
//! not confine SQLite sidecar IO or replace the operation's recovery protocol.
use crate::{
    skill_coordination::{FinalizedWriteLease, PreparedContentError},
    skill_event::{EventDraft, EventStatus},
    skill_event_binding::EventConnectionBinding,
    skill_event_store::EventStore,
};
use rusqlite::params;
use serde_json::Value;

#[derive(Debug)]
pub enum EventWriteFailure {
    CancelledBeforeWrite,
    BeforeWrite(String),
    MayHaveWritten(String),
}

impl std::fmt::Display for EventWriteFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CancelledBeforeWrite => {
                formatter.write_str("Event write cancelled before a transaction began")
            }
            Self::BeforeWrite(message) | Self::MayHaveWritten(message) => {
                formatter.write_str(message)
            }
        }
    }
}
impl std::error::Error for EventWriteFailure {}

pub struct GuardedEventStore<'store> {
    store: &'store EventStore,
    binding: EventConnectionBinding<'store>,
}

impl<'store> GuardedEventStore<'store> {
    pub fn bind(
        store: &'store EventStore,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Self, String> {
        Self::bind_prepared(store, lease).map_err(|error| error.to_string())
    }

    pub fn bind_prepared(
        store: &'store EventStore,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Self, PreparedContentError> {
        lease.validate_state_tree_prepared(&store.app_data)?;
        let binding = EventConnectionBinding::bind(&store.conn, &store.app_data)?;
        let result = Self { store, binding };
        result.validate_prepared(lease)?;
        Ok(result)
    }

    fn validate(&self, lease: &FinalizedWriteLease<'_>) -> Result<(), String> {
        self.validate_prepared(lease)
            .map_err(|error| error.to_string())
    }

    fn validate_prepared(
        &self,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<(), PreparedContentError> {
        lease.validate_state_tree_prepared(&self.store.app_data)?;
        self.binding.revalidate()?;
        if !self.store.conn.is_autocommit() {
            return Err("Event writes require their own committed statement".into());
        }
        let synchronous: i64 = self
            .store
            .conn
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .map_err(|error| error.to_string())?;
        let journal_mode: String = self
            .store
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if journal_mode != "wal" {
            return Err("Guarded event writes require WAL journal mode".into());
        }
        if synchronous < 2 {
            return Err("Event writes require SQLite synchronous FULL or EXTRA".into());
        }
        Ok(())
    }

    fn transaction(
        &self,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<rusqlite::Transaction<'_>, String> {
        self.start_transaction(lease)
            .map_err(|error| error.to_string())
    }

    fn start_transaction(
        &self,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<rusqlite::Transaction<'_>, EventWriteFailure> {
        let validate = || {
            self.validate_prepared(lease).map_err(|error| {
                if error.is_cancelled() {
                    EventWriteFailure::CancelledBeforeWrite
                } else {
                    EventWriteFailure::BeforeWrite(error.to_string())
                }
            })
        };
        validate()?;
        let connection = &self.store.conn;
        let timeout_ms: i64 = connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        let timeout =
            std::time::Duration::from_millis(timeout_ms.try_into().map_err(|_| {
                EventWriteFailure::BeforeWrite("Invalid SQLite busy timeout".into())
            })?);
        connection
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        let result = (|| {
            let started = std::time::Instant::now();
            loop {
                validate()?;
                match rusqlite::Transaction::new_unchecked(
                    connection,
                    rusqlite::TransactionBehavior::Immediate,
                ) {
                    Ok(transaction) => return Ok(transaction),
                    Err(error)
                        if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy)
                            && started.elapsed() < timeout =>
                    {
                        std::thread::sleep(
                            timeout
                                .saturating_sub(started.elapsed())
                                .min(std::time::Duration::from_millis(10)),
                        );
                    }
                    Err(error) => return Err(EventWriteFailure::BeforeWrite(error.to_string())),
                }
            }
        })();
        connection
            .busy_timeout(timeout)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        result
    }

    fn write(
        &self,
        lease: &FinalizedWriteLease<'_>,
        statement: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), EventWriteFailure> {
        self.validate(lease)
            .map_err(EventWriteFailure::BeforeWrite)?;
        statement().map_err(EventWriteFailure::MayHaveWritten)?;
        self.validate(lease)
            .map_err(EventWriteFailure::MayHaveWritten)
    }

    fn check_unresolved(&self) -> Result<(), String> {
        crate::skill_event_statements::require_recovered(&self.store.conn)
    }

    pub fn record_pending(
        &self,
        lease: &FinalizedWriteLease<'_>,
        id: &str,
        draft: EventDraft,
    ) -> Result<(), EventWriteFailure> {
        let transaction = self.start_transaction(lease)?;
        self.check_unresolved()
            .map_err(EventWriteFailure::BeforeWrite)?;
        self.store
            .record(id, draft)
            .map_err(EventWriteFailure::MayHaveWritten)?;
        transaction
            .commit()
            .map_err(|error| EventWriteFailure::MayHaveWritten(error.to_string()))?;
        self.validate(lease)
            .map_err(EventWriteFailure::MayHaveWritten)
    }

    pub(crate) fn record_copy_move_reversal(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_event::EventRow,
        id: &str,
        intent: &crate::skill_copy_move_intent::CopyMoveIntent,
    ) -> Result<(), EventWriteFailure> {
        let expected = serde_json::to_value(source)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            self.check_unresolved()?;
            let current = self.store.get(&source.id)?.ok_or("Copy visibility source is missing")?;
            if serde_json::to_value(&current).map_err(|error| error.to_string())? != expected {
                return Err("Copy visibility source changed since preparation".into());
            }
            let source_intent = crate::skill_copy_move_intent::CopyMoveIntent::from_event(source)?;
            self.check_copy_move_claim(source, &source_intent)?;
            let expected_intent = intent.clone().reversing(source)?;
            if serde_json::to_value(&expected_intent).map_err(|error| error.to_string())?
                != serde_json::to_value(intent).map_err(|error| error.to_string())?
            {
                return Err("Copy visibility source reference does not match".into());
            }
            if !crate::skill_backup_reservation::valid_id(id) || id == source.id {
                return Err("Invalid Copy visibility reversal ID".into());
            }
            self.store.record(id, intent.event_draft()?)?;
            let count = transaction
                .execute(
                    "UPDATE events SET reverted_by = ?1 WHERE id = ?2 AND status = 'done' AND reverted_by IS NULL",
                    params![id, &source.id],
                )
                .map_err(|error| error.to_string())?;
            if count != 1 {
                return Err("Copy visibility source is already claimed".into());
            }
            transaction.commit().map_err(|error| error.to_string())
        })
    }

    fn check_copy_move_claim(
        &self,
        row: &crate::skill_event::EventRow,
        intent: &crate::skill_copy_move_intent::CopyMoveIntent,
    ) -> Result<(), String> {
        if let Some(reference) = intent.source() {
            let source = self
                .store
                .get(&reference.event_id)?
                .ok_or("Copy visibility origin is missing")?;
            intent.validate_source_claim(&source, &row.id)?;
        }
        Ok(())
    }

    pub(crate) fn validate_copy_move_claim(
        &self,
        lease: &FinalizedWriteLease<'_>,
        row: &crate::skill_event::EventRow,
        intent: &crate::skill_copy_move_intent::CopyMoveIntent,
    ) -> Result<(), String> {
        self.validate(lease)?;
        self.check_copy_move_claim(row, intent)?;
        self.validate(lease)
    }

    pub(crate) fn finish_copy_move_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        row: &crate::skill_event::EventRow,
        intent: &crate::skill_copy_move_intent::CopyMoveIntent,
        status: EventStatus,
    ) -> Result<(), EventWriteFailure> {
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            self.check_copy_move_claim(row, intent)?;
            crate::skill_event_statements::finish_recovery_snapshot(&transaction, row, status, None)?;
            if matches!(status, EventStatus::Failed) {
                if let Some(reference) = intent.source() {
                    let count = transaction.execute(
                        "UPDATE events SET reverted_by = NULL WHERE id = ?1 AND status = 'done' AND reverted_by = ?2",
                        params![&reference.event_id, &row.id],
                    ).map_err(|error| error.to_string())?;
                    if count != 1 {
                        return Err("Copy visibility source claim changed".into());
                    }
                }
            }
            transaction.commit().map_err(|error| error.to_string())
        })
    }

    pub(crate) fn replace_pending_payload(
        &self,
        lease: &FinalizedWriteLease<'_>,
        expected: &crate::skill_event::EventRow,
        proposed: Value,
    ) -> Result<crate::skill_event::EventRow, EventWriteFailure> {
        if !matches!(expected.status.as_str(), "pending" | "interrupted")
            || expected.reverted_by.is_some()
        {
            return Err(EventWriteFailure::BeforeWrite(
                "Event is not eligible for a payload transition".into(),
            ));
        }
        let expected_value = serde_json::to_value(expected)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        let proposed = serde_json::to_string(&proposed)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        let mut updated = None;
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            let current = self
                .store
                .get(&expected.id)?
                .ok_or("Pending event disappeared before its payload transition")?;
            if serde_json::to_value(&current).map_err(|error| error.to_string())?
                != expected_value
            {
                return Err("Pending event changed before its payload transition".into());
            }
            let changed = transaction
                .execute(
                    "UPDATE events SET payload = ?1 WHERE id = ?2 AND status = ?3 AND status IN ('pending', 'interrupted') AND reverted_by IS NULL",
                    params![proposed, current.id, current.status],
                )
                .map_err(|error| error.to_string())?;
            if changed != 1 {
                return Err("Event is no longer eligible for a payload transition".into());
            }
            updated = Some(
                self.store
                    .get(&expected.id)?
                    .ok_or("Updated event disappeared")?,
            );
            transaction.commit().map_err(|error| error.to_string())
        })?;
        updated.ok_or_else(|| {
            EventWriteFailure::MayHaveWritten("Event payload transition receipt is missing".into())
        })
    }

    pub(crate) fn finish_recovery_snapshot(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_event::EventRow,
        status: EventStatus,
        inverse: Option<Value>,
    ) -> Result<(), EventWriteFailure> {
        let inverse = inverse
            .map(|value| serde_json::to_string(&value))
            .transpose()
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            crate::skill_event_statements::finish_recovery_snapshot(
                &transaction,
                event,
                status,
                inverse.as_deref(),
            )?;
            transaction.commit().map_err(|error| error.to_string())
        })
    }

    pub(crate) fn advance_dotagents_unfork_provider(
        &self,
        lease: &FinalizedWriteLease<'_>,
        expected: &crate::skill_unfork_preparation::PendingDotagentsUnforkEvent,
        next: crate::skill_unfork_preparation::UnforkProviderState,
    ) -> Result<crate::skill_unfork_preparation::PendingDotagentsUnforkEvent, EventWriteFailure>
    {
        let payload = serde_json::to_value(
            expected
                .advance_payload(next)
                .map_err(EventWriteFailure::BeforeWrite)?,
        )
        .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        let current = self.replace_pending_payload(lease, expected.event(), payload)?;
        crate::skill_unfork_preparation::PendingDotagentsUnforkEvent::from_row(&current)
            .map_err(EventWriteFailure::MayHaveWritten)
    }

    pub(crate) fn advance_skills_sh_unfork_provider(
        &self,
        lease: &FinalizedWriteLease<'_>,
        expected: &crate::skill_unfork_preparation::PendingSkillsShUnforkEvent,
        next: crate::skill_unfork_preparation::SkillsShUnforkProviderState,
    ) -> Result<crate::skill_unfork_preparation::PendingSkillsShUnforkEvent, EventWriteFailure>
    {
        let payload = expected
            .advance_payload(next)
            .map_err(EventWriteFailure::BeforeWrite)?;
        let current = self.replace_pending_payload(lease, expected.event(), payload)?;
        crate::skill_unfork_preparation::PendingSkillsShUnforkEvent::from_row(&current)
            .map_err(EventWriteFailure::MayHaveWritten)
    }

    pub(crate) fn resolve_unstarted_dotagents_unfork(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_unfork_preparation::PendingDotagentsUnforkEvent,
    ) -> Result<(), EventWriteFailure> {
        self.finish_recovery_snapshot(lease, event.event(), EventStatus::Failed, None)
    }

    pub(crate) fn abandon_may_have_started_dotagents_unfork(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_unfork_preparation::PendingDotagentsUnforkEvent,
    ) -> Result<(), EventWriteFailure> {
        self.finish_recovery_snapshot(lease, event.event(), EventStatus::Failed, None)
    }
}

impl GuardedEventStore<'_> {
    pub fn next_recovery_event(
        &self,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<Option<crate::skill_event::EventRow>, String> {
        self.validate(lease)?;
        let id = crate::skill_event_statements::unresolved_event(&self.store.conn)?;
        let row = id.map(|id| self.store.get(&id)).transpose()?.flatten();
        self.validate(lease)?;
        Ok(row)
    }

    pub fn finish_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_repair_recovery_event::RepairRecoveryEvent,
        status: EventStatus,
        inverse: Option<Value>,
    ) -> Result<(), EventWriteFailure> {
        self.finish_recovery_snapshot(lease, event.snapshot(), status, inverse)
    }

    /// Records a non-Undo restore and claims its completed source in the same
    /// transaction. The source remains claimed through uncertain publication;
    /// callers may release it only after a proven pre-publication failure.
    pub(crate) fn record_trial_backup_restore(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_event::EventRow,
        operation_id: &str,
        draft: EventDraft,
    ) -> Result<(crate::skill_event::EventRow, crate::skill_event::EventRow), EventWriteFailure>
    {
        if operation_id.len() != 26 || ulid::Ulid::from_string(operation_id).is_err() {
            return Err(EventWriteFailure::BeforeWrite(
                "Invalid trial backup restore operation ID".into(),
            ));
        }
        let expected = serde_json::to_value(source)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        let mut recorded = None;
        let transaction = self.start_transaction(lease)?;
        (|| {
            self.check_unresolved()?;
            let current = self.store.get(&source.id)?.ok_or("Trial backup source is missing")?;
            if serde_json::to_value(&current).map_err(|error| error.to_string())? != expected {
                return Err("Trial backup source changed before recording".into());
            }
            self.store.record(operation_id, draft)?;
            let changed = transaction.execute(
                "UPDATE events SET reverted_by = ?1 WHERE id = ?2 AND status = 'done' AND reverted_by IS NULL",
                params![operation_id, source.id],
            ).map_err(|error| error.to_string())?;
            if changed != 1 { return Err("Trial backup source cannot be claimed".into()); }
            let mut claimed = current;
            claimed.reverted_by = Some(operation_id.into());
            recorded = Some((claimed, self.store.get(operation_id)?.ok_or("Trial backup restore event is missing")?));
            transaction.commit().map_err(|error| error.to_string())
        })()
        .map_err(EventWriteFailure::MayHaveWritten)?;
        self.validate(lease)
            .map_err(EventWriteFailure::MayHaveWritten)?;
        recorded.ok_or_else(|| {
            EventWriteFailure::MayHaveWritten("Trial backup restore receipt is missing".into())
        })
    }

    pub(crate) fn cancel_trial_backup_restore(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_event::EventRow,
        event: &crate::skill_event::EventRow,
    ) -> Result<(), EventWriteFailure> {
        let expected = serde_json::to_value(source)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            let current = self
                .store
                .get(&source.id)?
                .ok_or("Trial backup source is missing")?;
            if serde_json::to_value(current).map_err(|error| error.to_string())? != expected
                || source.reverted_by.as_deref() != Some(event.id.as_str())
            {
                return Err("Trial backup restore claim changed before cancellation".into());
            }
            crate::skill_event_statements::finish_recovery_snapshot(
                &transaction,
                event,
                EventStatus::Failed,
                None,
            )?;
            let changed = transaction
                .execute(
                    "UPDATE events SET reverted_by = NULL WHERE id = ?1 AND reverted_by = ?2",
                    params![source.id, event.id],
                )
                .map_err(|error| error.to_string())?;
            if changed != 1 {
                return Err("Trial backup source claim could not be released".into());
            }
            transaction.commit().map_err(|error| error.to_string())
        })
    }

    pub(crate) fn finish_trial_backup_restore(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_event::EventRow,
        event: &crate::skill_event::EventRow,
    ) -> Result<(), EventWriteFailure> {
        let expected = serde_json::to_value(source)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            let current = self
                .store
                .get(&source.id)?
                .ok_or("Trial backup source is missing")?;
            if serde_json::to_value(&current).map_err(|error| error.to_string())? != expected
                || source.reverted_by.as_deref() != Some(event.id.as_str())
            {
                return Err("Trial backup restore claim changed before completion".into());
            }
            crate::skill_event_statements::finish_recovery_snapshot(
                &transaction,
                event,
                EventStatus::Done,
                None,
            )?;
            transaction.commit().map_err(|error| error.to_string())
        })
    }

    pub(crate) fn require_trial_backup_restore(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_event::EventRow,
        event: &crate::skill_event::EventRow,
    ) -> Result<(), EventWriteFailure> {
        self.validate(lease)
            .map_err(EventWriteFailure::BeforeWrite)?;
        let current_source = self
            .store
            .get(&source.id)
            .map_err(EventWriteFailure::BeforeWrite)?
            .ok_or_else(|| {
                EventWriteFailure::BeforeWrite("Trial backup source is missing".into())
            })?;
        let current_event = self
            .store
            .get(&event.id)
            .map_err(EventWriteFailure::BeforeWrite)?
            .ok_or_else(|| {
                EventWriteFailure::BeforeWrite("Trial backup restore event is missing".into())
            })?;
        if serde_json::to_value(current_source)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?
            != serde_json::to_value(source)
                .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?
            || serde_json::to_value(current_event)
                .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?
                != serde_json::to_value(event)
                    .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?
            || source.reverted_by.as_deref() != Some(event.id.as_str())
        {
            return Err(EventWriteFailure::BeforeWrite(
                "Trial backup restore history changed before its next effect".into(),
            ));
        }
        self.validate(lease).map_err(EventWriteFailure::BeforeWrite)
    }

    pub(crate) fn advance_trial_backup_restore(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_event::EventRow,
        event: &crate::skill_event::EventRow,
        payload: Value,
    ) -> Result<crate::skill_event::EventRow, EventWriteFailure> {
        let expected = serde_json::to_value(event)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        let payload = serde_json::to_string(&payload)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        let mut updated = None;
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            let current_source = self
                .store
                .get(&source.id)?
                .ok_or("Trial backup restore source is missing")?;
            let current = self.store.get(&event.id)?.ok_or("Trial backup restore event is missing")?;
            if serde_json::to_value(&current_source).map_err(|error| error.to_string())?
                != serde_json::to_value(source).map_err(|error| error.to_string())?
                || source.reverted_by.as_deref() != Some(event.id.as_str())
                || serde_json::to_value(&current).map_err(|error| error.to_string())? != expected
            {
                return Err("Trial backup restore changed before phase update".into());
            }
            let changed = transaction.execute(
                "UPDATE events SET payload = ?1 WHERE id = ?2 AND status IN ('pending', 'interrupted')",
                params![payload, event.id],
            ).map_err(|error| error.to_string())?;
            if changed != 1 { return Err("Trial backup restore is no longer eligible for phase update".into()); }
            updated = Some(self.store.get(&event.id)?.ok_or("Updated trial backup restore is missing")?);
            transaction.commit().map_err(|error| error.to_string())
        })?;
        updated.ok_or_else(|| {
            EventWriteFailure::MayHaveWritten("Trial backup restore phase result is missing".into())
        })
    }

    pub fn record_copy_undo(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_repair_recovery_event::CopyRepairUndoSource,
        undo_id: &str,
    ) -> Result<RecordedCopyUndo, EventWriteFailure> {
        if !crate::skill_backup_reservation::valid_id(undo_id) || undo_id == source.id() {
            return Err(EventWriteFailure::BeforeWrite(
                "Invalid copy undo ID".into(),
            ));
        }
        let expected = serde_json::to_value(source.snapshot())
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        let mut recorded = None;
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            self.check_unresolved()?;
            let current = self.store.get(source.id())?.ok_or("Copy undo source is missing")?;
            if serde_json::to_value(&current).map_err(|error| error.to_string())? != expected {
                return Err("Copy undo source changed since preparation".into());
            }
            self.store.record(undo_id, EventDraft {
                kind: "undo_copy_frontmatter".into(), skill: current.skill, harness: current.harness,
                scope: current.scope, project_path: current.project_path,
                payload: serde_json::json!({"target_event": source.id(), "repair": source.intent()}),
                inverse: None, backup_dir: Some(format!("backups/{undo_id}")), restorable: false,
            })?;
            let count = transaction.execute("UPDATE events SET reverted_by = ?1 WHERE id = ?2 AND reverted_by IS NULL AND status = 'done'", params![undo_id, source.id()]).map_err(|error| error.to_string())?;
            if count != 1 { return Err("Copy undo source is no longer available".into()); }
            let mut expected_source = source.snapshot().clone();
            expected_source.reverted_by = Some(undo_id.into());
            recorded = Some(RecordedCopyUndo { source: expected_source,
                undo: self.store.get(undo_id)?.ok_or("Recorded undo is missing")? });
            transaction.commit().map_err(|error| error.to_string())
        })?;
        recorded
            .ok_or_else(|| EventWriteFailure::MayHaveWritten("Copy undo receipt is missing".into()))
    }

    pub fn record_copy_redo(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_repair_recovery_event::CopyRepairRedoSource,
        intent: &crate::skill_copy_repair::CopyRepairRedoIntent,
        redo_id: &str,
    ) -> Result<RecordedCopyRedo, EventWriteFailure> {
        if !crate::skill_backup_reservation::valid_id(redo_id)
            || redo_id == source.source().id
            || redo_id == source.undo().id
        {
            return Err(EventWriteFailure::BeforeWrite(
                "Invalid copy redo ID".into(),
            ));
        }
        intent
            .validate_source(source)
            .map_err(EventWriteFailure::BeforeWrite)?;
        let payload = serde_json::to_value(intent)
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        let mut recorded = None;
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            self.check_unresolved()?;
            self.validate_copy_undo_pair(source.source(), source.undo())?;
            let original = source.source();
            self.store.record(redo_id, EventDraft {
                kind: "redo_copy_frontmatter".into(), skill: original.skill.clone(), harness: original.harness.clone(),
                scope: original.scope.clone(), project_path: original.project_path.clone(), payload,
                inverse: None, backup_dir: Some(format!("backups/{redo_id}")), restorable: false,
            })?;
            let count = transaction.execute("UPDATE events SET reverted_by = ?1 WHERE id = ?2 AND reverted_by IS NULL AND status = 'done'", params![redo_id, &source.undo().id]).map_err(|error| error.to_string())?;
            if count != 1 { return Err("Copy redo undo source is no longer available".into()); }
            let mut undo = source.undo().clone();
            undo.reverted_by = Some(redo_id.into());
            recorded = Some(RecordedCopyRedo { source: original.clone(), undo,
                redo: self.store.get(redo_id)?.ok_or("Recorded redo is missing")? });
            transaction.commit().map_err(|error| error.to_string())
        })?;
        recorded
            .ok_or_else(|| EventWriteFailure::MayHaveWritten("Copy redo receipt is missing".into()))
    }

    pub fn finish_copy_redo(
        &self,
        lease: &FinalizedWriteLease<'_>,
        recorded: &RecordedCopyRedo,
    ) -> Result<(), EventWriteFailure> {
        self.finish_copy_redo_rows(
            lease,
            &recorded.source,
            &recorded.undo,
            &recorded.redo,
            true,
        )
    }

    pub fn read_copy_redo_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        redo: &crate::skill_event::EventRow,
    ) -> Result<crate::skill_repair_recovery_event::CopyRedoRecoveryEvent, String> {
        self.validate(lease)?;
        let source_id = redo
            .payload
            .get("source_event")
            .and_then(Value::as_str)
            .ok_or("Redo source ID is missing")?;
        let undo_id = redo
            .payload
            .get("undo_event")
            .and_then(Value::as_str)
            .ok_or("Redo undo ID is missing")?;
        let source = self.store.get(source_id)?.ok_or("Redo source is missing")?;
        let undo = self.store.get(undo_id)?.ok_or("Redo undo is missing")?;
        let event = crate::skill_repair_recovery_event::CopyRedoRecoveryEvent::from_rows(
            &source, &undo, redo,
        )?;
        self.validate_copy_redo_recovery(lease, &event)?;
        Ok(event)
    }

    pub fn validate_copy_redo_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_repair_recovery_event::CopyRedoRecoveryEvent,
    ) -> Result<(), String> {
        self.validate(lease)?;
        self.validate_copy_redo_rows(event.source(), event.undo(), event.redo())?;
        self.validate(lease)
    }

    fn validate_copy_redo_rows(
        &self,
        source: &crate::skill_event::EventRow,
        undo: &crate::skill_event::EventRow,
        redo: &crate::skill_event::EventRow,
    ) -> Result<(), String> {
        self.validate_copy_undo_pair(source, undo)?;
        let current = self
            .store
            .get(&redo.id)?
            .ok_or("Copy redo event is missing")?;
        if serde_json::to_value(current).map_err(|error| error.to_string())?
            != serde_json::to_value(redo).map_err(|error| error.to_string())?
        {
            return Err("Copy redo event changed".into());
        }
        Ok(())
    }

    pub fn finish_copy_redo_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_repair_recovery_event::CopyRedoRecoveryEvent,
        applied: bool,
    ) -> Result<(), EventWriteFailure> {
        self.finish_copy_redo_rows(lease, event.source(), event.undo(), event.redo(), applied)
    }

    fn finish_copy_redo_rows(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_event::EventRow,
        undo: &crate::skill_event::EventRow,
        redo: &crate::skill_event::EventRow,
        applied: bool,
    ) -> Result<(), EventWriteFailure> {
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            self.validate_copy_redo_rows(source, undo, redo)?;
            let status = if applied { "done" } else { "failed" };
            let count = transaction.execute("UPDATE events SET status = ?1 WHERE id = ?2 AND status = ?3", params![status, &redo.id, &redo.status]).map_err(|error| error.to_string())?;
            if count != 1 { return Err("Copy redo is no longer eligible".into()); }
            if !applied {
                let count = transaction.execute("UPDATE events SET reverted_by = NULL WHERE id = ?1 AND reverted_by = ?2 AND status = 'done'", params![&undo.id, &redo.id]).map_err(|error| error.to_string())?;
                if count != 1 { return Err("Copy redo undo claim changed".into()); }
            }
            transaction.commit().map_err(|error| error.to_string())
        })
    }

    pub fn finish_copy_undo(
        &self,
        lease: &FinalizedWriteLease<'_>,
        recorded: &RecordedCopyUndo,
    ) -> Result<(), EventWriteFailure> {
        self.finish_copy_undo_pair(lease, &recorded.source, &recorded.undo, true)
    }

    pub fn read_copy_undo_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        undo: &crate::skill_event::EventRow,
    ) -> Result<crate::skill_repair_recovery_event::CopyUndoRecoveryEvent, String> {
        self.validate(lease)?;
        let source_id = undo
            .payload
            .get("target_event")
            .and_then(Value::as_str)
            .ok_or("Copy undo source ID is missing")?;
        let source = self
            .store
            .get(source_id)?
            .ok_or("Copy undo source is missing")?;
        let event =
            crate::skill_repair_recovery_event::CopyUndoRecoveryEvent::from_rows(&source, undo)?;
        self.validate_copy_undo_recovery(lease, &event)?;
        Ok(event)
    }

    pub fn validate_copy_undo_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_repair_recovery_event::CopyUndoRecoveryEvent,
    ) -> Result<(), String> {
        self.validate(lease)?;
        self.validate_copy_undo_pair(event.source(), event.undo())?;
        self.validate(lease)
    }

    pub fn finish_copy_undo_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_repair_recovery_event::CopyUndoRecoveryEvent,
        applied: bool,
    ) -> Result<(), EventWriteFailure> {
        self.finish_copy_undo_pair(lease, event.source(), event.undo(), applied)
    }

    fn validate_copy_undo_pair(
        &self,
        source: &crate::skill_event::EventRow,
        undo: &crate::skill_event::EventRow,
    ) -> Result<(), String> {
        for expected in [source, undo] {
            let current = self
                .store
                .get(&expected.id)?
                .ok_or("Copy undo event is missing")?;
            if serde_json::to_value(current).map_err(|error| error.to_string())?
                != serde_json::to_value(expected).map_err(|error| error.to_string())?
            {
                return Err("Copy undo event or source claim changed".into());
            }
        }
        Ok(())
    }

    fn finish_copy_undo_pair(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_event::EventRow,
        undo: &crate::skill_event::EventRow,
        applied: bool,
    ) -> Result<(), EventWriteFailure> {
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            self.validate_copy_undo_pair(source, undo)?;
            let status = if applied { "done" } else { "failed" };
            let count = transaction.execute(
                "UPDATE events SET status = ?1 WHERE id = ?2 AND status = ?3",
                params![status, &undo.id, &undo.status],
            ).map_err(|error| error.to_string())?;
            if count != 1 { return Err("Copy undo is no longer eligible".into()); }
            if !applied {
                let count = transaction.execute(
                    "UPDATE events SET reverted_by = NULL WHERE id = ?1 AND reverted_by = ?2 AND status = 'done'",
                    params![&source.id, &undo.id],
                ).map_err(|error| error.to_string())?;
                if count != 1 { return Err("Copy undo source claim changed".into()); }
            }
            transaction.commit().map_err(|error| error.to_string())
        })
    }

    pub(crate) fn validate_copy_document_source(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_copy_document_edit::CopyDocumentEditSource,
    ) -> Result<(), String> {
        self.validate(lease)?;
        self.check_copy_document_source(source)?;
        self.validate(lease)
    }

    fn check_copy_document_source(
        &self,
        source: &crate::skill_copy_document_edit::CopyDocumentEditSource,
    ) -> Result<(), String> {
        let current = self
            .store
            .get(&source.snapshot().id)?
            .ok_or("Copy edit source is missing")?;
        if serde_json::to_value(&current).map_err(|e| e.to_string())?
            != serde_json::to_value(source.snapshot()).map_err(|e| e.to_string())?
            || current.status != "done"
            || current.reverted_by.is_some()
        {
            return Err("Copy edit source changed or was already reversed".into());
        }
        if let Some(reference) = source.origin() {
            let origin = self
                .store
                .get(&reference.event_id)?
                .ok_or("Copy edit origin is missing")?;
            reference.validate_claim(&origin, &current.id, &current.kind, source.intent())?;
        }
        Ok(())
    }

    pub(crate) fn record_copy_document_reversal(
        &self,
        lease: &FinalizedWriteLease<'_>,
        source: &crate::skill_copy_document_edit::CopyDocumentEditSource,
        id: &str,
        intent: &crate::skill_copy_document_edit::CopyDocumentEditIntent,
    ) -> Result<(), EventWriteFailure> {
        let draft = source
            .reversal_draft(id, intent)
            .map_err(EventWriteFailure::BeforeWrite)?;
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            self.check_unresolved()?;
            self.check_copy_document_source(source)?;
            self.store.record(id, draft)?;
            let count = transaction.execute(
                "UPDATE events SET reverted_by = ?1 WHERE id = ?2 AND status = 'done' AND reverted_by IS NULL",
                params![id, &source.snapshot().id],
            ).map_err(|e| e.to_string())?;
            if count != 1 { return Err("Copy document source is no longer available".into()); }
            transaction.commit().map_err(|e| e.to_string())
        })
    }

    pub(crate) fn validate_copy_document_edit_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_copy_document_edit::CopyDocumentEditRecoveryEvent,
    ) -> Result<(), String> {
        self.validate(lease)?;
        self.check_copy_document_edit_recovery(event)?;
        self.validate(lease)
    }

    fn check_copy_document_edit_recovery(
        &self,
        event: &crate::skill_copy_document_edit::CopyDocumentEditRecoveryEvent,
    ) -> Result<(), String> {
        let current = self
            .store
            .get(&event.snapshot().id)?
            .ok_or("Copy edit recovery event is missing")?;
        if serde_json::to_value(current).map_err(|error| error.to_string())?
            != serde_json::to_value(event.snapshot()).map_err(|error| error.to_string())?
        {
            return Err("Copy edit event changed since preparation".into());
        }
        if let Some(reference) = event.source() {
            let source = self
                .store
                .get(&reference.event_id)?
                .ok_or("Copy edit source is missing")?;
            reference.validate_claim(
                &source,
                &event.snapshot().id,
                &event.snapshot().kind,
                event.intent(),
            )?;
        }
        Ok(())
    }

    pub(crate) fn finish_copy_document_edit_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_copy_document_edit::CopyDocumentEditRecoveryEvent,
        status: EventStatus,
    ) -> Result<(), EventWriteFailure> {
        self.write(lease, || {
            let transaction = self.transaction(lease)?;
            self.check_copy_document_edit_recovery(event)?;
            crate::skill_event_statements::finish_recovery_snapshot(&transaction, event.snapshot(), status, None)?;
            if matches!(status, EventStatus::Failed) {
                if let Some(reference) = event.source() {
                    let count = transaction.execute(
                        "UPDATE events SET reverted_by = NULL WHERE id = ?1 AND status = 'done' AND reverted_by = ?2",
                        params![&reference.event_id, &event.snapshot().id],
                    ).map_err(|e| e.to_string())?;
                    if count != 1 { return Err("Copy edit source claim changed".into()); }
                }
            }
            transaction.commit().map_err(|e| e.to_string())
        })
    }

    pub fn validate_copy_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_repair_recovery_event::CopyRepairRecoveryEvent,
    ) -> Result<(), String> {
        self.validate(lease)?;
        let current = self
            .store
            .get(event.id())?
            .ok_or("Copy recovery event is missing")?;
        if serde_json::to_value(current).map_err(|error| error.to_string())?
            != serde_json::to_value(event.snapshot()).map_err(|error| error.to_string())?
        {
            return Err("Copy recovery event changed since preparation".into());
        }
        self.validate(lease)
    }

    pub fn finish_copy_recovery(
        &self,
        lease: &FinalizedWriteLease<'_>,
        event: &crate::skill_repair_recovery_event::CopyRepairRecoveryEvent,
        status: EventStatus,
    ) -> Result<(), EventWriteFailure> {
        self.finish_recovery_snapshot(lease, event.snapshot(), status, None)
    }

    pub fn finish_pending(
        &self,
        lease: &FinalizedWriteLease<'_>,
        id: &str,
        status: EventStatus,
        inverse: Option<Value>,
    ) -> Result<(), EventWriteFailure> {
        let inverse = inverse
            .map(|value| serde_json::to_string(&value))
            .transpose()
            .map_err(|error| EventWriteFailure::BeforeWrite(error.to_string()))?;
        self.write(lease, || {
            crate::skill_event_statements::finish_pending(
                &self.store.conn,
                id,
                status,
                inverse.as_deref(),
            )
        })
    }
}

#[derive(Debug)]
pub struct RecordedCopyUndo {
    source: crate::skill_event::EventRow,
    undo: crate::skill_event::EventRow,
}
#[derive(Debug)]
pub struct RecordedCopyRedo {
    source: crate::skill_event::EventRow,
    undo: crate::skill_event::EventRow,
    redo: crate::skill_event::EventRow,
}

impl GuardedEventStore<'_> {
    pub fn require_recovered(&self, lease: &FinalizedWriteLease<'_>) -> Result<(), String> {
        self.require_recovered_prepared(lease)
            .map_err(|error| error.to_string())
    }
}

impl GuardedEventStore<'_> {
    pub fn require_recovered_prepared(
        &self,
        lease: &FinalizedWriteLease<'_>,
    ) -> Result<(), PreparedContentError> {
        self.validate_prepared(lease)?;
        self.check_unresolved()?;
        self.validate_prepared(lease)
    }
}
