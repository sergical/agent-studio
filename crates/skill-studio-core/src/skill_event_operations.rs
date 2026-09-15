//! Guarded event statements on an already-open compatibility store. This does
//! not confine SQLite sidecar IO or replace the operation's recovery protocol.
use crate::{
    skill_coordination::{FinalizedWriteLease, PreparedContentError},
    skill_event::{EventDraft, EventStatus},
    skill_event_binding::EventConnectionBinding,
    skill_event_store::EventStore,
};
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
}
