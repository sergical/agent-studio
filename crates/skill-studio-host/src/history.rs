//! [`HistoryOpener`]/[`HistoryStore`] adapters: a no-op placeholder and a
//! real SQLite-backed store byte-compatible with the desktop's
//! `events.sqlite3` and its `backups/<event-id>/manifest.json` layout
//! (`apps/desktop/src-tauri/src/skills/event_store.rs`), so a user
//! upgrading from the shipped app keeps every existing event restorable.

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use skill_studio_core::error::{CoreError, ErrorCode};
use skill_studio_core::events::{
    BackupEntry, BackupManifest, EventDraft, EventFilter, EventRecord, EventStatus,
};
use skill_studio_core::identity::{sha256_hex, AgentId, EventId, Fingerprint, SkillName};
use skill_studio_core::ports::{ExclusiveGuard, HistoryAccess, HistoryOpener, HistoryStore};
use skill_studio_core::scope::NormalizedScope;

/// A `HistoryOpener` with no store behind it.
///
/// Read-only operations such as a scan never open history, so this is
/// enough for them today. A mutation that asks for
/// [`HistoryAccess::ReadWrite`] fails with [`ErrorCode::Unsupported`];
/// [`SqliteHistoryOpener`] is the real event log.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoHistoryOpener;

impl HistoryOpener for NoHistoryOpener {
    fn open(
        &self,
        _scope: &NormalizedScope,
        access: HistoryAccess,
    ) -> Result<Option<Box<dyn HistoryStore>>, CoreError> {
        match access {
            HistoryAccess::ReadIfExists => Ok(None),
            HistoryAccess::ReadWrite => Err(CoreError::new(
                ErrorCode::Unsupported,
                "this host build has no history store; mutations are not available yet",
            )),
        }
    }
}

/// Schema shared with the desktop's `event_store::open`. Kept in sync by
/// hand; the round-trip test in this module opens a database seeded with
/// the desktop's own `CREATE TABLE` statements to catch drift.
const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS events (
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
);
";

/// Opens the SQLite event log at a fixed path.
///
/// [`HistoryAccess::ReadIfExists`] never creates `db_path`; it returns
/// `None` when the file is absent. [`HistoryAccess::ReadWrite`] creates the
/// parent directory and the schema on first use.
#[derive(Debug, Clone)]
pub struct SqliteHistoryOpener {
    db_path: PathBuf,
}

impl SqliteHistoryOpener {
    /// Builds an opener bound to `db_path`.
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        SqliteHistoryOpener {
            db_path: db_path.into(),
        }
    }
}

impl HistoryOpener for SqliteHistoryOpener {
    fn open(
        &self,
        _scope: &NormalizedScope,
        access: HistoryAccess,
    ) -> Result<Option<Box<dyn HistoryStore>>, CoreError> {
        match access {
            HistoryAccess::ReadIfExists => {
                if !self.db_path.exists() {
                    return Ok(None);
                }
                Ok(Some(Box::new(SqliteHistoryStore::open(&self.db_path)?)))
            }
            HistoryAccess::ReadWrite => {
                Ok(Some(Box::new(SqliteHistoryStore::open(&self.db_path)?)))
            }
        }
    }
}

/// The real event log: SQLite via `rusqlite`, plus byte backups on disk
/// under `<db_path's directory>/backups/<event-id>/`.
pub struct SqliteHistoryStore {
    conn: Connection,
    /// `db_path`'s parent directory; `backup_dir` values (e.g.
    /// `backups/<event-id>`) are relative to this.
    root: PathBuf,
    backups_root: PathBuf,
}

impl SqliteHistoryStore {
    fn open(db_path: &Path) -> Result<Self, CoreError> {
        let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
        let conn = Connection::open(db_path).map_err(sql_err)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(sql_err)?;
        // `reverted_by` is claimed before the restore row that references it
        // exists (see `claim_revert`), so foreign key enforcement on that
        // column must stay off - matches the desktop's `event_store::open`.
        conn.pragma_update(None, "foreign_keys", "OFF")
            .map_err(sql_err)?;
        conn.execute_batch(SCHEMA_SQL).map_err(sql_err)?;
        Ok(SqliteHistoryStore {
            conn,
            root: parent.to_path_buf(),
            backups_root: parent.join("backups"),
        })
    }

    fn backup_dir_for(&self, id: &EventId) -> PathBuf {
        self.backups_root.join(&id.0)
    }
}

impl HistoryStore for SqliteHistoryStore {
    fn list(&self, filter: &EventFilter) -> Result<Vec<EventRecord>, CoreError> {
        let mut clauses = Vec::new();
        if filter.skill.is_some() {
            clauses.push("skill = ?");
        }
        if filter.after.is_some() {
            clauses.push("rowid < (SELECT rowid FROM events WHERE id = ?)");
        }
        let mut sql = String::from("SELECT * FROM events");
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY rowid DESC LIMIT ?");

        let mut owned_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(skill) = &filter.skill {
            owned_params.push(Box::new(skill.0.clone()));
        }
        if let Some(after) = &filter.after {
            owned_params.push(Box::new(after.0.clone()));
        }
        owned_params.push(Box::new(filter.limit as i64));
        let bound: Vec<&dyn rusqlite::ToSql> = owned_params.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql).map_err(sql_err)?;
        let rows = stmt
            .query_map(bound.as_slice(), row_from)
            .map_err(sql_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
    }

    fn get(&self, id: &EventId) -> Result<Option<EventRecord>, CoreError> {
        self.conn
            .query_row(
                "SELECT * FROM events WHERE id = ?1",
                params![id.0],
                row_from,
            )
            .optional()
            .map_err(sql_err)
    }

    fn backup_paths(
        &mut self,
        _guard: &ExclusiveGuard,
        id: &EventId,
        paths: &[PathBuf],
    ) -> Result<BackupManifest, CoreError> {
        let dir = self.backup_dir_for(id);
        fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;

        // Every path gets a core `BackupEntry`, present or not: `fingerprint`
        // is `None` exactly when the original was absent at backup time,
        // which a later drift check reads as "this must still be absent"
        // rather than as a hash mismatch. The on-disk `manifest.json` mirrors
        // this with the desktop's literal `"absent"` string - see
        // `DesktopBackupManifest` - so a restore reads the real
        // desktop-compatible record either way.
        let mut entries = Vec::new();
        let mut on_disk = DesktopBackupManifest::default();
        for (i, path) in paths.iter().enumerate() {
            let key = path.to_string_lossy().into_owned();
            if fs::symlink_metadata(path).is_err() {
                on_disk.entries.insert(
                    key,
                    DesktopBackupEntry {
                        relative_path: String::new(),
                        fingerprint: "absent".to_string(),
                    },
                );
                entries.push(BackupEntry {
                    original: path.clone(),
                    relative: String::new(),
                    fingerprint: None,
                });
                continue;
            }
            let basename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("path-{i}"));
            let relative_path = format!("{i}-{basename}");
            copy_recursive(path, &dir.join(&relative_path)).map_err(|e| CoreError::io(path, e))?;
            let hex = hash_entry(path).map_err(|e| CoreError::io(path, e))?;
            on_disk.entries.insert(
                key,
                DesktopBackupEntry {
                    relative_path: relative_path.clone(),
                    fingerprint: hex.clone(),
                },
            );
            entries.push(BackupEntry {
                original: path.clone(),
                relative: relative_path,
                fingerprint: Some(
                    Fingerprint::parse(&hex).expect("hash_entry returns 64 lowercase hex chars"),
                ),
            });
        }

        let json = serde_json::to_vec_pretty(&on_disk).map_err(json_err)?;
        let manifest_path = dir.join("manifest.json");
        let mut file =
            File::create(&manifest_path).map_err(|e| CoreError::io(&manifest_path, e))?;
        file.write_all(&json)
            .map_err(|e| CoreError::io(&manifest_path, e))?;
        file.sync_all()
            .map_err(|e| CoreError::io(&manifest_path, e))?;

        Ok(BackupManifest {
            event_id: id.clone(),
            backup_dir: format!("backups/{}", id.0),
            entries,
        })
    }

    fn record(
        &mut self,
        _guard: &ExclusiveGuard,
        id: &EventId,
        draft: &EventDraft,
    ) -> Result<(), CoreError> {
        let ts = Utc::now().to_rfc3339();
        let payload_json = serde_json::to_string(&draft.payload).map_err(json_err)?;
        let inverse_json = draft
            .inverse
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(json_err)?;
        let project_path = draft
            .project_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        self.conn
            .execute(
                "INSERT INTO events
                    (id, ts, kind, skill, harness, scope, project_path, payload, inverse, backup_dir, status, reverted_by, restorable)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', NULL, 1)",
                params![
                    id.0,
                    ts,
                    draft.kind.as_str(),
                    draft.skill.0,
                    draft.harness.as_ref().map(|a| a.as_str()),
                    draft.scope,
                    project_path,
                    payload_json,
                    inverse_json,
                    draft.backup_dir,
                ],
            )
            .map_err(sql_err)?;
        Ok(())
    }

    fn finish(
        &mut self,
        _guard: &ExclusiveGuard,
        id: &EventId,
        status: EventStatus,
        post_fingerprint: Option<Fingerprint>,
    ) -> Result<(), CoreError> {
        self.conn
            .execute(
                "UPDATE events SET status = ?1 WHERE id = ?2",
                params![status.as_str(), id.0],
            )
            .map_err(sql_err)?;

        let Some(fingerprint) = post_fingerprint else {
            return Ok(());
        };
        let inverse_str: Option<String> = self
            .conn
            .query_row(
                "SELECT inverse FROM events WHERE id = ?1",
                params![id.0],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_err)?
            .flatten();
        let Some(inverse_str) = inverse_str else {
            return Ok(());
        };
        let mut value: serde_json::Value =
            serde_json::from_str(&inverse_str).unwrap_or(serde_json::Value::Null);
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "post_fingerprint".to_string(),
                serde_json::Value::String(fingerprint.bare_hex().to_string()),
            );
            let updated = serde_json::to_string(&value).map_err(json_err)?;
            self.conn
                .execute(
                    "UPDATE events SET inverse = ?1 WHERE id = ?2",
                    params![updated, id.0],
                )
                .map_err(sql_err)?;
        }
        Ok(())
    }

    fn claim_revert(
        &mut self,
        _guard: &ExclusiveGuard,
        target: &EventId,
        by: &EventId,
    ) -> Result<bool, CoreError> {
        let changed = self
            .conn
            .execute(
                "UPDATE events SET reverted_by = ?1 WHERE id = ?2 AND reverted_by IS NULL",
                params![by.0, target.0],
            )
            .map_err(sql_err)?;
        Ok(changed > 0)
    }

    fn release_revert(
        &mut self,
        _guard: &ExclusiveGuard,
        target: &EventId,
        restore: &EventId,
    ) -> Result<bool, CoreError> {
        let changed = self
            .conn
            .execute(
                "UPDATE events SET reverted_by = NULL WHERE id = ?1 AND reverted_by = ?2",
                params![target.0, restore.0],
            )
            .map_err(sql_err)?;
        Ok(changed > 0)
    }

    fn pending(&self) -> Result<Vec<EventRecord>, CoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM events WHERE status = 'pending' ORDER BY rowid ASC")
            .map_err(sql_err)?;
        let rows = stmt.query_map([], row_from).map_err(sql_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
    }

    fn read_manifest(&self, backup_dir: &str) -> Result<BackupManifest, CoreError> {
        let dir = self.root.join(backup_dir);
        let on_disk = read_manifest(&dir)?;
        let entries = on_disk
            .entries
            .into_iter()
            .map(|(original, entry)| {
                let fingerprint =
                    if entry.fingerprint == "absent" {
                        None
                    } else {
                        Some(Fingerprint::parse(&entry.fingerprint).unwrap_or_else(|_| {
                            Fingerprint::of_bytes(entry.fingerprint.as_bytes())
                        }))
                    };
                BackupEntry {
                    original: PathBuf::from(original),
                    relative: entry.relative_path,
                    fingerprint,
                }
            })
            .collect();
        Ok(BackupManifest {
            // The event id is not recoverable from the manifest alone; the
            // caller already has it (it is the id that named `backup_dir`).
            event_id: EventId(String::new()),
            backup_dir: backup_dir.to_string(),
            entries,
        })
    }

    fn read_backup_bytes(&self, backup_dir: &str, relative: &str) -> Result<Vec<u8>, CoreError> {
        let path = self.root.join(backup_dir).join(relative);
        fs::read(&path).map_err(|e| CoreError::io(&path, e))
    }
}

/// On-disk `manifest.json` shape, byte-for-byte the desktop's
/// `event_store::BackupManifest`/`BackupEntry`
/// (`apps/desktop/src-tauri/src/skills/event_store.rs`): keyed by the
/// original absolute path as a lossy string, so a backup directory written
/// by either implementation reads back in the other. `fingerprint` is a
/// plain string here (not the core's typed `Fingerprint`) because the
/// desktop writes the literal `"absent"` for a path that did not exist,
/// which the typed form cannot hold.
#[derive(Debug, Default, Serialize, Deserialize)]
struct DesktopBackupManifest {
    entries: std::collections::BTreeMap<String, DesktopBackupEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DesktopBackupEntry {
    /// Path relative to the backup dir; empty when the original was absent.
    relative_path: String,
    fingerprint: String,
}

/// Reads and parses `<dir>/manifest.json`, in the desktop-compatible shape.
fn read_manifest(dir: &Path) -> Result<DesktopBackupManifest, CoreError> {
    let manifest_path = dir.join("manifest.json");
    let bytes = fs::read(&manifest_path).map_err(|e| CoreError::io(&manifest_path, e))?;
    serde_json::from_slice(&bytes).map_err(json_err)
}

fn row_from(row: &rusqlite::Row) -> rusqlite::Result<EventRecord> {
    let id: String = row.get("id")?;
    let ts: String = row.get("ts")?;
    let kind: String = row.get("kind")?;
    let skill: String = row.get("skill")?;
    let harness: Option<String> = row.get("harness")?;
    let scope: Option<String> = row.get("scope")?;
    let project_path: Option<String> = row.get("project_path")?;
    let payload_str: String = row.get("payload")?;
    let inverse_str: Option<String> = row.get("inverse")?;
    let backup_dir: Option<String> = row.get("backup_dir")?;
    let status: String = row.get("status")?;
    let reverted_by: Option<String> = row.get("reverted_by")?;
    let restorable: bool = row.get("restorable")?;

    let ts = DateTime::parse_from_rfc3339(&ts)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let status = EventStatus::parse(&status).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "unknown event status `{status}`"
            )),
        )
    })?;
    let harness = harness
        .map(|h| AgentId::parse(&h))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::<dyn std::error::Error + Send + Sync>::from(e.to_string()),
            )
        })?;
    let payload = serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null);
    let inverse = inverse_str.map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null));

    Ok(EventRecord {
        id: EventId(id),
        ts,
        kind,
        skill: SkillName(skill),
        harness,
        scope,
        project_path: project_path.map(PathBuf::from),
        payload,
        inverse,
        backup_dir,
        status,
        reverted_by: reverted_by.map(EventId),
        restorable,
    })
}

fn sql_err(e: rusqlite::Error) -> CoreError {
    CoreError::new(ErrorCode::Io, format!("sqlite error: {e}"))
}

fn json_err(e: serde_json::Error) -> CoreError {
    CoreError::new(ErrorCode::Io, format!("json error: {e}"))
}

/// Content hash for an existing path: matches the desktop's
/// `fingerprint_path`/`hash_entry` byte for byte (same tag bytes, same
/// length framing), so the two implementations compute identical hex for
/// identical filesystem content. Callers only reach this for a path that
/// exists; an absent path is handled separately (see `backup_paths`).
pub fn hash_entry(path: &Path) -> std::io::Result<String> {
    let meta = fs::symlink_metadata(path)?;
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(path)?;
        let mut buf = vec![b'L'];
        buf.extend_from_slice(target.to_string_lossy().as_bytes());
        Ok(sha256_hex(&buf))
    } else if file_type.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(path)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        let mut buf = vec![b'D'];
        for entry in entries {
            let name = entry
                .file_name()
                .to_string_lossy()
                .into_owned()
                .into_bytes();
            let child = hash_entry(&entry.path())?;
            buf.extend_from_slice(&(name.len() as u64).to_le_bytes());
            buf.extend_from_slice(&name);
            buf.extend_from_slice(&(child.len() as u64).to_le_bytes());
            buf.extend_from_slice(child.as_bytes());
        }
        Ok(sha256_hex(&buf))
    } else {
        let bytes = fs::read(path)?;
        let mut buf = Vec::with_capacity(bytes.len() + 9);
        buf.push(b'F');
        buf.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        buf.extend_from_slice(&bytes);
        Ok(sha256_hex(&buf))
    }
}

/// Copies `src` into `dest`, preserving regular files as bytes, directories
/// recursively, and symlinks as the literal link (never following it).
fn copy_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(src)?;
        create_symlink(&target, dest)?;
    } else if file_type.is_dir() {
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
    } else {
        fs::copy(src, dest)?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "symlinking is only supported on Unix",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::events::EventKind;
    use skill_studio_core::ports::{acquire_exclusive, HistoryOpener};
    use skill_studio_core::scope::RuntimeScope;

    fn scope_for(home: &Path) -> NormalizedScope {
        let raw = RuntimeScope::fixture(home);
        NormalizedScope::normalize(&raw, &crate::fs::RealFs::new()).unwrap()
    }

    fn guard_for(dir: &Path, scope: &NormalizedScope) -> ExclusiveGuard {
        let leases = crate::lease::FileLease::new(dir.join("leases"));
        acquire_exclusive(&leases, scope).unwrap()
    }

    fn draft(
        kind: EventKind,
        skill: &str,
        payload: serde_json::Value,
        inverse: Option<serde_json::Value>,
    ) -> EventDraft {
        EventDraft {
            kind,
            skill: SkillName(skill.to_string()),
            harness: Some(AgentId::parse("claude-code").unwrap()),
            scope: Some("global".to_string()),
            project_path: None,
            payload,
            inverse,
            backup_dir: None,
        }
    }

    #[test]
    fn read_if_exists_returns_none_for_a_missing_file_and_never_creates_it() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope_for(tmp.path());
        let db_path = tmp.path().join("history").join("events.sqlite3");
        let opener = SqliteHistoryOpener::new(&db_path);

        let store = opener.open(&scope, HistoryAccess::ReadIfExists).unwrap();
        assert!(store.is_none());
        assert!(!db_path.exists());
    }

    #[test]
    fn read_write_creates_the_directory_and_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope_for(tmp.path());
        let db_path = tmp.path().join("history").join("events.sqlite3");
        let opener = SqliteHistoryOpener::new(&db_path);

        let store = opener.open(&scope, HistoryAccess::ReadWrite).unwrap();
        assert!(store.is_some());
        assert!(db_path.exists());

        // Reopening with ReadIfExists now finds the file the write created.
        let store = opener.open(&scope, HistoryAccess::ReadIfExists).unwrap();
        assert!(store.is_some());
    }

    #[test]
    fn record_then_list_round_trips_payload_and_inverse() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope_for(tmp.path());
        let guard = guard_for(tmp.path(), &scope);
        let db_path = tmp.path().join("history").join("events.sqlite3");
        let mut store = SqliteHistoryStore::open(&db_path).unwrap();

        let id = EventId::from_ulid(ulid::Ulid::new());
        let inverse =
            serde_json::json!({"op": "recreate_symlink", "pre_fingerprint": "sha256:abc"});
        store
            .record(
                &guard,
                &id,
                &draft(
                    EventKind::Install,
                    "alpha",
                    serde_json::json!({"n": 1}),
                    Some(inverse.clone()),
                ),
            )
            .unwrap();
        store.finish(&guard, &id, EventStatus::Done, None).unwrap();

        let rows = store
            .list(&EventFilter {
                skill: None,
                limit: 10,
                after: None,
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].kind, "install");
        assert_eq!(rows[0].skill, SkillName("alpha".to_string()));
        assert_eq!(rows[0].payload, serde_json::json!({"n": 1}));
        assert_eq!(rows[0].inverse, Some(inverse));
        assert_eq!(rows[0].status, EventStatus::Done);

        let fetched = store.get(&id).unwrap().unwrap();
        assert_eq!(fetched.id, id);
    }

    #[test]
    fn after_pages_backwards_through_older_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope_for(tmp.path());
        let guard = guard_for(tmp.path(), &scope);
        let db_path = tmp.path().join("history").join("events.sqlite3");
        let mut store = SqliteHistoryStore::open(&db_path).unwrap();

        let mut ids = Vec::new();
        for i in 0..3 {
            let id = EventId::from_ulid(ulid::Ulid::new());
            store
                .record(
                    &guard,
                    &id,
                    &draft(
                        EventKind::Install,
                        "alpha",
                        serde_json::json!({"n": i}),
                        None,
                    ),
                )
                .unwrap();
            store.finish(&guard, &id, EventStatus::Done, None).unwrap();
            ids.push(id);
        }
        // Newest first: ids[2], ids[1], ids[0].
        let first_page = store
            .list(&EventFilter {
                skill: None,
                limit: 1,
                after: None,
            })
            .unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].id, ids[2]);

        let second_page = store
            .list(&EventFilter {
                skill: None,
                limit: 10,
                after: Some(ids[2].clone()),
            })
            .unwrap();
        assert_eq!(
            second_page.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            vec![ids[1].clone(), ids[0].clone()]
        );
    }

    #[test]
    fn claim_revert_twice_returns_false_the_second_time() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope_for(tmp.path());
        let guard = guard_for(tmp.path(), &scope);
        let db_path = tmp.path().join("history").join("events.sqlite3");
        let mut store = SqliteHistoryStore::open(&db_path).unwrap();

        let target = EventId::from_ulid(ulid::Ulid::new());
        store
            .record(
                &guard,
                &target,
                &draft(EventKind::Remove, "alpha", serde_json::json!({}), None),
            )
            .unwrap();
        store
            .finish(&guard, &target, EventStatus::Done, None)
            .unwrap();

        let first_restore = EventId::from_ulid(ulid::Ulid::new());
        let second_restore = EventId::from_ulid(ulid::Ulid::new());
        assert!(store.claim_revert(&guard, &target, &first_restore).unwrap());
        assert!(!store
            .claim_revert(&guard, &target, &second_restore)
            .unwrap());

        let row = store.get(&target).unwrap().unwrap();
        assert_eq!(row.reverted_by, Some(first_restore));
    }

    #[test]
    fn release_revert_clears_the_claim_it_holds() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope_for(tmp.path());
        let guard = guard_for(tmp.path(), &scope);
        let db_path = tmp.path().join("history").join("events.sqlite3");
        let mut store = SqliteHistoryStore::open(&db_path).unwrap();

        let target = EventId::from_ulid(ulid::Ulid::new());
        store
            .record(
                &guard,
                &target,
                &draft(EventKind::Remove, "alpha", serde_json::json!({}), None),
            )
            .unwrap();
        store
            .finish(&guard, &target, EventStatus::Done, None)
            .unwrap();

        let restore = EventId::from_ulid(ulid::Ulid::new());
        assert!(store.claim_revert(&guard, &target, &restore).unwrap());
        assert!(store.release_revert(&guard, &target, &restore).unwrap());

        let row = store.get(&target).unwrap().unwrap();
        assert_eq!(row.reverted_by, None);
    }

    #[test]
    fn release_revert_returns_false_when_another_restore_holds_the_claim() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope_for(tmp.path());
        let guard = guard_for(tmp.path(), &scope);
        let db_path = tmp.path().join("history").join("events.sqlite3");
        let mut store = SqliteHistoryStore::open(&db_path).unwrap();

        let target = EventId::from_ulid(ulid::Ulid::new());
        store
            .record(
                &guard,
                &target,
                &draft(EventKind::Remove, "alpha", serde_json::json!({}), None),
            )
            .unwrap();
        store
            .finish(&guard, &target, EventStatus::Done, None)
            .unwrap();

        let holder = EventId::from_ulid(ulid::Ulid::new());
        let impostor = EventId::from_ulid(ulid::Ulid::new());
        assert!(store.claim_revert(&guard, &target, &holder).unwrap());
        assert!(!store.release_revert(&guard, &target, &impostor).unwrap());

        let row = store.get(&target).unwrap().unwrap();
        assert_eq!(row.reverted_by, Some(holder));
    }

    #[test]
    fn a_row_with_an_unknown_kind_still_loads_and_is_not_restorable() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("history").join("events.sqlite3");
        let store = SqliteHistoryStore::open(&db_path).unwrap();
        store
            .conn
            .execute(
                "INSERT INTO events (id, ts, kind, skill, payload, inverse, status)
                 VALUES ('future1', '2026-09-09T00:00:00Z', 'future_kind', 'x', '{}', '{}', 'done')",
                [],
            )
            .unwrap();

        let row = store.get(&EventId("future1".to_string())).unwrap().unwrap();
        assert_eq!(row.kind(), None);
        assert_eq!(
            row.restore_capability(),
            skill_studio_core::dto::RestoreCapability::UnknownKind
        );
    }

    #[test]
    fn an_existing_desktop_schema_database_opens_without_migration() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("history").join("events.sqlite3");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();

        // Copied verbatim from the desktop's `event_store::open`.
        let legacy = Connection::open(&db_path).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE events (
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
                CREATE INDEX idx_events_skill ON events(skill, ts DESC);

                CREATE TABLE materialized_roots (
                    root_path   TEXT PRIMARY KEY,
                    harness     TEXT NOT NULL,
                    shared_root TEXT NOT NULL,
                    created_by  TEXT REFERENCES events(id)
                );
                CREATE TABLE materialized_disabled (
                    root_path   TEXT NOT NULL REFERENCES materialized_roots(root_path),
                    skill       TEXT NOT NULL,
                    PRIMARY KEY (root_path, skill)
                );
                INSERT INTO events (id, ts, kind, skill, payload, status)
                VALUES ('legacy', '2026-01-01T00:00:00Z', 'install', 'old', '{}', 'done');",
            )
            .unwrap();
        drop(legacy);

        let store = SqliteHistoryStore::open(&db_path).unwrap();
        let rows = store
            .list(&EventFilter {
                skill: None,
                limit: 10,
                after: None,
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, EventId("legacy".to_string()));
        assert_eq!(rows[0].kind, "install");
    }

    #[test]
    fn a_manifest_written_by_the_desktops_code_path_is_read_correctly_including_an_absent_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("backups").join("evt1");
        fs::create_dir_all(&dir).unwrap();
        // Byte-for-byte what the desktop's `EventStore::backup_paths` would
        // write for two paths, one present (index 0) and one absent.
        fs::write(
            dir.join("manifest.json"),
            r#"{
  "entries": {
    "/skills/alpha": {
      "relative_path": "0-alpha",
      "fingerprint": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85"
    },
    "/skills/gone": {
      "relative_path": "",
      "fingerprint": "absent"
    }
  }
}"#,
        )
        .unwrap();

        let manifest = read_manifest(&dir).unwrap();
        assert_eq!(manifest.entries.len(), 2);
        let present = &manifest.entries["/skills/alpha"];
        assert_eq!(present.relative_path, "0-alpha");
        assert_eq!(
            present.fingerprint,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85"
        );
        let absent = &manifest.entries["/skills/gone"];
        assert_eq!(absent.relative_path, "");
        assert_eq!(absent.fingerprint, "absent");
    }

    #[test]
    fn a_manifest_this_adapter_writes_parses_into_the_desktops_struct_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = scope_for(tmp.path());
        let guard = guard_for(tmp.path(), &scope);
        let db_path = tmp.path().join("history").join("events.sqlite3");
        let mut store = SqliteHistoryStore::open(&db_path).unwrap();

        let present = tmp.path().join("skills").join("alpha");
        fs::create_dir_all(&present).unwrap();
        fs::write(present.join("SKILL.md"), b"hello").unwrap();
        let absent = tmp.path().join("skills").join("gone");

        let id = EventId::from_ulid(ulid::Ulid::new());
        store
            .backup_paths(&guard, &id, &[present.clone(), absent.clone()])
            .unwrap();

        // Re-parse the manifest.json this adapter just wrote using a local
        // copy of the desktop's exact struct shape (field names, map keyed
        // by lossy absolute path), independent of this module's own reader.
        #[derive(Debug, Default, Deserialize)]
        struct DesktopShapeManifest {
            entries: std::collections::HashMap<String, DesktopShapeEntry>,
        }
        #[derive(Debug, Deserialize)]
        struct DesktopShapeEntry {
            relative_path: String,
            fingerprint: String,
        }

        let manifest_path = tmp
            .path()
            .join("history")
            .join("backups")
            .join(&id.0)
            .join("manifest.json");
        let bytes = fs::read(&manifest_path).unwrap();
        let parsed: DesktopShapeManifest = serde_json::from_slice(&bytes).unwrap();

        let present_key = present.to_string_lossy().into_owned();
        let absent_key = absent.to_string_lossy().into_owned();
        assert_eq!(parsed.entries[&present_key].relative_path, "0-alpha");
        assert_ne!(parsed.entries[&present_key].fingerprint, "absent");
        assert_eq!(parsed.entries[&absent_key].relative_path, "");
        assert_eq!(parsed.entries[&absent_key].fingerprint, "absent");
    }
}
