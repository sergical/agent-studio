// ============================================================================
// Skills Module - event_store
// Append-only log of every mutating operation Skill Studio performs
// (install/remove/park/harness-disable/etc.), plus the byte backups those
// mutations displace. Every mutating command follows the same five phases:
// allocate an id, back up anything about to be destroyed, record a
// `pending` row, perform the mutation, then `finish` the row `done` or
// `failed`. A crash leaves a `pending` row that `reconcile_at_startup`
// flips to `interrupted` on the next launch, so nothing silently vanishes.
//
// Restore semantics: each restorable event's `inverse` JSON is a tagged
// `InverseOp` carrying `pre_fingerprint` (the state the destination had
// *before* the original mutation - what restoring should bring back) and
// `post_fingerprint` (the state the mutation *left behind* - what the
// filesystem should still look like right before a restore runs). The
// drift guard in `restore` compares the destination's live fingerprint
// against `post_fingerprint`; a mismatch means something touched the path
// since the event, and restore refuses unless `force`. Restore is itself a
// mutation: before applying the inverse it backs up whatever currently sits
// at the destination (drifted or not) under its own event id and inserts a
// `restore` event with its own `RestoreBackup` inverse, so restore-of-restore
// is always possible and `force` never destroys the only copy of anything.
// ============================================================================

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Opens (creating if absent) the event store DB at `db_path` and ensures
/// its schema exists.
pub fn open(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    let conn = Connection::open(db_path)
        .map_err(|e| format!("Failed to open {}: {e}", db_path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| format!("Failed to set WAL mode: {e}"))?;
    // `reverted_by` is claimed (set to the restore event's id) before that
    // restore row exists - see `restore()` - so foreign key enforcement on
    // that column must stay off.
    conn.pragma_update(None, "foreign_keys", "OFF")
        .map_err(|e| format!("Failed to disable foreign keys: {e}"))?;
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
            reverted_by TEXT REFERENCES events(id)
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
        );",
    )
    .map_err(|e| format!("Failed to create event store schema: {e}"))?;
    Ok(conn)
}

/// Owns the event store connection plus the app data dir its backups live
/// under (`<app_data>/backups/<event-id>/`).
pub struct EventStore {
    pub conn: Connection,
    pub app_data: PathBuf,
}

impl EventStore {
    /// Opens `<app_data>/events.sqlite3`, creating `app_data` if needed.
    pub fn open(app_data: &Path) -> Result<Self, String> {
        fs::create_dir_all(app_data)
            .map_err(|e| format!("Failed to create {}: {e}", app_data.display()))?;
        let conn = open(&app_data.join("events.sqlite3"))?;
        Ok(Self {
            conn,
            app_data: app_data.to_path_buf(),
        })
    }

    fn backup_dir_for(&self, id: &str) -> PathBuf {
        self.app_data.join("backups").join(id)
    }

    /// Copies each existing top-level path under `paths` into
    /// `<app_data>/backups/<id>/<n>-<basename>` and writes a fsynced
    /// `manifest.json` mapping each original absolute path to its backup
    /// location and content fingerprint. Paths that don't exist are
    /// recorded with fingerprint `"absent"` and no bytes.
    pub fn backup_paths(&self, id: &str, paths: &[PathBuf]) -> Result<BackupManifest, String> {
        let dir = self.backup_dir_for(id);
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create backup dir {}: {e}", dir.display()))?;

        let mut manifest = BackupManifest::default();
        for (i, path) in paths.iter().enumerate() {
            let fingerprint = fingerprint_path(path);
            if fingerprint == "absent" {
                manifest.entries.insert(
                    path.to_string_lossy().into_owned(),
                    BackupEntry {
                        relative_path: String::new(),
                        fingerprint,
                    },
                );
                continue;
            }
            let basename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("path-{i}"));
            let relative_path = format!("{i}-{basename}");
            copy_recursive(path, &dir.join(&relative_path))?;
            manifest.entries.insert(
                path.to_string_lossy().into_owned(),
                BackupEntry {
                    relative_path,
                    fingerprint,
                },
            );
        }

        let json = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| format!("Failed to serialize backup manifest: {e}"))?;
        let manifest_path = dir.join("manifest.json");
        let mut file = File::create(&manifest_path)
            .map_err(|e| format!("Failed to write {}: {e}", manifest_path.display()))?;
        file.write_all(&json)
            .map_err(|e| format!("Failed to write {}: {e}", manifest_path.display()))?;
        file.sync_all()
            .map_err(|e| format!("Failed to fsync {}: {e}", manifest_path.display()))?;
        Ok(manifest)
    }

    fn read_manifest(&self, backup_dir: &Path) -> Result<BackupManifest, String> {
        let data = fs::read(backup_dir.join("manifest.json"))
            .map_err(|e| format!("Failed to read manifest in {}: {e}", backup_dir.display()))?;
        serde_json::from_slice(&data).map_err(|e| format!("Failed to parse manifest: {e}"))
    }

    /// Inserts a `pending` row for `id`.
    pub fn record(&self, id: &str, draft: EventDraft) -> Result<(), String> {
        let ts = Utc::now().to_rfc3339();
        let payload_json = serde_json::to_string(&draft.payload)
            .map_err(|e| format!("Failed to serialize payload: {e}"))?;
        let inverse_json = draft
            .inverse
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| format!("Failed to serialize inverse: {e}"))?;
        self.conn
            .execute(
                "INSERT INTO events
                    (id, ts, kind, skill, harness, scope, project_path, payload, inverse, backup_dir, status, reverted_by)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', NULL)",
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
                ],
            )
            .map_err(|e| format!("Failed to insert event {id}: {e}"))?;
        Ok(())
    }

    /// Marks `id` as `done` or `failed`.
    pub fn finish(&self, id: &str, status: EventStatus) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE events SET status = ?1 WHERE id = ?2",
                params![status.as_str(), id],
            )
            .map_err(|e| format!("Failed to update event {id}: {e}"))?;
        Ok(())
    }

    /// Looks up one event row by id - used by callers (e.g.
    /// `restore_guard_for_explode`) that need to inspect an event before
    /// deciding whether to restore it.
    pub fn get(&self, id: &str) -> Result<Option<EventRow>, String> {
        self.get_event(id)
    }

    fn get_event(&self, id: &str) -> Result<Option<EventRow>, String> {
        self.conn
            .query_row("SELECT * FROM events WHERE id = ?1", params![id], row_from)
            .optional()
            .map_err(|e| format!("Failed to query event {id}: {e}"))
    }

    /// Lists events newest-first (by insertion order - two ULIDs allocated
    /// in the same millisecond don't reliably sort, so `rowid` is the order).
    pub fn list(&self, limit: usize, skill: Option<&str>) -> Result<Vec<EventRow>, String> {
        let mut stmt = if skill.is_some() {
            self.conn
                .prepare("SELECT * FROM events WHERE skill = ?1 ORDER BY rowid DESC LIMIT ?2")
        } else {
            self.conn
                .prepare("SELECT * FROM events ORDER BY rowid DESC LIMIT ?1")
        }
        .map_err(|e| format!("Failed to prepare event list query: {e}"))?;

        let rows = if let Some(skill) = skill {
            stmt.query_map(params![skill, limit as i64], row_from)
        } else {
            stmt.query_map(params![limit as i64], row_from)
        }
        .map_err(|e| format!("Failed to list events: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read event row: {e}"))
    }

    /// Flips every `pending` row to `interrupted` (a crash is the only way
    /// one survives a restart) and returns the flipped rows.
    pub fn reconcile_at_startup(&self) -> Result<Vec<EventRow>, String> {
        let ids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM events WHERE status = 'pending'")
                .map_err(|e| format!("Failed to prepare pending query: {e}"))?;
            let mapped = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| format!("Failed to query pending events: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to read pending id: {e}"))?;
            mapped
        };
        for id in &ids {
            self.conn
                .execute(
                    "UPDATE events SET status = 'interrupted' WHERE id = ?1",
                    params![id],
                )
                .map_err(|e| format!("Failed to interrupt event {id}: {e}"))?;
        }
        ids.iter()
            .map(|id| {
                self.get_event(id)?
                    .ok_or_else(|| format!("Event {id} vanished mid-reconcile"))
            })
            .collect()
    }

    /// Undoes event `target_id`. Returns the id of the `restore` event
    /// created to do it. See the module header for the drift-guard and
    /// restore-of-restore design.
    pub fn restore(&self, target_id: &str, force: bool) -> Result<String, String> {
        let target = self
            .get_event(target_id)?
            .ok_or_else(|| format!("Event {target_id} not found"))?;
        if target.reverted_by.is_some() {
            return Err(format!("Event {target_id} was already restored"));
        }
        let inverse_value = target
            .inverse
            .clone()
            .ok_or_else(|| format!("Event {target_id} has no inverse and cannot be restored"))?;
        let inverse: InverseOp = serde_json::from_value(inverse_value)
            .map_err(|e| format!("Failed to parse inverse for {target_id}: {e}"))?;

        let restore_id = allocate_id();
        let claimed = self
            .conn
            .execute(
                "UPDATE events SET reverted_by = ?1 WHERE id = ?2 AND reverted_by IS NULL",
                params![restore_id, target_id],
            )
            .map_err(|e| format!("Failed to claim event {target_id}: {e}"))?;
        if claimed == 0 {
            return Err(format!("Event {target_id} was already restored"));
        }

        match self.apply_restore(&restore_id, &target, &inverse, force) {
            Ok(()) => Ok(restore_id),
            Err(e) => {
                let _ = self.conn.execute(
                    "UPDATE events SET reverted_by = NULL WHERE id = ?1 AND reverted_by = ?2",
                    params![target_id, restore_id],
                );
                Err(e)
            }
        }
    }

    fn apply_restore(
        &self,
        restore_id: &str,
        target: &EventRow,
        inverse: &InverseOp,
        force: bool,
    ) -> Result<(), String> {
        // `distribute_from_shared`'s inverse touches several paths (the
        // shared dir plus every copy it created), not the single destination
        // the generic flow below drift-checks and restores - see the module
        // header's "add a per-kind arm" note and `apply_restore_distribute`.
        if let InverseOp::UndistributeFromShared {
            shared_dir,
            copies,
            copy_fingerprints,
            symlinks,
            ..
        } = inverse
        {
            return self.apply_restore_distribute(
                restore_id,
                target,
                shared_dir,
                copies,
                copy_fingerprints,
                symlinks,
                force,
            );
        }

        let dest = inverse.destination().to_path_buf();
        let current_fp = fingerprint_path(&dest);
        if let Some(expected) = inverse.post_fingerprint() {
            if current_fp != *expected && !force {
                return Err(format!(
                    "{} has changed since the event that would be undone; use force to restore anyway (the current content will be backed up first)",
                    dest.display()
                ));
            }
        }

        // Phase 2: preserve whatever currently sits at the destination
        // (drifted or not) under the restore event's own backup dir.
        self.backup_paths(restore_id, std::slice::from_ref(&dest))?;
        let backup_dir = format!("backups/{restore_id}");

        // Phase 3: record the pending restore row, with a provisional
        // inverse (its post_fingerprint is patched in once we know the
        // state the restore itself leaves behind).
        let restore_inverse = InverseOp::RestoreBackup {
            path: dest.clone(),
            pre_fingerprint: current_fp,
            post_fingerprint: None,
        };
        self.record(
            restore_id,
            EventDraft {
                kind: "restore".to_string(),
                skill: target.skill.clone(),
                harness: target.harness.clone(),
                scope: target.scope.clone(),
                project_path: target.project_path.clone(),
                payload: serde_json::json!({ "target_event": target.id }),
                inverse: Some(
                    serde_json::to_value(&restore_inverse)
                        .map_err(|e| format!("Failed to serialize restore inverse: {e}"))?,
                ),
                backup_dir: Some(backup_dir),
            },
        )?;

        // Phase 4: apply the target event's inverse.
        match self.apply_inverse_op(inverse, target.backup_dir.as_deref()) {
            Ok(()) => {
                let post_fp = fingerprint_path(&dest);
                self.patch_inverse_post_fingerprint(restore_id, &post_fp)?;
                self.finish(restore_id, EventStatus::Done)?;
                Ok(())
            }
            Err(e) => {
                self.finish(restore_id, EventStatus::Failed)?;
                Err(e)
            }
        }
    }

    /// Patches an already-recorded event's `inverse.post_fingerprint` once
    /// the state its mutation left behind is known - the same pattern
    /// `apply_restore` uses for its own restore row. Exposed to
    /// `skill_materialize` so multi-step mutations (record pending, mutate,
    /// then learn the post-fingerprint) outside this module can do the same.
    pub(crate) fn patch_inverse_post_fingerprint(
        &self,
        id: &str,
        post_fp: &str,
    ) -> Result<(), String> {
        let row = self
            .get_event(id)?
            .ok_or_else(|| format!("Event {id} vanished before its inverse could be patched"))?;
        let mut inverse = row
            .inverse
            .ok_or_else(|| format!("Event {id} has no inverse to patch"))?;
        if let Some(obj) = inverse.as_object_mut() {
            obj.insert(
                "post_fingerprint".to_string(),
                Value::String(post_fp.to_string()),
            );
        }
        let json = serde_json::to_string(&inverse)
            .map_err(|e| format!("Failed to serialize patched inverse: {e}"))?;
        self.conn
            .execute(
                "UPDATE events SET inverse = ?1 WHERE id = ?2",
                params![json, id],
            )
            .map_err(|e| format!("Failed to patch inverse for {id}: {e}"))?;
        Ok(())
    }

    /// Applies one inverse op to the filesystem. `source_backup_dir` is the
    /// *original* event's backup dir, needed by `RestoreBackup` to find the
    /// bytes it's putting back.
    fn apply_inverse_op(
        &self,
        op: &InverseOp,
        source_backup_dir: Option<&str>,
    ) -> Result<(), String> {
        match op {
            InverseOp::RecreateSymlink { link, target, .. } => stage_replace_symlink(link, target),
            InverseOp::RemoveSymlink { link, .. } => {
                if let Ok(meta) = fs::symlink_metadata(link) {
                    if meta.file_type().is_symlink() {
                        fs::remove_file(link)
                            .map_err(|e| format!("Failed to remove {}: {e}", link.display()))?;
                    }
                }
                Ok(())
            }
            InverseOp::MoveBack { from, to, .. } => {
                if fs::symlink_metadata(to).is_ok() {
                    remove_path(to)?;
                }
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
                }
                fs::rename(from, to).map_err(|e| {
                    format!(
                        "Failed to move {} back to {}: {e}",
                        from.display(),
                        to.display()
                    )
                })
            }
            InverseOp::RestoreBackup { path, .. } => {
                let backup_dir_rel = source_backup_dir
                    .ok_or_else(|| "restore_backup has no source backup dir".to_string())?;
                self.restore_from_backup(backup_dir_rel, path)
            }
            // Handled by `apply_restore_distribute` before `apply_inverse_op`
            // is ever reached - see the "per-kind arm" note in `apply_restore`.
            InverseOp::UndistributeFromShared { .. } => Ok(()),
        }
    }

    /// Puts `path` back exactly as `backup_paths` found it under
    /// `backup_dir_rel` (relative to `app_data`) - removing whatever
    /// currently sits at `path` first, or leaving it absent if that's what
    /// was backed up. Shared by the generic `RestoreBackup` inverse and
    /// `apply_restore_distribute`'s shared-dir restore.
    fn restore_from_backup(&self, backup_dir_rel: &str, path: &Path) -> Result<(), String> {
        let backup_dir = self.app_data.join(backup_dir_rel);
        let manifest = self.read_manifest(&backup_dir)?;
        let key = path.to_string_lossy().into_owned();
        let entry = manifest
            .entries
            .get(&key)
            .ok_or_else(|| format!("No backup entry for {}", path.display()))?;
        if fs::symlink_metadata(path).is_ok() {
            remove_path(path)?;
        }
        if entry.fingerprint == "absent" {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
        }
        copy_recursive(&backup_dir.join(&entry.relative_path), path)
    }

    /// The `UndistributeFromShared` arm of `apply_restore`: refuses (without
    /// `force`) if any copy `distribute_from_shared` created has drifted from
    /// its fingerprint at distribution time, then backs up the shared dir and
    /// every copy (so this restore is itself restorable), puts the shared dir
    /// back from the original event's backup, deletes the copies, and
    /// recreates the symlinks that were removed.
    #[allow(clippy::too_many_arguments)]
    fn apply_restore_distribute(
        &self,
        restore_id: &str,
        target: &EventRow,
        shared_dir: &Path,
        copies: &[PathBuf],
        copy_fingerprints: &[String],
        symlinks: &[(PathBuf, PathBuf)],
        force: bool,
    ) -> Result<(), String> {
        if !force {
            for (path, expected) in copies.iter().zip(copy_fingerprints) {
                let current = fingerprint_path(path);
                if current != *expected {
                    return Err(format!(
                        "{} has changed since the event that would be undone; use force to restore anyway (the current content will be backed up first)",
                        path.display()
                    ));
                }
            }
        }

        // Phase 2: preserve whatever currently sits at every path this
        // restore is about to touch.
        let mut backup_targets = vec![shared_dir.to_path_buf()];
        backup_targets.extend(copies.iter().cloned());
        self.backup_paths(restore_id, &backup_targets)?;
        let backup_dir = format!("backups/{restore_id}");

        // Phase 3: record the pending restore row. Its own inverse only
        // covers putting the shared dir back - see the module doc note on
        // `apply_restore_distribute` for why a restore-of-this-restore
        // doesn't also recreate the copies/symlinks.
        let restore_inverse = InverseOp::RestoreBackup {
            path: shared_dir.to_path_buf(),
            pre_fingerprint: fingerprint_path(shared_dir),
            post_fingerprint: None,
        };
        self.record(
            restore_id,
            EventDraft {
                kind: "restore".to_string(),
                skill: target.skill.clone(),
                harness: target.harness.clone(),
                scope: target.scope.clone(),
                project_path: target.project_path.clone(),
                payload: serde_json::json!({ "target_event": target.id }),
                inverse: Some(
                    serde_json::to_value(&restore_inverse)
                        .map_err(|e| format!("Failed to serialize restore inverse: {e}"))?,
                ),
                backup_dir: Some(backup_dir),
            },
        )?;

        // Phase 4: put the shared dir back, delete the copies, recreate the
        // removed symlinks.
        let apply: Result<(), String> = (|| {
            let source_backup_dir = target
                .backup_dir
                .as_deref()
                .ok_or_else(|| "distribute_from_shared event has no backup dir".to_string())?;
            self.restore_from_backup(source_backup_dir, shared_dir)?;
            for copy in copies {
                if fs::symlink_metadata(copy).is_ok() {
                    remove_path(copy)?;
                }
            }
            for (link, link_target) in symlinks {
                if let Some(parent) = link.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
                }
                create_symlink(link_target, link)?;
            }
            Ok(())
        })();

        match apply {
            Ok(()) => {
                let post_fp = fingerprint_path(shared_dir);
                self.patch_inverse_post_fingerprint(restore_id, &post_fp)?;
                self.finish(restore_id, EventStatus::Done)?;
                Ok(())
            }
            Err(e) => {
                self.finish(restore_id, EventStatus::Failed)?;
                Err(e)
            }
        }
    }

    /// Records `root` as a harness skills dir that Skill Studio converted
    /// to per-skill links mirroring `shared_root` (see the spec's
    /// `explode_shared_dir`). Idempotent: re-registering updates the row.
    pub fn register_materialized_root(
        &self,
        root: &Path,
        harness: &str,
        shared_root: &Path,
        created_by: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO materialized_roots (root_path, harness, shared_root, created_by)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(root_path) DO UPDATE SET
                    harness = excluded.harness,
                    shared_root = excluded.shared_root,
                    created_by = excluded.created_by",
                params![
                    root.to_string_lossy(),
                    harness,
                    shared_root.to_string_lossy(),
                    created_by,
                ],
            )
            .map_err(|e| format!("Failed to register materialized root: {e}"))?;
        Ok(())
    }

    pub fn unregister_materialized_root(&self, root: &Path) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM materialized_disabled WHERE root_path = ?1",
                params![root.to_string_lossy()],
            )
            .map_err(|e| format!("Failed to clear materialized_disabled: {e}"))?;
        self.conn
            .execute(
                "DELETE FROM materialized_roots WHERE root_path = ?1",
                params![root.to_string_lossy()],
            )
            .map_err(|e| format!("Failed to unregister materialized root: {e}"))?;
        Ok(())
    }

    pub fn materialized_root(&self, root: &Path) -> Result<Option<MaterializedRoot>, String> {
        self.conn
            .query_row(
                "SELECT root_path, harness, shared_root, created_by FROM materialized_roots WHERE root_path = ?1",
                params![root.to_string_lossy()],
                |row| {
                    Ok(MaterializedRoot {
                        root_path: row.get(0)?,
                        harness: row.get(1)?,
                        shared_root: row.get(2)?,
                        created_by: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("Failed to query materialized root: {e}"))
    }

    pub fn set_materialized_disabled(
        &self,
        root: &Path,
        skill: &str,
        disabled: bool,
    ) -> Result<(), String> {
        if disabled {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO materialized_disabled (root_path, skill) VALUES (?1, ?2)",
                    params![root.to_string_lossy(), skill],
                )
                .map_err(|e| format!("Failed to disable {skill}: {e}"))?;
        } else {
            self.conn
                .execute(
                    "DELETE FROM materialized_disabled WHERE root_path = ?1 AND skill = ?2",
                    params![root.to_string_lossy(), skill],
                )
                .map_err(|e| format!("Failed to re-enable {skill}: {e}"))?;
        }
        Ok(())
    }

    pub fn materialized_disabled(&self, root: &Path) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT skill FROM materialized_disabled WHERE root_path = ?1")
            .map_err(|e| format!("Failed to prepare disabled query: {e}"))?;
        let mapped = stmt
            .query_map(params![root.to_string_lossy()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| format!("Failed to query disabled skills: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read disabled row: {e}"))?;
        Ok(mapped)
    }
}

fn row_from(row: &rusqlite::Row) -> rusqlite::Result<EventRow> {
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
    })
}

/// A fresh, sortable-by-time event id. No DB access.
pub fn allocate_id() -> String {
    ulid::Ulid::new().to_string()
}

/// Content fingerprint for drift detection and backup verification:
/// `"absent"` when nothing exists at `path` (checked via `symlink_metadata`
/// so a broken symlink still fingerprints as present), a SHA-256 of the
/// literal target string for a symlink, of the bytes for a file, and of the
/// sorted `(name, entry-fingerprint)` pairs for a directory (so a rename
/// inside a directory changes its fingerprint even if total bytes match).
pub fn fingerprint_path(path: &Path) -> String {
    if fs::symlink_metadata(path).is_err() {
        return "absent".to_string();
    }
    hash_entry(path).unwrap_or_else(|_| "absent".to_string())
}

fn hash_entry(path: &Path) -> std::io::Result<String> {
    let meta = fs::symlink_metadata(path)?;
    let file_type = meta.file_type();
    let mut hasher = Sha256::new();
    if file_type.is_symlink() {
        let target = fs::read_link(path)?;
        hasher.update(b"L");
        hasher.update(target.to_string_lossy().as_bytes());
    } else if file_type.is_dir() {
        hasher.update(b"D");
        let mut entries: Vec<_> = fs::read_dir(path)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name_bytes = entry
                .file_name()
                .to_string_lossy()
                .into_owned()
                .into_bytes();
            let child_fp = hash_entry(&entry.path())?;
            hasher.update((name_bytes.len() as u64).to_le_bytes());
            hasher.update(&name_bytes);
            hasher.update((child_fp.len() as u64).to_le_bytes());
            hasher.update(child_fp.as_bytes());
        }
    } else {
        hasher.update(b"F");
        let bytes = fs::read(path)?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// Copies `src` into `dest`, preserving regular files as bytes, directories
/// recursively, and symlinks as the literal link (never following it).
/// `pub(crate)`: Copy removal also uses it to stage and restore exact paths.
pub(crate) fn copy_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    let meta =
        fs::symlink_metadata(src).map_err(|e| format!("Failed to stat {}: {e}", src.display()))?;
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(src)
            .map_err(|e| format!("Failed to read link {}: {e}", src.display()))?;
        create_symlink(&target, dest)?;
    } else if file_type.is_dir() {
        fs::create_dir_all(dest)
            .map_err(|e| format!("Failed to create {}: {e}", dest.display()))?;
        for entry in
            fs::read_dir(src).map_err(|e| format!("Failed to read dir {}: {e}", src.display()))?
        {
            let entry = entry.map_err(|e| format!("Failed to read dir entry: {e}"))?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
    } else {
        fs::copy(src, dest).map_err(|e| {
            format!(
                "Failed to copy {} to {}: {e}",
                src.display(),
                dest.display()
            )
        })?;
    }
    Ok(())
}

fn create_symlink(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
            .map_err(|e| format!("Failed to symlink {}: {e}", link.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err("Symlinking is only supported on Unix".to_string())
    }
}

fn remove_path(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to stat {}: {e}", path.display()))?;
    if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(path).map_err(|e| format!("Failed to remove {}: {e}", path.display()))
    } else {
        fs::remove_file(path).map_err(|e| format!("Failed to remove {}: {e}", path.display()))
    }
}

/// Creates `link -> target` at a temp name in `link`'s parent, only then
/// removes whatever currently sits at `link`, then renames the temp link
/// into place - a crash mid-sequence leaves either the original entry or
/// the finished replacement, never neither.
fn stage_replace_symlink(link: &Path, target: &Path) -> Result<(), String> {
    let parent = link
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", link.display()))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    let tmp = parent.join(format!(".skill-studio-restore-{}", allocate_id()));
    create_symlink(target, &tmp)?;
    if fs::symlink_metadata(link).is_ok() {
        remove_path(link)?;
    }
    fs::rename(&tmp, link).map_err(|e| format!("Failed to move {} into place: {e}", link.display()))
}

/// Manifest written alongside a backup: original absolute path -> where its
/// bytes live inside the backup dir, plus its fingerprint at backup time.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BackupManifest {
    pub entries: std::collections::BTreeMap<String, BackupEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    /// Path relative to the backup dir; empty when the original was absent.
    pub relative_path: String,
    pub fingerprint: String,
}

/// Forward-mutation data for a not-yet-written event row.
#[derive(Debug, Clone)]
pub struct EventDraft {
    pub kind: String,
    pub skill: String,
    pub harness: Option<String>,
    pub scope: Option<String>,
    pub project_path: Option<String>,
    pub payload: Value,
    pub inverse: Option<Value>,
    /// Relative to `app_data`, e.g. `"backups/<id>"`.
    pub backup_dir: Option<String>,
}

pub enum EventStatus {
    Done,
    Failed,
}

impl EventStatus {
    fn as_str(&self) -> &'static str {
        match self {
            EventStatus::Done => "done",
            EventStatus::Failed => "failed",
        }
    }
}

/// One row of the `events` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRow {
    pub id: String,
    pub ts: String,
    pub kind: String,
    pub skill: String,
    pub harness: Option<String>,
    pub scope: Option<String>,
    pub project_path: Option<String>,
    pub payload: Value,
    pub inverse: Option<Value>,
    pub backup_dir: Option<String>,
    pub status: String,
    pub reverted_by: Option<String>,
}

/// How to undo one event. `pre_fingerprint` is the destination's
/// fingerprint before the forward mutation ran (what restoring recreates);
/// `post_fingerprint` is the fingerprint the mutation left behind (what the
/// drift guard checks the live filesystem against before restoring). It is
/// `Option` because it's only known once the forward mutation completes -
/// `record` writes it as `None` and the caller patches it in after phase 4,
/// same as `restore` patches its own inverse in `patch_inverse_post_fingerprint`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum InverseOp {
    /// Undo of a deleted symlink: recreate `link` pointing at `target`.
    RecreateSymlink {
        link: PathBuf,
        target: PathBuf,
        pre_fingerprint: String,
        post_fingerprint: Option<String>,
    },
    /// Undo of a created symlink (`pre_fingerprint` is `"absent"`): remove
    /// `link`, but only if it is still a symlink.
    RemoveSymlink {
        link: PathBuf,
        pre_fingerprint: String,
        post_fingerprint: Option<String>,
    },
    /// Undo of a rename: move `from` back to `to`.
    MoveBack {
        from: PathBuf,
        to: PathBuf,
        pre_fingerprint: String,
        post_fingerprint: Option<String>,
    },
    /// Undo of any mutation that backed up the destination first: copy its
    /// bytes back out of the original event's backup dir.
    RestoreBackup {
        path: PathBuf,
        pre_fingerprint: String,
        post_fingerprint: Option<String>,
    },
    /// Undo of `skill_materialize::distribute_from_shared`: put `shared_dir`
    /// back from the event's backup, delete every path in `copies` (real
    /// directories the operation created), and recreate every `(link,
    /// target)` in `symlinks` (the per-skill symlinks it removed to make
    /// room for those copies). `copy_fingerprints` is parallel to `copies` -
    /// each copy's fingerprint right after distribution, for the drift guard
    /// `apply_restore_distribute` runs instead of the generic single-path
    /// check the other variants get.
    UndistributeFromShared {
        shared_dir: PathBuf,
        copies: Vec<PathBuf>,
        copy_fingerprints: Vec<String>,
        symlinks: Vec<(PathBuf, PathBuf)>,
        pre_fingerprint: String,
        post_fingerprint: Option<String>,
    },
}

impl InverseOp {
    fn destination(&self) -> &Path {
        match self {
            InverseOp::RecreateSymlink { link, .. } => link,
            InverseOp::RemoveSymlink { link, .. } => link,
            InverseOp::MoveBack { to, .. } => to,
            InverseOp::RestoreBackup { path, .. } => path,
            InverseOp::UndistributeFromShared { shared_dir, .. } => shared_dir,
        }
    }

    fn post_fingerprint(&self) -> Option<&String> {
        match self {
            InverseOp::RecreateSymlink {
                post_fingerprint, ..
            }
            | InverseOp::RemoveSymlink {
                post_fingerprint, ..
            }
            | InverseOp::MoveBack {
                post_fingerprint, ..
            }
            | InverseOp::RestoreBackup {
                post_fingerprint, ..
            }
            | InverseOp::UndistributeFromShared {
                post_fingerprint, ..
            } => post_fingerprint.as_ref(),
        }
    }
}

/// A harness skills dir Skill Studio converted to per-skill links mirroring
/// a shared root (see the spec's `explode_shared_dir` / Materialize).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterializedRoot {
    pub root_path: String,
    pub harness: String,
    pub shared_root: String,
    pub created_by: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn store(dir: &Path) -> EventStore {
        EventStore::open(&dir.join("app_data")).expect("open store")
    }

    fn draft(
        kind: &str,
        skill: &str,
        payload: Value,
        inverse: Option<Value>,
        backup_dir: Option<String>,
    ) -> EventDraft {
        EventDraft {
            kind: kind.to_string(),
            skill: skill.to_string(),
            harness: Some("claude-code".to_string()),
            scope: Some("global".to_string()),
            project_path: None,
            payload,
            inverse,
            backup_dir,
        }
    }

    #[test]
    fn record_list_roundtrip_preserves_fields_and_order() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());

        let id1 = allocate_id();
        store
            .record(
                &id1,
                draft("install", "alpha", serde_json::json!({"n": 1}), None, None),
            )
            .unwrap();
        store.finish(&id1, EventStatus::Done).unwrap();

        let id2 = allocate_id();
        store
            .record(
                &id2,
                draft("remove", "alpha", serde_json::json!({"n": 2}), None, None),
            )
            .unwrap();
        store.finish(&id2, EventStatus::Done).unwrap();

        let rows = store.list(10, None).unwrap();
        assert_eq!(rows.len(), 2);
        // newest first
        assert_eq!(rows[0].id, id2);
        assert_eq!(rows[1].id, id1);
        assert_eq!(rows[1].kind, "install");
        assert_eq!(rows[1].skill, "alpha");
        assert_eq!(rows[1].payload, serde_json::json!({"n": 1}));
        assert_eq!(rows[1].status, "done");
        assert_eq!(rows[1].harness.as_deref(), Some("claude-code"));
        assert_eq!(rows[1].scope.as_deref(), Some("global"));

        let filtered = store.list(10, Some("alpha")).unwrap();
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn backup_and_restore_a_skill_folder_is_byte_identical() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());

        let skill_dir = tmp.path().join("skills").join("my-skill");
        fs::create_dir_all(skill_dir.join("nested")).unwrap();
        fs::write(skill_dir.join("SKILL.md"), b"---\nname: my-skill\n---\n").unwrap();
        fs::write(skill_dir.join("nested/file.txt"), b"hello").unwrap();
        symlink("nested/file.txt", skill_dir.join("link")).unwrap();

        let fp_before = fingerprint_path(&skill_dir);

        let id = allocate_id();
        store
            .backup_paths(&id, std::slice::from_ref(&skill_dir))
            .unwrap();
        fs::remove_dir_all(&skill_dir).unwrap();

        let inverse = InverseOp::RestoreBackup {
            path: skill_dir.clone(),
            pre_fingerprint: fp_before.clone(),
            post_fingerprint: Some("absent".to_string()),
        };
        store
            .record(
                &id,
                draft(
                    "remove",
                    "my-skill",
                    serde_json::json!({}),
                    Some(serde_json::to_value(&inverse).unwrap()),
                    Some(format!("backups/{id}")),
                ),
            )
            .unwrap();
        store.finish(&id, EventStatus::Done).unwrap();

        assert_eq!(fingerprint_path(&skill_dir), "absent");

        store.restore(&id, false).unwrap();
        assert_eq!(fingerprint_path(&skill_dir), fp_before);
    }

    #[test]
    fn restore_of_reverted_event_fails_and_restore_of_restore_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());

        let path = tmp.path().join("skills").join("beta");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("SKILL.md"), b"beta").unwrap();
        let fp_before = fingerprint_path(&path);

        let id = allocate_id();
        store
            .backup_paths(&id, std::slice::from_ref(&path))
            .unwrap();
        fs::remove_dir_all(&path).unwrap();
        let inverse = InverseOp::RestoreBackup {
            path: path.clone(),
            pre_fingerprint: fp_before.clone(),
            post_fingerprint: Some("absent".to_string()),
        };
        store
            .record(
                &id,
                draft(
                    "remove",
                    "beta",
                    serde_json::json!({}),
                    Some(serde_json::to_value(&inverse).unwrap()),
                    Some(format!("backups/{id}")),
                ),
            )
            .unwrap();
        store.finish(&id, EventStatus::Done).unwrap();

        let restore_id = store.restore(&id, false).unwrap();
        assert_eq!(fingerprint_path(&path), fp_before);

        let err = store.restore(&id, false).unwrap_err();
        assert!(err.contains("already restored"), "unexpected error: {err}");

        // Restore-of-restore: brings the path back to "absent", the state
        // right before the first restore ran.
        store.restore(&restore_id, false).unwrap();
        assert_eq!(fingerprint_path(&path), "absent");
    }

    #[test]
    fn failed_event_keeps_status_and_backup_pending_flips_to_interrupted() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());

        let path = tmp.path().join("skills").join("gamma");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("SKILL.md"), b"gamma").unwrap();

        let failed_id = allocate_id();
        store
            .backup_paths(&failed_id, std::slice::from_ref(&path))
            .unwrap();
        store
            .record(
                &failed_id,
                draft(
                    "remove",
                    "gamma",
                    serde_json::json!({}),
                    None,
                    Some(format!("backups/{failed_id}")),
                ),
            )
            .unwrap();
        store.finish(&failed_id, EventStatus::Failed).unwrap();

        let rows = store.list(10, Some("gamma")).unwrap();
        assert_eq!(rows[0].status, "failed");
        assert!(tmp
            .path()
            .join("app_data/backups")
            .join(&failed_id)
            .join("manifest.json")
            .exists());

        let pending_id = allocate_id();
        store
            .record(
                &pending_id,
                draft("remove", "gamma", serde_json::json!({}), None, None),
            )
            .unwrap();

        let flipped = store.reconcile_at_startup().unwrap();
        assert_eq!(flipped.len(), 1);
        assert_eq!(flipped[0].id, pending_id);
        assert_eq!(flipped[0].status, "interrupted");
    }

    #[test]
    fn drift_guard_refuses_without_force_and_force_preserves_drifted_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());

        let file = tmp.path().join("skills").join("delta").join("SKILL.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"original").unwrap();
        let fp_original = fingerprint_path(&file);

        let id = allocate_id();
        store
            .backup_paths(&id, std::slice::from_ref(&file))
            .unwrap();
        fs::write(&file, b"post-event").unwrap();
        let fp_post_event = fingerprint_path(&file);
        let inverse = InverseOp::RestoreBackup {
            path: file.clone(),
            pre_fingerprint: fp_original.clone(),
            post_fingerprint: Some(fp_post_event),
        };
        store
            .record(
                &id,
                draft(
                    "update",
                    "delta",
                    serde_json::json!({}),
                    Some(serde_json::to_value(&inverse).unwrap()),
                    Some(format!("backups/{id}")),
                ),
            )
            .unwrap();
        store.finish(&id, EventStatus::Done).unwrap();

        // Drift: someone edits the file after the event.
        fs::write(&file, b"drifted-by-user").unwrap();

        let err = store.restore(&id, false).unwrap_err();
        assert!(
            err.contains(&file.display().to_string()),
            "error should name the path: {err}"
        );
        assert_eq!(fs::read(&file).unwrap(), b"drifted-by-user");

        let restore_id = store.restore(&id, true).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"original");

        // The drifted bytes must be recoverable from the restore event's backup.
        store.restore(&restore_id, false).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"drifted-by-user");
    }
}
