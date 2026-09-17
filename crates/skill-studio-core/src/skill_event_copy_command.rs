use super::*;
use crate::skill_repair_recovery_event::{
    CopyRedoRecoveryEvent, CopyRepairRedoSource, CopyRepairUndoSource, CopyUndoRecoveryEvent,
};

/// Database-only transitions. File and backup authority stays with the parent.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "transition", rename_all = "snake_case", deny_unknown_fields)]
pub enum CopyHistoryTransition {
    RecordUndo {
        #[serde(with = "EventRowWire")]
        source: EventRow,
        #[serde(with = "EventRowWire")]
        pending: EventRow,
    },
    FinishUndo {
        #[serde(with = "EventRowWire")]
        source: EventRow,
        #[serde(with = "EventRowWire")]
        pending: EventRow,
        applied: bool,
    },
    RecordRedo {
        #[serde(with = "EventRowWire")]
        source: EventRow,
        #[serde(with = "EventRowWire")]
        undo: EventRow,
        #[serde(with = "EventRowWire")]
        pending: EventRow,
    },
    FinishRedo {
        #[serde(with = "EventRowWire")]
        source: EventRow,
        #[serde(with = "EventRowWire")]
        undo: EventRow,
        #[serde(with = "EventRowWire")]
        pending: EventRow,
        applied: bool,
    },
}

impl CopyHistoryTransition {
    pub fn event(&self) -> &EventRow {
        match self {
            Self::RecordUndo { pending, .. }
            | Self::FinishUndo { pending, .. }
            | Self::RecordRedo { pending, .. }
            | Self::FinishRedo { pending, .. } => pending,
        }
    }

    fn ancestors(&self) -> Vec<&EventRow> {
        match self {
            Self::RecordUndo { source, .. } | Self::FinishUndo { source, .. } => vec![source],
            Self::RecordRedo { source, undo, .. } | Self::FinishRedo { source, undo, .. } => {
                vec![source, undo]
            }
        }
    }

    fn claim(&self) -> &EventRow {
        match self {
            Self::RecordUndo { source, .. } | Self::FinishUndo { source, .. } => source,
            Self::RecordRedo { undo, .. } | Self::FinishRedo { undo, .. } => undo,
        }
    }

    fn completion(&self) -> Option<EventStatus> {
        match self {
            Self::RecordUndo { .. } | Self::RecordRedo { .. } => None,
            Self::FinishUndo { applied, .. } | Self::FinishRedo { applied, .. } => {
                Some(if *applied {
                    EventStatus::Done
                } else {
                    EventStatus::Failed
                })
            }
        }
    }

    fn validate(&self) -> Result<(), String> {
        match self {
            Self::RecordUndo { source, pending } => {
                CopyRepairUndoSource::from_row(source)?;
                let mut claimed = source.clone();
                claimed.reverted_by = Some(pending.id.clone());
                CopyUndoRecoveryEvent::from_rows(&claimed, pending)?;
            }
            Self::FinishUndo {
                source, pending, ..
            } => {
                CopyUndoRecoveryEvent::from_rows(source, pending)?;
            }
            Self::RecordRedo {
                source,
                undo,
                pending,
            } => {
                CopyRepairRedoSource::from_rows(source, undo)?;
                let mut claimed = undo.clone();
                claimed.reverted_by = Some(pending.id.clone());
                CopyRedoRecoveryEvent::from_rows(source, &claimed, pending)?;
            }
            Self::FinishRedo {
                source,
                undo,
                pending,
                ..
            } => {
                CopyRedoRecoveryEvent::from_rows(source, undo, pending)?;
            }
        }
        if self.completion().is_none() && self.event().status != "pending" {
            return Err("New Copy history must start pending".into());
        }
        Ok(())
    }
}

fn draft(row: &EventRow) -> EventDraft {
    EventDraft {
        kind: row.kind.clone(),
        skill: row.skill.clone(),
        harness: row.harness.clone(),
        scope: row.scope.clone(),
        project_path: row.project_path.clone(),
        payload: row.payload.clone(),
        inverse: row.inverse.clone(),
        backup_dir: row.backup_dir.clone(),
        restorable: row.restorable,
    }
}

pub(super) struct PreparedCopyCommand {
    transition: CopyHistoryTransition,
    command_id: String,
    pub(super) digest: Vec<u8>,
}

impl PreparedCopyCommand {
    pub(super) fn new(
        command_id: &str,
        event_id: &str,
        transition: CopyHistoryTransition,
    ) -> Result<Self, String> {
        validate_ids(command_id, event_id)?;
        transition.validate()?;
        let event = transition.event();
        if event.id != event_id {
            return Err("Copy history event identity differs from command".into());
        }
        match transition.completion() {
            None => {
                PreparedRecord::new(command_id, event_id, &event.ts, draft(event))?;
            }
            Some(status) => {
                PreparedRecovery::new(command_id, event_id, event.clone(), status, None)?;
            }
        }
        let encoded = serde_json::to_vec(&("event-copy-history-v1", &transition))
            .map_err(|error| error.to_string())?;
        if encoded.len() > 1024 * 1024 {
            return Err("Command exceeds the 1 MiB limit".into());
        }
        Ok(Self {
            transition,
            command_id: command_id.into(),
            digest: Sha256::digest(encoded).to_vec(),
        })
    }

    pub(super) fn apply(&self, connection: &mut Connection) -> Result<CommandReceipt, String> {
        let event = self.transition.event();
        apply(
            connection,
            &self.command_id,
            &event.id,
            &self.digest,
            |transaction| {
                for expected in self.transition.ancestors() {
                    let current = transaction
                        .query_row(
                            "SELECT * FROM events WHERE id = ?1",
                            [&expected.id],
                            crate::skill_event_statements::row_from,
                        )
                        .optional()
                        .map_err(|error| error.to_string())?
                        .ok_or("Copy history ancestor is missing")?;
                    if serde_json::to_value(current).map_err(|error| error.to_string())?
                        != serde_json::to_value(expected).map_err(|error| error.to_string())?
                    {
                        return Err("Copy history ancestor or source claim changed".into());
                    }
                }
                let claim = self.transition.claim();
                match self.transition.completion() {
                    None => {
                        crate::skill_event_statements::require_recovered(transaction)?;
                        crate::skill_event_statements::insert_pending(
                            transaction,
                            &event.id,
                            &event.ts,
                            draft(event),
                        )?;
                        let changed = transaction.execute(
                        "UPDATE events SET reverted_by = ?1 WHERE id = ?2 AND reverted_by IS NULL AND status = 'done'",
                        params![event.id, claim.id],
                    ).map_err(|error| error.to_string())?;
                        if changed != 1 {
                            return Err("Copy history source is already claimed".into());
                        }
                    }
                    Some(status) => {
                        crate::skill_event_statements::finish_recovery_snapshot(
                            transaction,
                            event,
                            status,
                            None,
                        )?;
                        if matches!(status, EventStatus::Failed) {
                            let changed = transaction.execute(
                            "UPDATE events SET reverted_by = NULL WHERE id = ?1 AND reverted_by = ?2 AND status = 'done'",
                            params![claim.id, event.id],
                        ).map_err(|error| error.to_string())?;
                            if changed != 1 {
                                return Err("Copy history claim changed".into());
                            }
                        }
                    }
                }
                Ok(())
            },
        )
    }
}
