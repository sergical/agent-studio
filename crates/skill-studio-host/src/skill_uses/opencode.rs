//! Reads OpenCode's `session_message` (v2) and `part` (v1) tables into an
//! [`IndexedDatabase`], re-querying only the rows whose `time_updated`
//! advanced past the cached watermark. See `discovery.rs`'s module doc for
//! why the database is opened the way it is.

use std::collections::BTreeSet;
use std::path::Path;

use rusqlite::Connection;
use skill_studio_core::skill_uses::{
    parse_opencode_message, parse_opencode_part, OpenCodeMessageRow, OpenCodePartRow,
};

use crate::opencode_db::{open_opencode_database, table_exists};
use crate::skill_uses::IndexedDatabase;

/// Rows can commit with a `time_updated` a little behind a row that was
/// written moments earlier (out-of-order commit), so the next refresh
/// re-queries slightly before the last watermark rather than exactly at it.
const WATERMARK_SLACK_MS: i64 = 60_000;

/// Re-reads `path` into `entry`: `true` and `entry` updated on success,
/// `false` and `entry` unchanged on any open, query, or row-shape failure
/// (the new state is built on a clone and only swapped in once every table
/// finishes cleanly, so a mid-read failure never leaves `entry` half
/// updated).
pub(crate) fn read_database(path: &Path, entry: &mut IndexedDatabase) -> bool {
    let Some(conn) = open_opencode_database(path) else {
        return false;
    };
    let mut next = entry.clone();
    let read = (|| -> rusqlite::Result<()> {
        let tx = conn.unchecked_transaction()?;
        if table_exists(&tx, "session_message")? {
            read_session_message_table(&tx, &mut next)?;
        }
        if table_exists(&tx, "part")? {
            read_part_table(&tx, &mut next)?;
        }
        Ok(())
    })();
    match read {
        Ok(()) => {
            *entry = next;
            true
        }
        Err(_) => false,
    }
}

/// Drops every cached row keyed to `table` (`"<table>:<id>"`) and returns
/// the floor to re-query from, given the table's current and previously
/// seen `MAX(time_updated)`. A database that was replaced wholesale (its
/// max went backwards) can't be resumed from a watermark, so every cached
/// row from that table is cleared and the floor is reset to read everything.
fn since_and_maybe_reset(entry: &mut IndexedDatabase, table: &str, new_mark: i64) -> i64 {
    let Some(old_mark) = entry.watermarks.get(table).copied() else {
        return i64::MIN;
    };
    if new_mark < old_mark {
        let prefix = format!("{table}:");
        entry.rows.retain(|key, _| !key.starts_with(&prefix));
        return i64::MIN;
    }
    old_mark.saturating_sub(WATERMARK_SLACK_MS)
}

/// Removes every cached key of `table` that wasn't in `returned_ids` this
/// pass and whose row no longer exists (checked by primary-key lookup, only
/// for the keys this pass didn't already confirm are still present).
fn prune_missing_rows(
    conn: &Connection,
    entry: &mut IndexedDatabase,
    table: &str,
    returned_ids: &BTreeSet<String>,
) -> rusqlite::Result<()> {
    let prefix = format!("{table}:");
    let stale: Vec<String> = entry
        .rows
        .keys()
        .filter(|key| key.starts_with(&prefix))
        .filter(|key| !returned_ids.contains(&key[prefix.len()..]))
        .cloned()
        .collect();
    let lookup_sql = format!("SELECT 1 FROM {table} WHERE id = ?1");
    let mut lookup = conn.prepare(&lookup_sql)?;
    for key in stale {
        let id = &key[prefix.len()..];
        let still_present = lookup.exists([id])?;
        if !still_present {
            entry.rows.remove(&key);
        }
    }
    Ok(())
}

/// Reads a `session_message` row's non-id columns (`m.session_id, m.type,
/// m.time_created, m.data, <directory>`, in that order). Pulled out of the
/// row loop so a bad value in any one of them (a NULL `time_created`, a BLOB
/// `data`, ...) fails just this call, and the row can be skipped without
/// failing the whole read.
fn read_message_columns(
    row: &rusqlite::Row,
) -> rusqlite::Result<(String, String, i64, String, Option<String>)> {
    Ok((
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn read_session_message_table(
    conn: &Connection,
    entry: &mut IndexedDatabase,
) -> rusqlite::Result<()> {
    let new_mark: Option<i64> =
        conn.query_row("SELECT MAX(time_updated) FROM session_message", [], |row| {
            row.get(0)
        })?;
    let Some(new_mark) = new_mark else {
        return Ok(());
    };
    let since = since_and_maybe_reset(entry, "session_message", new_mark);

    let has_v2 = table_exists(conn, "session_v2")?;
    let has_v1 = table_exists(conn, "session")?;
    let directory_expr = match (has_v2, has_v1) {
        (true, true) => "COALESCE(v2.directory, s.directory)",
        (true, false) => "v2.directory",
        (false, true) => "s.directory",
        (false, false) => "NULL",
    };
    let mut sql = format!(
        "SELECT m.id, m.session_id, m.type, m.time_created, m.data, {directory_expr} \
         FROM session_message m"
    );
    if has_v2 {
        sql.push_str(" LEFT JOIN session_v2 v2 ON v2.id = m.session_id");
    }
    if has_v1 {
        sql.push_str(" LEFT JOIN session s ON s.id = m.session_id");
    }
    sql.push_str(
        " WHERE m.time_updated >= ?1 AND (m.type = 'skill' OR (m.type = 'assistant' AND \
         (m.data LIKE '%\"name\":\"skill\"%' OR m.data LIKE '%SKILL.md%')))",
    );

    let mut statement = conn.prepare(&sql)?;
    let mut returned_ids: BTreeSet<String> = BTreeSet::new();
    let mut rows = statement.query([since])?;
    while let Some(row) = rows.next()? {
        let Ok(id) = row.get::<_, String>(0) else {
            continue;
        };
        // Recorded before the other columns are even attempted, so a row
        // that fails below is still known to exist and `prune_missing_rows`
        // doesn't waste a lookup on it.
        returned_ids.insert(id.clone());
        let Ok((session_id, kind, time_created, data, directory)) = read_message_columns(row)
        else {
            continue;
        };

        let key = format!("session_message:{id}");
        let uses = parse_opencode_message(&OpenCodeMessageRow {
            session_id: &session_id,
            kind: &kind,
            time_created,
            data: &data,
            directory: directory.as_deref(),
        });
        if uses.is_empty() {
            entry.rows.remove(&key);
        } else {
            entry.rows.insert(key, uses);
        }
    }

    prune_missing_rows(conn, entry, "session_message", &returned_ids)?;
    entry
        .watermarks
        .insert("session_message".to_string(), new_mark);
    Ok(())
}

/// Reads a `part` row's non-id columns (`p.session_id, p.time_created,
/// p.data, <directory>`, in that order). See `read_message_columns` for why
/// this is split out.
fn read_part_columns(
    row: &rusqlite::Row,
) -> rusqlite::Result<(String, i64, String, Option<String>)> {
    Ok((row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
}

fn read_part_table(conn: &Connection, entry: &mut IndexedDatabase) -> rusqlite::Result<()> {
    let new_mark: Option<i64> =
        conn.query_row("SELECT MAX(time_updated) FROM part", [], |row| row.get(0))?;
    let Some(new_mark) = new_mark else {
        return Ok(());
    };
    let since = since_and_maybe_reset(entry, "part", new_mark);

    let has_session = table_exists(conn, "session")?;
    let directory_expr = if has_session { "s.directory" } else { "NULL" };
    let mut sql =
        format!("SELECT p.id, p.session_id, p.time_created, p.data, {directory_expr} FROM part p");
    if has_session {
        sql.push_str(" LEFT JOIN session s ON s.id = p.session_id");
    }
    sql.push_str(
        " WHERE p.time_updated >= ?1 AND (p.data LIKE '%\"tool\":\"skill\"%' OR \
         (p.data LIKE '%\"tool\":\"read\"%' AND p.data LIKE '%SKILL.md%'))",
    );

    let mut statement = conn.prepare(&sql)?;
    let mut returned_ids: BTreeSet<String> = BTreeSet::new();
    let mut rows = statement.query([since])?;
    while let Some(row) = rows.next()? {
        let Ok(id) = row.get::<_, String>(0) else {
            continue;
        };
        // Recorded before the other columns are even attempted, so a row
        // that fails below is still known to exist and `prune_missing_rows`
        // doesn't waste a lookup on it.
        returned_ids.insert(id.clone());
        let Ok((session_id, time_created, data, directory)) = read_part_columns(row) else {
            continue;
        };

        let key = format!("part:{id}");
        let uses = parse_opencode_part(&OpenCodePartRow {
            session_id: &session_id,
            time_created,
            data: &data,
            directory: directory.as_deref(),
        });
        if uses.is_empty() {
            entry.rows.remove(&key);
        } else {
            entry.rows.insert(key, uses);
        }
    }

    prune_missing_rows(conn, entry, "part", &returned_ids)?;
    entry.watermarks.insert("part".to_string(), new_mark);
    Ok(())
}
