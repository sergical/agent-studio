//! Database statements shared by guarded callers and the future event worker.
//! Callers own authorization, transaction boundaries and recovery decisions.
use crate::skill_event::{EventDraft, EventRow, EventStatus};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

pub(crate) fn insert_pending(
    connection: &Connection,
    id: &str,
    ts: &str,
    draft: EventDraft,
) -> Result<(), String> {
    let payload_json = serde_json::to_string(&draft.payload)
        .map_err(|e| format!("Failed to serialize payload: {e}"))?;
    let inverse_json = draft
        .inverse
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| format!("Failed to serialize inverse: {e}"))?;
    connection
            .execute(
                "INSERT INTO events
                    (id, ts, kind, skill, harness, scope, project_path, payload, inverse, backup_dir, status, reverted_by, restorable)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', NULL, ?11)",
                params![
                    id,
                    ts,
                    draft.kind,
                    draft.skill,
                    draft.harness,
                    draft.scope,
                    draft.project_path,
                    payload_json,
                    inverse_json,
                    draft.backup_dir,
                    draft.restorable,
                ],
            )
            .map_err(|e| format!("Failed to insert event {id}: {e}"))?;
    Ok(())
}

pub(crate) fn require_recovered(connection: &Connection) -> Result<(), String> {
    let id = unresolved_event(connection)?;
    if let Some(id) = id {
        return Err(format!(
            "Event {id} requires recovery before a new mutation"
        ));
    }
    Ok(())
}

pub(crate) fn unresolved_event(connection: &Connection) -> Result<Option<String>, String> {
    connection.query_row(
            "SELECT event.id FROM events AS event WHERE event.status IN ('pending', 'interrupted') AND NOT EXISTS (SELECT 1 FROM events AS restoration WHERE restoration.id = event.reverted_by AND restoration.status = 'done') ORDER BY event.rowid LIMIT 1",
            [], |row| row.get(0),
        ).optional().map_err(|error| error.to_string())
}

pub(crate) fn row_from(row: &rusqlite::Row) -> rusqlite::Result<EventRow> {
    let payload_str: String = row.get("payload")?;
    let inverse_str: Option<String> = row.get("inverse")?;
    Ok(EventRow {
        id: row.get("id")?,
        ts: row.get("ts")?,
        kind: row.get("kind")?,
        skill: row.get("skill")?,
        harness: row.get("harness")?,
        scope: row.get("scope")?,
        project_path: row.get("project_path")?,
        payload: serde_json::from_str(&payload_str).unwrap_or(Value::Null),
        inverse: inverse_str.map(|s| serde_json::from_str(&s).unwrap_or(Value::Null)),
        backup_dir: row.get("backup_dir")?,
        status: row.get("status")?,
        reverted_by: row.get("reverted_by")?,
        restorable: row.get("restorable")?,
    })
}

pub(crate) fn finish_recovery_snapshot(
    connection: &Connection,
    event: &EventRow,
    status: EventStatus,
    inverse: Option<&str>,
) -> Result<(), String> {
    let expected = serde_json::to_value(event).map_err(|error| error.to_string())?;
    let current = connection
        .query_row("SELECT * FROM events WHERE id = ?1", [&event.id], row_from)
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or("Recovery event is missing")?;
    if serde_json::to_value(current).map_err(|error| error.to_string())? != expected {
        return Err("Recovery event changed since preparation".into());
    }
    let affected = connection.execute(
        "UPDATE events SET status = ?1, inverse = ?2 WHERE id = ?3 AND status = ?4 AND reverted_by IS NULL",
        params![status.as_str(), inverse, &event.id, &event.status],
    ).map_err(|error| error.to_string())?;
    if affected != 1 {
        return Err("Recovery event is no longer eligible".into());
    }
    Ok(())
}

pub(crate) fn finish_pending(
    connection: &Connection,
    id: &str,
    status: EventStatus,
    inverse_json: Option<&str>,
) -> Result<(), String> {
    let affected = connection
        .execute(
            "UPDATE events SET status = ?1, inverse = ?2 WHERE id = ?3 AND status = 'pending'",
            params![status.as_str(), inverse_json, id],
        )
        .map_err(|error| error.to_string())?;
    if affected != 1 {
        return Err("Event is missing or is no longer pending".into());
    }
    Ok(())
}
