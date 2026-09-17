//! Bounded queries over an already authorized event-store connection.
//! This module does not open files, validate filesystem scope, migrate schemas,
//! or authorize inverse execution. Adapters must not use filters as authority.

use crate::skill_event::EventRow;
use rusqlite::{params, types::ValueRef, Connection};
use serde::{Deserialize, Serialize};

pub const MAX_HISTORY_ROWS: u16 = 100;
pub const MAX_HISTORY_RECORD_BYTES: usize = 256 * 1024;
pub const MAX_HISTORY_PAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "project_path",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum HistoryScope {
    All,
    Global,
    Project(String),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    pub limit: u16,
    pub before_rowid: Option<i64>,
    pub skill: Option<String>,
    pub scope: HistoryScope,
}
impl HistoryQuery {
    pub fn validate(&self) -> Result<(), HistoryError> {
        let valid_text = |value: &str| {
            !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
        };
        if self.event_id.as_deref().is_some_and(|id| !valid_text(id))
            || (self.event_id.is_some() && (self.limit != 1 || self.before_rowid.is_some()))
            || self.limit == 0
            || self.limit > MAX_HISTORY_ROWS
            || self.before_rowid.is_some_and(|id| id <= 0)
            || self
                .skill
                .as_deref()
                .is_some_and(|value| !valid_text(value))
            || matches!(&self.scope, HistoryScope::Project(path) if !valid_text(path) || !std::path::Path::new(path).is_absolute())
        {
            return Err(HistoryError::InvalidQuery);
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryPage {
    pub events: Vec<EventRow>,
    pub next_before_rowid: Option<i64>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum HistoryError {
    InvalidQuery,
    InvalidStore,
    Busy,
    InvalidRecord,
    RecordTooLarge,
    InvalidControl,
    Cancelled,
    DeadlineExceeded,
}
impl HistoryError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidQuery => "invalid_history_query",
            Self::InvalidStore => "history_store_unreadable",
            Self::Busy => "history_store_busy",
            Self::InvalidRecord => "invalid_history_record",
            Self::RecordTooLarge => "history_record_too_large",
            Self::InvalidControl => "invalid_history_control",
            Self::Cancelled => "history_cancelled",
            Self::DeadlineExceeded => "history_deadline_exceeded",
        }
    }
}
impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for HistoryError {}

fn store_error(error: rusqlite::Error) -> HistoryError {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            HistoryError::Busy
        }
        _ => HistoryError::InvalidStore,
    }
}

fn validate_history_schema(connection: &Connection) -> Result<(), HistoryError> {
    let ordinary: bool = connection.query_row(
        "SELECT type = 'table' AND wr = 0 FROM pragma_table_list WHERE schema = 'main' AND name = 'events'",
        [], |row| row.get(0),
    ).map_err(store_error)?;
    if !ordinary {
        return Err(HistoryError::InvalidStore);
    }
    let mut columns = connection
        .prepare("SELECT name, type, hidden FROM pragma_table_xinfo('events', 'main')")
        .map_err(store_error)?;
    let mut rows = columns.query([]).map_err(store_error)?;
    let required = [
        "id",
        "ts",
        "kind",
        "skill",
        "harness",
        "scope",
        "project_path",
        "payload",
        "inverse",
        "backup_dir",
        "status",
        "reverted_by",
        "restorable",
    ];
    let mut found = [false; 13];
    while let Some(row) = rows.next().map_err(store_error)? {
        let name: String = row.get(0).map_err(store_error)?;
        if ["rowid", "_rowid_", "oid"]
            .iter()
            .any(|alias| name.eq_ignore_ascii_case(alias))
        {
            return Err(HistoryError::InvalidStore);
        }
        if let Some(index) = required
            .iter()
            .position(|field| name.eq_ignore_ascii_case(field))
        {
            let kind: String = row.get(1).map_err(store_error)?;
            let hidden: i64 = row.get(2).map_err(store_error)?;
            let expected = if required[index] == "restorable" {
                "INTEGER"
            } else {
                "TEXT"
            };
            if hidden != 0 || !kind.eq_ignore_ascii_case(expected) {
                return Err(HistoryError::InvalidStore);
            }
            found[index] = true;
        }
    }
    if found.iter().all(|present| *present) {
        Ok(())
    } else {
        Err(HistoryError::InvalidStore)
    }
}

pub fn read_history_page(
    connection: &Connection,
    query: &HistoryQuery,
) -> Result<HistoryPage, HistoryError> {
    query.validate()?;
    // Schema inspection and rows must share one SQLite read snapshot.
    let _transaction = if connection.is_autocommit() {
        Some(connection.unchecked_transaction().map_err(store_error)?)
    } else {
        None
    };
    validate_history_schema(connection)?;
    let (scope, project) = match &query.scope {
        HistoryScope::All => (None, None),
        HistoryScope::Global => (Some("global"), None),
        HistoryScope::Project(project) => (Some("project"), Some(project.as_str())),
    };
    let identity_filter = if query.event_id.is_some() {
        "id = ?6 AND"
    } else {
        "?6 IS NULL AND"
    };
    let mut statement = connection.prepare(&format!(
        "SELECT rowid, id, ts, kind, skill, harness, scope, project_path, payload, inverse, backup_dir, status, reverted_by, restorable FROM main.events
         WHERE {identity_filter} (?1 IS NULL OR rowid < ?1) AND (?2 IS NULL OR skill = ?2)
         AND (?3 IS NULL OR scope = ?3) AND (?4 IS NULL OR project_path = ?4)
         ORDER BY rowid DESC LIMIT ?5"
    )).map_err(store_error)?;
    let mut rows = statement
        .query(params![
            query.before_rowid,
            query.skill,
            scope,
            project,
            i64::from(query.limit) + 1,
            query.event_id
        ])
        .map_err(store_error)?;
    let mut events = Vec::new();
    let mut last_rowid = None;
    let mut page_bytes = 0;
    while let Some(row) = rows.next().map_err(store_error)? {
        if events.len() == usize::from(query.limit) {
            if query.event_id.is_some() {
                return Err(HistoryError::InvalidRecord);
            }
            return Ok(HistoryPage {
                events,
                next_before_rowid: last_rowid,
            });
        }
        let mut record_bytes = 0usize;
        for column in 1..14 {
            record_bytes = record_bytes.saturating_add(
                match row
                    .get_ref(column)
                    .map_err(|_| HistoryError::InvalidRecord)?
                {
                    ValueRef::Text(bytes) => bytes.len(),
                    ValueRef::Null | ValueRef::Integer(_) => 0,
                    _ => return Err(HistoryError::InvalidRecord),
                },
            );
        }
        if record_bytes > MAX_HISTORY_RECORD_BYTES {
            return Err(HistoryError::RecordTooLarge);
        }
        if page_bytes + record_bytes > MAX_HISTORY_PAGE_BYTES {
            return Ok(HistoryPage {
                events,
                next_before_rowid: last_rowid,
            });
        }
        let text = |index| {
            row.get_ref(index)
                .map_err(|_| HistoryError::InvalidRecord)?
                .as_str()
                .map_err(|_| HistoryError::InvalidRecord)
        };
        let optional = |index| -> Result<Option<String>, HistoryError> {
            match row
                .get_ref(index)
                .map_err(|_| HistoryError::InvalidRecord)?
            {
                ValueRef::Null => Ok(None),
                ValueRef::Text(bytes) => std::str::from_utf8(bytes)
                    .map(|value| Some(value.to_owned()))
                    .map_err(|_| HistoryError::InvalidRecord),
                _ => Err(HistoryError::InvalidRecord),
            }
        };
        let payload = serde_json::from_str(text(8)?).map_err(|_| HistoryError::InvalidRecord)?;
        let inverse = optional(9)?
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|_| HistoryError::InvalidRecord)?;
        let restorable: i64 = row.get(13).map_err(|_| HistoryError::InvalidRecord)?;
        if !matches!(restorable, 0 | 1) {
            return Err(HistoryError::InvalidRecord);
        }
        let rowid: i64 = row.get(0).map_err(|_| HistoryError::InvalidRecord)?;
        if rowid <= 0 {
            return Err(HistoryError::InvalidRecord);
        }
        events.push(EventRow {
            id: text(1)?.into(),
            ts: text(2)?.into(),
            kind: text(3)?.into(),
            skill: text(4)?.into(),
            harness: optional(5)?,
            scope: optional(6)?,
            project_path: optional(7)?,
            payload,
            inverse,
            backup_dir: optional(10)?,
            status: text(11)?.into(),
            reverted_by: optional(12)?,
            restorable: restorable == 1,
        });
        last_rowid = Some(rowid);
        page_bytes += record_bytes;
    }
    Ok(HistoryPage {
        events,
        next_before_rowid: None,
    })
}

#[derive(Debug)]
pub struct HistorySummary {
    pub id: String,
    pub ts: String,
    pub kind: String,
    pub skill: String,
    pub harness: Option<String>,
    pub scope: Option<String>,
    pub project_path: Option<String>,
    pub status: String,
    pub reverted_by: Option<String>,
    pub backup_dir: Option<String>,
    pub restorable: bool,
    pub has_inverse: bool,
    pub copy_visibility_disabled: Option<bool>,
}

pub fn read_history_summaries(
    connection: &Connection,
    limit: usize,
    skill: Option<&str>,
) -> Result<Vec<HistorySummary>, HistoryError> {
    if limit > 1000
        || skill.is_some_and(|value| value.len() > 4096 || value.chars().any(char::is_control))
    {
        return Err(HistoryError::InvalidQuery);
    }
    let _transaction = if connection.is_autocommit() {
        Some(connection.unchecked_transaction().map_err(store_error)?)
    } else {
        None
    };
    validate_history_schema(connection)?;
    let mut statement = connection.prepare(
        "SELECT id,ts,kind,skill,harness,scope,project_path,status,reverted_by,backup_dir,restorable,inverse IS NOT NULL,
         CASE WHEN kind = 'move_copy_deployment' AND length(CAST(payload AS BLOB)) <= ?3 THEN
              CASE WHEN json_valid(payload) THEN
                   CASE WHEN json_type(payload, '$.transition.after.disabled') IN ('true', 'false')
                        THEN json_extract(payload, '$.transition.after.disabled') ELSE NULL END
              ELSE NULL END
         ELSE NULL END
         FROM main.events WHERE (?1 IS NULL OR skill = ?1) ORDER BY rowid DESC LIMIT ?2"
    ).map_err(store_error)?;
    let rows = statement
        .query_map(
            params![skill, limit as i64, MAX_HISTORY_RECORD_BYTES as i64],
            |row| {
                Ok(HistorySummary {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    kind: row.get(2)?,
                    skill: row.get(3)?,
                    harness: row.get(4)?,
                    scope: row.get(5)?,
                    project_path: row.get(6)?,
                    status: row.get(7)?,
                    reverted_by: row.get(8)?,
                    backup_dir: row.get(9)?,
                    restorable: row.get(10)?,
                    has_inverse: row.get(11)?,
                    copy_visibility_disabled: row.get(12)?,
                })
            },
        )
        .map_err(store_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(store_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_rejects_unbounded_or_invalid_detail_requests() {
        assert_eq!(
            HistoryQuery {
                event_id: None,
                limit: MAX_HISTORY_ROWS + 1,
                before_rowid: None,
                skill: None,
                scope: HistoryScope::All,
            }
            .validate(),
            Err(HistoryError::InvalidQuery)
        );
        assert_eq!(
            HistoryQuery {
                event_id: Some("event".into()),
                limit: 2,
                before_rowid: None,
                skill: None,
                scope: HistoryScope::All,
            }
            .validate(),
            Err(HistoryError::InvalidQuery)
        );
    }

    #[test]
    fn summaries_refuse_an_unbounded_limit_before_querying_the_store() {
        let connection = Connection::open_in_memory().unwrap();
        assert_eq!(
            read_history_summaries(&connection, 1001, None).unwrap_err(),
            HistoryError::InvalidQuery
        );
    }
    #[test]
    fn summaries_preserve_order_scope_and_visibility_without_loading_large_payloads() {
        let connection = Connection::open_in_memory().unwrap();
        crate::skill_event_schema::initialize(&connection).unwrap();
        for (id, kind, scope, payload) in [
            (
                "older",
                "add",
                "global",
                serde_json::json!({"body": "x".repeat(MAX_HISTORY_RECORD_BYTES)}),
            ),
            (
                "newer",
                "move_copy_deployment",
                "project",
                serde_json::json!({"transition": {"after": {"disabled": true}}}),
            ),
        ] {
            connection.execute("INSERT INTO events(id,ts,kind,skill,scope,payload,status) VALUES(?1,'now',?2,'sample',?3,?4,'done')", params![id,kind,scope,payload.to_string()]).unwrap();
        }
        let summaries = read_history_summaries(&connection, 2, Some("sample")).unwrap();
        assert_eq!(
            summaries
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["newer", "older"]
        );
        assert_eq!(summaries[0].scope.as_deref(), Some("project"));
        assert_eq!(summaries[0].copy_visibility_disabled, Some(true));
        assert!(read_history_summaries(&connection, 2, Some("other"))
            .unwrap()
            .is_empty());
        let query = HistoryQuery {
            event_id: Some("older".into()),
            limit: 1,
            before_rowid: None,
            skill: None,
            scope: HistoryScope::All,
        };
        assert!(matches!(
            read_history_page(&connection, &query),
            Err(HistoryError::RecordTooLarge)
        ));
    }

    #[test]
    fn detail_queries_refuse_a_view_in_place_of_the_event_table() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE VIEW events AS SELECT 1 AS id")
            .unwrap();
        assert!(matches!(
            read_history_summaries(&connection, 20, None),
            Err(HistoryError::InvalidStore)
        ));
    }
    #[test]
    fn summaries_do_not_load_oversized_explode_or_copy_payloads() {
        let connection = Connection::open_in_memory().unwrap();
        crate::skill_event_schema::initialize(&connection).unwrap();
        let payload = serde_json::json!({"root": "/fixture", "body": "x".repeat(MAX_HISTORY_RECORD_BYTES), "transition": {"after": {"disabled": true}}}).to_string();
        for kind in ["explode_shared_dir", "move_copy_deployment"] {
            connection.execute("INSERT INTO events(id,ts,kind,skill,payload,status) VALUES(?1,'now',?1,'sample',?2,'done')", params![kind,payload]).unwrap();
        }
        let rows = read_history_summaries(&connection, 2, None).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, "move_copy_deployment");
        assert!(rows[0].copy_visibility_disabled.is_none());
        assert_eq!(rows[1].kind, "explode_shared_dir");
    }
}
