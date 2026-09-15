//! Shared schema initialization on an already-authorized SQLite connection.
use rusqlite::Connection;

pub(crate) fn initialize(conn: &Connection) -> Result<(), String> {
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| format!("Failed to set WAL mode: {e}"))?;
    // `reverted_by` is claimed (set to the restore event's id) before that
    // restore row exists - see `restore()` - so foreign key enforcement on
    // that column must stay off.
    conn.pragma_update(None, "foreign_keys", "OFF")
        .map_err(|e| format!("Failed to disable foreign keys: {e}"))?;
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
    let conn = &transaction;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS events (
            id          TEXT PRIMARY KEY,
            ts          TEXT NOT NULL,
            kind        TEXT NOT NULL,
            skill       TEXT NOT NULL,
            harness     TEXT,
            scope       TEXT,
            project_path TEXT,
            payload     TEXT NOT NULL,
            inverse     TEXT,
            backup_dir  TEXT,
            status      TEXT NOT NULL,
            reverted_by TEXT REFERENCES events(id),
            restorable  INTEGER NOT NULL DEFAULT 1
        );
        CREATE INDEX IF NOT EXISTS idx_events_skill ON events(skill, ts DESC);

        CREATE TABLE IF NOT EXISTS event_command_receipts (
            command_id TEXT PRIMARY KEY NOT NULL,
            event_id   TEXT NOT NULL,
            digest     BLOB NOT NULL CHECK(typeof(digest) = 'blob' AND length(digest) = 32)
        );

        CREATE TABLE IF NOT EXISTS materialized_roots (
            root_path   TEXT PRIMARY KEY,
            harness     TEXT NOT NULL,
            shared_root TEXT NOT NULL,
            created_by  TEXT REFERENCES events(id)
        );
        CREATE TABLE IF NOT EXISTS materialized_disabled (
            root_path   TEXT NOT NULL REFERENCES materialized_roots(root_path),
            skill       TEXT NOT NULL,
            PRIMARY KEY (root_path, skill)
        );",
    )
    .map_err(|e| format!("Failed to create event store schema: {e}"))?;
    let has_restorable = conn
        .prepare("SELECT restorable FROM events LIMIT 0")
        .is_ok();
    if !has_restorable {
        conn.execute(
            "ALTER TABLE events ADD COLUMN restorable INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| format!("Failed to add events.restorable: {e}"))?;
    }
    let has_backup_dir = {
        let mut statement = conn
            .prepare("PRAGMA table_info(events)")
            .map_err(|e| format!("Failed to inspect event store schema: {e}"))?;
        let has_backup_dir = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("Failed to query event store columns: {e}"))?
            .try_fold(false, |found, column| {
                column.map(|column| found || column == "backup_dir")
            })
            .map_err(|e| format!("Failed to read event store columns: {e}"))?;
        has_backup_dir
    };
    if !has_backup_dir {
        conn.execute("ALTER TABLE events ADD COLUMN backup_dir TEXT", [])
            .map_err(|e| format!("Failed to add events.backup_dir: {e}"))?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(())
}
