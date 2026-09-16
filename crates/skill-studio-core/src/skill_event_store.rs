//! Existing event storage and inverse execution, retained for desktop compatibility.
//! This store creates/migrates its database and uses ambient filesystem paths.
//! It is not the scope-validated or read-only history service for CLI callers.

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
// `restore` event with its own `RestoreBackup` inverse. Most restore events
// can themselves be restored. A caller can mark one non-restorable when
// recreating bytes cannot recreate the matching ownership metadata.
// ============================================================================

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::skill_event_statements::row_from;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub use crate::skill_event::{
    BackupEntry, BackupManifest, EventDraft, EventRow, EventStatus, InverseOp, MaterializedRoot,
};

use crate::skill_document_write::{begin_skill_md_write_transaction, SkillMdWriteTransaction};

// This preflight rejects unsafe existing entries. It does not bind later VFS IO
// or prevent an external process from replacing an entry after the check.
fn check_existing_database_entries(database: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;

    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = database.as_os_str().to_os_string();
        name.push(suffix);
        let path = PathBuf::from(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && metadata.nlink() == 1 => {}
            Ok(_) => {
                return Err(format!(
                    "Event database entry must be a single-link regular file: {}",
                    path.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not inspect {}: {error}", path.display())),
        }
    }
    Ok(())
}

/// Opens (creating if absent) the event store DB at `db_path` and ensures
/// its schema exists.
pub fn open(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    check_existing_database_entries(db_path)?;
    let conn = Connection::open(db_path)
        .map_err(|e| format!("Failed to open {}: {e}", db_path.display()))?;
    crate::skill_event_schema::initialize(&conn)?;
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
        self.backup_paths_with_sync(id, paths, &mut sync_backup_path)
    }

    fn backup_paths_with_sync(
        &self,
        id: &str,
        paths: &[PathBuf],
        sync: &mut impl FnMut(&Path) -> Result<(), String>,
    ) -> Result<BackupManifest, String> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(
                "Backup ID must contain 1..128 ASCII letters, digits, hyphens or underscores"
                    .into(),
            );
        }
        let backups = self.app_data.join("backups");
        match fs::create_dir(&backups) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&backups)
                    .map_err(|error| format!("Cannot inspect backup root: {error}"))?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err("Backup root must be a real directory".into());
                }
            }
            Err(error) => return Err(format!("Cannot create backup root: {error}")),
        }
        let dir = self.backup_dir_for(id);
        fs::create_dir(&dir).map_err(|e| {
            format!(
                "Cannot reserve a new backup directory {}: {e}",
                dir.display()
            )
        })?;

        let mut manifest = BackupManifest::default();
        for (i, path) in paths.iter().enumerate() {
            let observed = fingerprint_path_checked(path).map_err(|error| {
                format!(
                    "Cannot fingerprint backup source {}: {error}",
                    path.display()
                )
            })?;
            let Some(fingerprint) = observed else {
                manifest.entries.insert(
                    path.to_string_lossy().into_owned(),
                    BackupEntry {
                        relative_path: String::new(),
                        fingerprint: "absent".into(),
                    },
                );
                continue;
            };
            let basename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("path-{i}"));
            let relative_path = format!("{i}-{basename}");
            let destination = dir.join(&relative_path);
            copy_recursive(path, &destination)?;
            if fingerprint_path_checked(&destination)
                .map_err(|error| format!("Cannot fingerprint backup copy: {error}"))?
                .as_deref()
                != Some(fingerprint.as_str())
            {
                return Err("Backup copy does not match the observed source".into());
            }
            sync_backup_tree(&destination, sync)?;
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
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&manifest_path)
            .map_err(|e| format!("Failed to write {}: {e}", manifest_path.display()))?;
        file.write_all(&json)
            .map_err(|e| format!("Failed to write {}: {e}", manifest_path.display()))?;
        drop(file);
        sync(&manifest_path)?;
        sync(&dir)?;
        sync(&self.app_data.join("backups"))?;
        sync(&self.app_data)?;
        Ok(manifest)
    }

    fn read_manifest(&self, backup_dir: &Path) -> Result<BackupManifest, String> {
        let data = fs::read(backup_dir.join("manifest.json"))
            .map_err(|e| format!("Failed to read manifest in {}: {e}", backup_dir.display()))?;
        serde_json::from_slice(&data).map_err(|e| format!("Failed to parse manifest: {e}"))
    }

    /// Inserts a `pending` row for `id`.
    pub fn record(&self, id: &str, draft: EventDraft) -> Result<(), String> {
        crate::skill_event_statements::insert_pending(
            &self.conn,
            id,
            &Utc::now().to_rfc3339(),
            draft,
        )
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

    /// Returns whether an unfinished event still needs recovery. A completed
    /// restore resolves its source event; a failed restore leaves it visible.
    pub fn has_interrupted_events(&self) -> Result<bool, String> {
        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM events AS event
                    WHERE event.status = 'interrupted'
                      AND NOT EXISTS (
                        SELECT 1 FROM events AS restoration
                        WHERE restoration.id = event.reverted_by
                          AND restoration.status = 'done'
                      )
                )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| format!("Failed to query interrupted events: {error}"))
    }

    /// Lists active events of one kind for dependency guards. Failed and
    /// already-restored events cannot own live filesystem state.
    pub fn active_events_of_kind(&self, kind: &str) -> Result<Vec<EventRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM events
                 WHERE kind = ?1 AND reverted_by IS NULL
                   AND status IN ('pending', 'done', 'interrupted')
                 ORDER BY rowid DESC",
            )
            .map_err(|e| format!("Failed to prepare active event query: {e}"))?;
        let rows = stmt
            .query_map(params![kind], row_from)
            .map_err(|e| format!("Failed to query active {kind} events: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read active {kind} event: {e}"))
    }

    pub fn interrupted_events_of_kind(&self, kind: &str) -> Result<Vec<EventRow>, String> {
        let mut statement = self.conn.prepare(
            "SELECT * FROM events WHERE kind = ?1 AND status = 'interrupted' AND reverted_by IS NULL ORDER BY rowid ASC",
        ).map_err(|error| format!("Failed to prepare recovery query: {error}"))?;
        let rows = statement
            .query_map(params![kind], row_from)
            .map_err(|error| format!("Failed to query recovery events: {error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read recovery event: {error}"))
    }

    /// Lists every interrupted Make event and restore event that startup must
    /// retry. Recovery handlers discard restores that do not target Make.
    pub fn interrupted_independent_copy_events(&self) -> Result<Vec<EventRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM events
                 WHERE status = 'interrupted'
                   AND kind IN ('make_independent_copy', 'restore')
                 ORDER BY rowid ASC",
            )
            .map_err(|e| format!("Failed to prepare independent-copy recovery query: {e}"))?;
        let rows = stmt
            .query_map([], row_from)
            .map_err(|e| format!("Failed to query interrupted independent-copy events: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read interrupted independent-copy event: {e}"))
    }

    /// Lists interrupted whole-root convert-and-disable intents in creation order.
    pub fn interrupted_convert_then_disable_events(&self) -> Result<Vec<EventRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM events
                 WHERE status = 'interrupted' AND kind = 'materialize_then_disable'
                 ORDER BY rowid ASC",
            )
            .map_err(|e| format!("Failed to prepare convert-and-disable recovery query: {e}"))?;
        let rows = stmt
            .query_map([], row_from)
            .map_err(|e| format!("Failed to query interrupted convert-and-disable events: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read interrupted convert-and-disable event: {e}"))
    }

    /// Interrupted deterministic frontmatter repairs that startup can finish
    /// from their backend-generated, fingerprint-bound intent.
    pub fn interrupted_frontmatter_repair_events(&self) -> Result<Vec<EventRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM events
                 WHERE status = 'interrupted' AND kind = 'repair_skill_frontmatter'
                 ORDER BY rowid ASC",
            )
            .map_err(|e| format!("Failed to prepare frontmatter repair recovery query: {e}"))?;
        let rows = stmt
            .query_map([], row_from)
            .map_err(|e| format!("Failed to query interrupted frontmatter repairs: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read interrupted frontmatter repair: {e}"))
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
        self.restore_with_skill_md_transaction_observer(target_id, force, |_| {})
    }

    fn restore_with_skill_md_transaction_observer(
        &self,
        target_id: &str,
        force: bool,
        observe_transaction: impl FnOnce(Option<&SkillMdWriteTransaction>),
    ) -> Result<String, String> {
        let target = self
            .get_event(target_id)?
            .ok_or_else(|| format!("Event {target_id} not found"))?;
        if target.reverted_by.is_some() {
            return Err(format!("Event {target_id} was already restored"));
        }
        if !target.restorable {
            return Err(format!("Event {target_id} is not restorable"));
        }
        let inverse_value = target
            .inverse
            .clone()
            .ok_or_else(|| format!("Event {target_id} has no inverse and cannot be restored"))?;
        let inverse: InverseOp = serde_json::from_value(inverse_value)
            .map_err(|e| format!("Failed to parse inverse for {target_id}: {e}"))?;

        let skill_md_transaction = (inverse
            .destination()
            .file_name()
            .and_then(|name| name.to_str())
            == Some("SKILL.md"))
        .then(begin_skill_md_write_transaction)
        .transpose()?;
        observe_transaction(skill_md_transaction.as_ref());

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

        let result = match self.apply_restore(&restore_id, &target, &inverse, force) {
            Ok(()) => Ok(restore_id),
            Err(e) => {
                let _ = self.conn.execute(
                    "UPDATE events SET reverted_by = NULL WHERE id = ?1 AND reverted_by = ?2",
                    params![target_id, restore_id],
                );
                Err(e)
            }
        };
        drop(skill_md_transaction);
        result
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
                restorable: true,
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

    /// Replaces an already-recorded event payload. Independent copy records
    /// intent first, then fills fingerprints, Copy ownership, and any
    /// whole-root conversion id once those values exist.
    pub fn patch_event_payload(&self, id: &str, payload: &Value) -> Result<(), String> {
        let json = serde_json::to_string(payload)
            .map_err(|e| format!("Failed to serialize payload for {id}: {e}"))?;
        self.conn
            .execute(
                "UPDATE events SET payload = ?1 WHERE id = ?2",
                params![json, id],
            )
            .map_err(|e| format!("Failed to patch payload for {id}: {e}"))?;
        Ok(())
    }

    /// Replaces an already-recorded inverse. Whole-root independent copies
    /// record intent before the per-skill link exists, then fill RecreateSymlink.
    pub fn patch_event_inverse(&self, id: &str, inverse: &Value) -> Result<(), String> {
        let json = serde_json::to_string(inverse)
            .map_err(|e| format!("Failed to serialize inverse for {id}: {e}"))?;
        self.conn
            .execute(
                "UPDATE events SET inverse = ?1 WHERE id = ?2",
                params![json, id],
            )
            .map_err(|e| format!("Failed to patch inverse for {id}: {e}"))?;
        Ok(())
    }

    /// Claims `target_id` for restore event `restore_id`. Zero rows means
    /// another restore already claimed it.
    pub fn claim_event_restore(&self, target_id: &str, restore_id: &str) -> Result<(), String> {
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
        Ok(())
    }

    /// Clears a restore claim so an interrupted undo can be retried or
    /// abandoned without leaving the original event unrestorable.
    pub fn unclaim_event_restore(&self, target_id: &str, restore_id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE events SET reverted_by = NULL WHERE id = ?1 AND reverted_by = ?2",
                params![target_id, restore_id],
            )
            .map_err(|e| format!("Failed to unclaim event {target_id}: {e}"))?;
        Ok(())
    }

    /// Patches an already-recorded event's `inverse.post_fingerprint` once
    /// the state its mutation left behind is known - the same pattern
    /// `apply_restore` uses for its own restore row. Exposed to
    /// `skill_materialize` so multi-step mutations (record pending, mutate,
    /// then learn the post-fingerprint) outside this module can do the same.
    pub fn patch_inverse_post_fingerprint(&self, id: &str, post_fp: &str) -> Result<(), String> {
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
                restorable: true,
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
    fingerprint_path_checked(path)
        .ok()
        .flatten()
        .unwrap_or_else(|| "absent".to_string())
}

pub fn fingerprint_path_checked(path: &Path) -> std::io::Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(_) => hash_entry(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn fingerprint_regular_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"F");
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
        let mut entries: Vec<_> = fs::read_dir(path)?.collect::<Result<_, _>>()?;
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
    } else if file_type.is_file() {
        return Ok(fingerprint_regular_bytes(&fs::read(path)?));
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Unsupported filesystem entry",
        ));
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn sync_backup_path(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("Failed to sync backup {}: {error}", path.display()))
}

fn sync_backup_tree(
    path: &Path,
    sync: &mut impl FnMut(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Cannot inspect backup {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
            sync_backup_tree(&entry.map_err(|error| error.to_string())?.path(), sync)?;
        }
    } else if !metadata.is_file() {
        return Err("Backup contains an unsupported filesystem entry".into());
    }
    sync(path)
}

/// Copies `src` into `dest`, preserving regular files as bytes, directories
/// recursively, and symlinks as the literal link (never following it).
/// Copy removal also uses this primitive to stage and restore exact paths.
pub fn copy_recursive(src: &Path, dest: &Path) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_document_write::skill_md_write_transaction_is_held;
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
            restorable: true,
        }
    }

    #[test]
    fn recovery_gate_requires_a_completed_restoration() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(temp.path());
        store
            .record(
                "original",
                draft("unlink_harness", "sample", Value::Null, None, None),
            )
            .unwrap();
        store.reconcile_at_startup().unwrap();
        store.claim_event_restore("original", "restore").unwrap();
        assert_eq!(
            crate::skill_event_statements::unresolved_event(&store.conn)
                .unwrap()
                .as_deref(),
            Some("original")
        );
        store
            .record(
                "restore",
                draft("restore", "sample", Value::Null, None, None),
            )
            .unwrap();
        assert!(crate::skill_event_statements::require_recovered(&store.conn).is_err());
        store.finish("restore", EventStatus::Failed).unwrap();
        assert!(crate::skill_event_statements::require_recovered(&store.conn).is_err());
        store.finish("restore", EventStatus::Done).unwrap();
        assert!(crate::skill_event_statements::require_recovered(&store.conn).is_ok());
        store
            .record(
                "next",
                draft("remove_copy_deployment", "sample", Value::Null, None, None),
            )
            .unwrap();
        assert_eq!(
            crate::skill_event_statements::unresolved_event(&store.conn)
                .unwrap()
                .as_deref(),
            Some("next")
        );
    }

    #[test]
    fn open_accepts_existing_live_wal_sidecars() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("events.sqlite3");
        let first = open(&database).unwrap();
        first
            .execute_batch(
                "CREATE TABLE opening_probe (value INTEGER); INSERT INTO opening_probe VALUES (1)",
            )
            .unwrap();
        assert!(temp.path().join("events.sqlite3-wal").is_file());
        assert!(temp.path().join("events.sqlite3-shm").is_file());
        let second = open(&database).unwrap();
        second
            .execute("INSERT INTO opening_probe VALUES (2)", [])
            .unwrap();
        let total: i64 = first
            .query_row("SELECT SUM(value) FROM opening_probe", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 3);
    }

    #[test]
    fn open_refuses_linked_or_special_database_entries_before_sqlite_changes_files() {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            for kind in ["symlink", "dangling", "hardlink", "directory"] {
                let temp = tempfile::tempdir().unwrap();
                let state = temp.path().join("state");
                fs::create_dir(&state).unwrap();
                let outside = temp.path().join("outside");
                let original = b"outside fixture must stay unchanged";
                fs::write(&outside, original).unwrap();
                let entry = state.join(format!("events.sqlite3{suffix}"));
                match kind {
                    "symlink" => symlink(&outside, &entry).unwrap(),
                    "dangling" => symlink(temp.path().join("absent"), &entry).unwrap(),
                    "hardlink" => fs::hard_link(&outside, &entry).unwrap(),
                    "directory" => fs::create_dir(&entry).unwrap(),
                    _ => unreachable!(),
                }
                let error = open(&state.join("events.sqlite3")).unwrap_err();
                assert!(
                    error.contains("single-link regular file"),
                    "{kind} {suffix}: {error}"
                );
                assert_eq!(fs::read(&outside).unwrap(), original);
                assert_eq!(fs::read_dir(&state).unwrap().count(), 1);
                assert!(!temp.path().join("absent").exists());
                if kind == "symlink" || kind == "dangling" {
                    assert!(fs::symlink_metadata(&entry)
                        .unwrap()
                        .file_type()
                        .is_symlink());
                }
            }
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
    fn interrupted_status_ignores_history_limit_and_resolved_events() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());

        store
            .record(
                "interrupted",
                draft("install", "interrupted", serde_json::json!({}), None, None),
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE events SET status = 'interrupted' WHERE id = 'interrupted'",
                [],
            )
            .unwrap();
        for index in 0..201 {
            let id = format!("done-{index}");
            store
                .record(
                    &id,
                    draft("install", &id, serde_json::json!({}), None, None),
                )
                .unwrap();
            store.finish(&id, EventStatus::Done).unwrap();
        }

        assert_eq!(store.list(200, None).unwrap().len(), 200);
        assert!(store.has_interrupted_events().unwrap());

        store
            .conn
            .execute(
                "UPDATE events SET reverted_by = 'done-0' WHERE id = 'interrupted'",
                [],
            )
            .unwrap();
        assert!(!store.has_interrupted_events().unwrap());

        store
            .record(
                "failed",
                draft("install", "failed", serde_json::json!({}), None, None),
            )
            .unwrap();
        store.finish("failed", EventStatus::Failed).unwrap();
        assert!(!store.has_interrupted_events().unwrap());

        store
            .conn
            .execute(
                "UPDATE events SET reverted_by = 'failed' WHERE id = 'interrupted'",
                [],
            )
            .unwrap();
        assert!(store.has_interrupted_events().unwrap());

        store
            .conn
            .execute(
                "UPDATE events SET reverted_by = 'done-0' WHERE id = 'interrupted'",
                [],
            )
            .unwrap();
        assert!(!store.has_interrupted_events().unwrap());

        store
            .record(
                "pending",
                draft("install", "pending", serde_json::json!({}), None, None),
            )
            .unwrap();
        assert!(!store.has_interrupted_events().unwrap());
    }

    #[test]
    fn receipt_schema_migrates_without_changing_history_and_survives_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("events.sqlite3");
        let legacy = Connection::open(&database).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE events (
                id TEXT PRIMARY KEY, ts TEXT NOT NULL, kind TEXT NOT NULL,
                skill TEXT NOT NULL, harness TEXT, scope TEXT, project_path TEXT,
                payload TEXT NOT NULL, inverse TEXT, status TEXT NOT NULL, reverted_by TEXT
            );
            INSERT INTO events (id, ts, kind, skill, payload, status)
            VALUES ('legacy', '2026-01-01T00:00:00Z', 'install', 'old', '{}', 'done');",
            )
            .unwrap();
        drop(legacy);
        let migrated = open(&database).unwrap();
        let digest = [7_u8; 32];
        migrated.execute(
            "INSERT INTO event_command_receipts (command_id, event_id, digest) VALUES (?1, ?2, ?3)",
            params!["command", "legacy", digest.as_slice()],
        ).unwrap();
        for sql in [
            "INSERT INTO event_command_receipts VALUES ('command', 'legacy', zeroblob(32))",
            "INSERT INTO event_command_receipts VALUES (NULL, 'legacy', zeroblob(32))",
            "INSERT INTO event_command_receipts VALUES ('short', 'legacy', zeroblob(31))",
            "INSERT INTO event_command_receipts VALUES ('text', 'legacy', '12345678901234567890123456789012')",
        ] {
            assert!(migrated.execute(sql, []).is_err());
        }
        drop(migrated);
        let reopened = open(&database).unwrap();
        let stored: (String, Vec<u8>) = reopened
            .query_row(
                "SELECT event_id, digest FROM event_command_receipts WHERE command_id = 'command'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored, ("legacy".into(), digest.to_vec()));
        let history: (String, String, String, bool) = reopened
            .query_row(
                "SELECT ts, payload, status, restorable FROM events WHERE id = 'legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            history,
            (
                "2026-01-01T00:00:00Z".into(),
                "{}".into(),
                "done".into(),
                true
            )
        );
    }

    #[test]
    fn schema_migration_defaults_existing_events_to_restorable() {
        let tmp = tempfile::tempdir().unwrap();
        let app_data = tmp.path().join("app_data");
        fs::create_dir_all(&app_data).unwrap();
        let db_path = app_data.join("events.sqlite3");
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE events (
                    id TEXT PRIMARY KEY, ts TEXT NOT NULL, kind TEXT NOT NULL,
                    skill TEXT NOT NULL, harness TEXT, scope TEXT, project_path TEXT,
                    payload TEXT NOT NULL, inverse TEXT, backup_dir TEXT,
                    status TEXT NOT NULL, reverted_by TEXT
                );
                INSERT INTO events VALUES
                    ('legacy', '2026-09-05T00:00:00Z', 'install', 'find-bugs', NULL,
                     NULL, NULL, '{}', NULL, NULL, 'done', NULL);",
            )
            .unwrap();
        drop(connection);

        let store = EventStore::open(&app_data).unwrap();
        assert!(store.get("legacy").unwrap().unwrap().restorable);
    }

    #[test]
    fn open_migrates_legacy_events_schema_without_backup_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let app_data = tmp.path().join("app_data");
        fs::create_dir_all(&app_data).unwrap();
        let db_path = app_data.join("events.sqlite3");
        let legacy = Connection::open(&db_path).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE events (
                    id TEXT PRIMARY KEY, ts TEXT NOT NULL, kind TEXT NOT NULL,
                    skill TEXT NOT NULL, harness TEXT, scope TEXT, project_path TEXT,
                    payload TEXT NOT NULL, inverse TEXT, status TEXT NOT NULL, reverted_by TEXT
                );
                INSERT INTO events (id, ts, kind, skill, payload, status)
                VALUES ('legacy', '2026-01-01T00:00:00Z', 'install', 'old', '{}', 'done');",
            )
            .unwrap();
        drop(legacy);

        let store = EventStore::open(&app_data).unwrap();
        let id = allocate_id();
        let backup_dir = format!("backups/{id}");
        store
            .record(
                &id,
                draft(
                    "remove",
                    "new",
                    serde_json::json!({}),
                    None,
                    Some(backup_dir.clone()),
                ),
            )
            .unwrap();
        store.finish(&id, EventStatus::Done).unwrap();

        let rows = store.list(10, None).unwrap();
        assert_eq!(rows[0].backup_dir.as_deref(), Some(backup_dir.as_str()));
        assert_eq!(rows[1].id, "legacy");
        assert_eq!(rows[1].backup_dir, None);
    }

    #[test]
    fn backup_ids_cannot_escape_or_overwrite_an_existing_operation() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let store = EventStore::open(&state).unwrap();
        let source = temp.path().join("document");
        fs::write(&source, b"original").unwrap();
        for id in [
            "",
            ".",
            "..",
            "../outside",
            "/absolute",
            "nested/id",
            "a\\b",
            "white space",
            "é",
        ] {
            assert!(store
                .backup_paths(id, std::slice::from_ref(&source))
                .is_err());
            assert!(!state.join("backups").exists());
        }
        assert!(store
            .backup_paths(&"a".repeat(129), std::slice::from_ref(&source))
            .is_err());
        store
            .backup_paths("same-id", std::slice::from_ref(&source))
            .unwrap();
        let before = fingerprint_path_checked(&state.join("backups")).unwrap();
        fs::write(&source, b"new source").unwrap();
        assert!(store
            .backup_paths("same-id", std::slice::from_ref(&source))
            .is_err());
        assert_eq!(
            fingerprint_path_checked(&state.join("backups")).unwrap(),
            before
        );
        fs::create_dir(state.join("backups/partial")).unwrap();
        fs::write(state.join("backups/partial/preserve"), b"partial backup").unwrap();
        assert!(store.backup_paths("partial", &[source]).is_err());
        assert_eq!(
            fs::read(state.join("backups/partial/preserve")).unwrap(),
            b"partial backup"
        );
    }

    #[cfg(unix)]
    #[test]
    fn backup_refuses_existing_root_and_operation_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let store = EventStore::open(&state).unwrap();
        std::os::unix::fs::symlink(&outside, state.join("backups")).unwrap();
        assert!(store.backup_paths("fixture", &[]).is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        fs::remove_file(state.join("backups")).unwrap();
        fs::create_dir(state.join("backups")).unwrap();
        std::os::unix::fs::symlink(&outside, state.join("backups/fixture")).unwrap();
        assert!(store.backup_paths("fixture", &[]).is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[test]
    fn concurrent_backup_reservations_have_one_winner() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let first = EventStore::open(&state).unwrap();
        let second = EventStore::open(&state).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let other_barrier = barrier.clone();
        let child = std::thread::spawn(move || {
            other_barrier.wait();
            first.backup_paths("same-operation", &[])
        });
        barrier.wait();
        let result = second.backup_paths("same-operation", &[]);
        assert_ne!(child.join().unwrap().is_ok(), result.is_ok());
        let manifest = fs::read(state.join("backups/same-operation/manifest.json")).unwrap();
        assert!(serde_json::from_slice::<BackupManifest>(&manifest).is_ok());
    }

    #[test]
    fn backup_syncs_bytes_before_manifest_and_directories_before_return() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("content"), b"preserved").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/unavailable-backup-target", source.join("link")).unwrap();
        let state = temp.path().join("state");
        let store = EventStore::open(&state).unwrap();
        let mut synced = Vec::new();
        let manifest = store
            .backup_paths_with_sync("fixture", std::slice::from_ref(&source), &mut |path| {
                sync_backup_path(path)?;
                synced.push(path.to_path_buf());
                Ok(())
            })
            .unwrap();
        let backup = state.join("backups/fixture");
        assert_eq!(
            synced,
            vec![
                backup.join("0-source/content"),
                backup.join("0-source"),
                backup.join("manifest.json"),
                backup.clone(),
                state.join("backups"),
                state.clone(),
            ]
        );
        assert_eq!(
            fs::read(backup.join("0-source/content")).unwrap(),
            b"preserved"
        );
        assert_eq!(
            manifest.entries[source.to_str().unwrap()].fingerprint,
            fingerprint_path(&source)
        );
    }

    #[test]
    fn backup_sync_failure_never_reports_success_or_changes_source() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("document");
        fs::write(&source, b"preserve me").unwrap();
        let store = EventStore::open(&temp.path().join("state")).unwrap();
        for fail_at in 0..5 {
            let id = format!("failure-{fail_at}");
            let mut calls = 0;
            let result =
                store.backup_paths_with_sync(&id, std::slice::from_ref(&source), &mut |path| {
                    let current = calls;
                    calls += 1;
                    if current == fail_at {
                        return Err("fixture sync failure".into());
                    }
                    sync_backup_path(path)
                });
            assert_eq!(result.unwrap_err(), "fixture sync failure");
            assert_eq!(calls, fail_at + 1);
            assert_eq!(fs::read(&source).unwrap(), b"preserve me");
            if fail_at == 0 {
                assert!(!store.backup_dir_for(&id).join("manifest.json").exists());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn backup_refuses_unreadable_special_entries_instead_of_recording_absence() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("socket");
        let _socket = std::os::unix::net::UnixListener::bind(&source).unwrap();
        let store = EventStore::open(&temp.path().join("state")).unwrap();
        assert!(store.backup_paths("special", &[source]).is_err());
        assert!(!store
            .backup_dir_for("special")
            .join("manifest.json")
            .exists());
        let missing = temp.path().join("missing");
        let manifest = store
            .backup_paths("absent", std::slice::from_ref(&missing))
            .unwrap();
        assert_eq!(
            manifest.entries[missing.to_str().unwrap()].fingerprint,
            "absent"
        );
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
    fn repair_undo_and_redo_hold_skill_md_transaction_but_other_files_do_not() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(tmp.path());
        let skill_md = tmp.path().join("skills/sample/SKILL.md");
        fs::create_dir_all(skill_md.parent().unwrap()).unwrap();
        fs::write(&skill_md, b"malformed frontmatter").unwrap();
        let malformed_fingerprint = fingerprint_path(&skill_md);

        let repair_id = allocate_id();
        store
            .backup_paths(&repair_id, std::slice::from_ref(&skill_md))
            .unwrap();
        fs::write(&skill_md, b"repaired frontmatter").unwrap();
        let repair_inverse = InverseOp::RestoreBackup {
            path: skill_md.clone(),
            pre_fingerprint: malformed_fingerprint,
            post_fingerprint: Some(fingerprint_path(&skill_md)),
        };
        store
            .record(
                &repair_id,
                draft(
                    "repair_skill_frontmatter",
                    "sample",
                    serde_json::json!({}),
                    Some(serde_json::to_value(repair_inverse).unwrap()),
                    Some(format!("backups/{repair_id}")),
                ),
            )
            .unwrap();
        store.finish(&repair_id, EventStatus::Done).unwrap();

        let undo_id = store
            .restore_with_skill_md_transaction_observer(&repair_id, false, |transaction| {
                assert!(transaction.is_some());
                assert!(skill_md_write_transaction_is_held());
            })
            .unwrap();
        assert_eq!(fs::read(&skill_md).unwrap(), b"malformed frontmatter");
        assert_eq!(
            store
                .get(&repair_id)
                .unwrap()
                .unwrap()
                .reverted_by
                .as_deref(),
            Some(undo_id.as_str())
        );

        let redo_id = store
            .restore_with_skill_md_transaction_observer(&undo_id, false, |transaction| {
                assert!(transaction.is_some());
                assert!(skill_md_write_transaction_is_held());
            })
            .unwrap();
        assert_eq!(fs::read(&skill_md).unwrap(), b"repaired frontmatter");
        assert_eq!(
            store.get(&undo_id).unwrap().unwrap().reverted_by.as_deref(),
            Some(redo_id.as_str())
        );
        assert_eq!(store.get(&redo_id).unwrap().unwrap().status, "done");

        let ordinary_file = tmp.path().join("notes.md");
        fs::write(&ordinary_file, b"before").unwrap();
        let ordinary_before_fingerprint = fingerprint_path(&ordinary_file);
        let ordinary_id = allocate_id();
        store
            .backup_paths(&ordinary_id, std::slice::from_ref(&ordinary_file))
            .unwrap();
        fs::write(&ordinary_file, b"after").unwrap();
        let ordinary_inverse = InverseOp::RestoreBackup {
            path: ordinary_file.clone(),
            pre_fingerprint: ordinary_before_fingerprint,
            post_fingerprint: Some(fingerprint_path(&ordinary_file)),
        };
        store
            .record(
                &ordinary_id,
                draft(
                    "update_notes",
                    "sample",
                    serde_json::json!({}),
                    Some(serde_json::to_value(ordinary_inverse).unwrap()),
                    Some(format!("backups/{ordinary_id}")),
                ),
            )
            .unwrap();
        store.finish(&ordinary_id, EventStatus::Done).unwrap();

        store
            .restore_with_skill_md_transaction_observer(&ordinary_id, false, |transaction| {
                assert!(transaction.is_none());
            })
            .unwrap();
        assert_eq!(fs::read(ordinary_file).unwrap(), b"before");
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
