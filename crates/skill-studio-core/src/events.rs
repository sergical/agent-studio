//! History events: kinds, rows, drafts, backups, and startup recovery.
//!
//! The SQLite schema does not change. `kind` stays a string column so rows
//! written by older versions still load; [`EventKind`] is the typed view.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dto::{DriftState, EventDto, RestoreCapability};
use crate::error::{CoreError, ErrorCode};
use crate::identity::{AgentId, EventId, Fingerprint, SkillName};
use crate::ports::{CoreNotice, EventSink, ExclusiveGuard, FileKind, HistoryStore, ScopeFs};

/// Known event kinds.
///
/// Invariant: `as_str` returns the exact literal the desktop writes today,
/// and `parse` accepts every literal ever written. Unknown literals are not
/// an error; they render as [`RestoreCapability::UnknownKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Skill installed.
    Install,
    /// Skill removed.
    Remove,
    /// Skill updated from its source.
    Update,
    /// Universal skill moved to the parked root.
    Park,
    /// Parked skill moved back.
    Unpark,
    /// Native harness disable written.
    HarnessDisable,
    /// Native harness disable cleared.
    HarnessEnable,
    /// Folder moved into `.skill-studio-disabled`.
    MoveAsideDisable,
    /// Folder moved back out of `.skill-studio-disabled`.
    MoveAsideRestore,
    /// Invocation policy rewritten.
    InvocationChange,
    /// Fork created.
    Fork,
    /// Per-skill harness link removed.
    UnlinkHarness,
    /// Per-skill harness link recreated.
    RelinkHarness,
    /// Whole-dir link replaced by per-skill links.
    ExplodeSharedDir,
    /// Explode, then unlink one skill.
    MaterializeThenDisable,
    /// Reconcile removed a link whose target vanished.
    ReconcileRemoveStaleLink,
    /// Link repair removed a broken link.
    RepairRemoveLink,
    /// Link repair pointed a link at a new target.
    RepairRelinkLink,
    /// Linked deployment replaced by a copy.
    MakeIndependentCopy,
    /// `SKILL.md` frontmatter rewritten.
    RepairSkillFrontmatter,
    /// Undo of another event; `payload.target_event` names it.
    Restore,
}

impl EventKind {
    /// Every kind, in declaration order.
    pub const ALL: [EventKind; 21] = [
        EventKind::Install,
        EventKind::Remove,
        EventKind::Update,
        EventKind::Park,
        EventKind::Unpark,
        EventKind::HarnessDisable,
        EventKind::HarnessEnable,
        EventKind::MoveAsideDisable,
        EventKind::MoveAsideRestore,
        EventKind::InvocationChange,
        EventKind::Fork,
        EventKind::UnlinkHarness,
        EventKind::RelinkHarness,
        EventKind::ExplodeSharedDir,
        EventKind::MaterializeThenDisable,
        EventKind::ReconcileRemoveStaleLink,
        EventKind::RepairRemoveLink,
        EventKind::RepairRelinkLink,
        EventKind::MakeIndependentCopy,
        EventKind::RepairSkillFrontmatter,
        EventKind::Restore,
    ];

    /// The literal stored in the `kind` column.
    pub const fn as_str(self) -> &'static str {
        match self {
            EventKind::Install => "install",
            EventKind::Remove => "remove",
            EventKind::Update => "update",
            EventKind::Park => "park",
            EventKind::Unpark => "unpark",
            EventKind::HarnessDisable => "harness_disable",
            EventKind::HarnessEnable => "harness_enable",
            EventKind::MoveAsideDisable => "move_aside_disable",
            EventKind::MoveAsideRestore => "move_aside_restore",
            EventKind::InvocationChange => "invocation_change",
            EventKind::Fork => "fork",
            EventKind::UnlinkHarness => "unlink_harness",
            EventKind::RelinkHarness => "relink_harness",
            EventKind::ExplodeSharedDir => "explode_shared_dir",
            EventKind::MaterializeThenDisable => "materialize_then_disable",
            EventKind::ReconcileRemoveStaleLink => "reconcile_remove_stale_link",
            EventKind::RepairRemoveLink => "repair_remove_link",
            EventKind::RepairRelinkLink => "repair_relink_link",
            EventKind::MakeIndependentCopy => "make_independent_copy",
            EventKind::RepairSkillFrontmatter => "repair_skill_frontmatter",
            EventKind::Restore => "restore",
        }
    }

    /// Parses a stored literal. `None` for literals this version does not know.
    pub fn parse(raw: &str) -> Option<Self> {
        EventKind::ALL.into_iter().find(|k| k.as_str() == raw)
    }
}

/// Row status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    /// Recorded, mutation in progress.
    Pending,
    /// Mutation durable.
    Done,
    /// Mutation failed; backup kept.
    Failed,
    /// Found `pending` at startup; the process died mid-mutation.
    Interrupted,
}

impl EventStatus {
    /// The literal stored in the `status` column.
    pub const fn as_str(self) -> &'static str {
        match self {
            EventStatus::Pending => "pending",
            EventStatus::Done => "done",
            EventStatus::Failed => "failed",
            EventStatus::Interrupted => "interrupted",
        }
    }

    /// Parses a stored literal.
    pub fn parse(raw: &str) -> Option<Self> {
        [
            EventStatus::Pending,
            EventStatus::Done,
            EventStatus::Failed,
            EventStatus::Interrupted,
        ]
        .into_iter()
        .find(|s| s.as_str() == raw)
    }
}

/// One row of the `events` table.
///
/// Invariant: field names match the SQLite columns. `payload` and `inverse`
/// stay opaque JSON so old rows load without a migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventRecord {
    /// ULID.
    pub id: EventId,
    /// UTC time.
    pub ts: DateTime<Utc>,
    /// Kind literal.
    pub kind: String,
    /// Skill name.
    pub skill: SkillName,
    /// Harness, when harness-scoped.
    pub harness: Option<AgentId>,
    /// `global` or `project`.
    pub scope: Option<String>,
    /// Project path, when project-scoped.
    pub project_path: Option<PathBuf>,
    /// Kind-specific forward data.
    pub payload: serde_json::Value,
    /// How to undo; `None` means not restorable.
    pub inverse: Option<serde_json::Value>,
    /// Relative backup directory.
    pub backup_dir: Option<String>,
    /// Status.
    pub status: EventStatus,
    /// Restore event that reverted this row.
    pub reverted_by: Option<EventId>,
    /// Whether this row may ever be restored, independent of whether it has
    /// an inverse. The desktop sets this `false` for a handful of kinds
    /// (`explode_shared_dir`'s intermediate row, an independent-copy record,
    /// and one `event_commands` case) where recreating bytes cannot recreate
    /// the ownership metadata that went with them - see
    /// `apps/desktop/src-tauri/src/skills/skill_materialize.rs`,
    /// `skill_independent_copy.rs`, and `event_commands.rs`. The core itself
    /// never writes `false` (nothing it can produce needs the escape hatch
    /// yet), but a desktop-authored row read by the core must still honor it.
    pub restorable: bool,
}

impl EventRecord {
    /// Typed kind, when known.
    pub fn kind(&self) -> Option<EventKind> {
        EventKind::parse(&self.kind)
    }

    /// Whether a restore may target this row.
    pub fn restore_capability(&self) -> RestoreCapability {
        match (
            &self.reverted_by,
            self.restorable,
            &self.inverse,
            self.kind(),
        ) {
            (Some(by), _, _, _) => RestoreCapability::Reverted { by: by.clone() },
            (None, false, _, _) => RestoreCapability::NoInverse,
            (None, true, None, _) => RestoreCapability::NoInverse,
            (None, true, Some(_), None) => RestoreCapability::UnknownKind,
            (None, true, Some(_), Some(_)) => RestoreCapability::Yes,
        }
    }

    /// Projects the row for display with drift left `Unchecked`; the caller
    /// sets [`EventDto::drift`] after comparing fingerprints.
    pub fn to_dto(&self) -> EventDto {
        EventDto {
            id: self.id.clone(),
            ts: self.ts,
            kind: self.kind.clone(),
            skill: self.skill.clone(),
            harness: self.harness.clone(),
            scope: self.scope.clone(),
            project_path: self.project_path.clone(),
            status: self.status.as_str().to_string(),
            restore: self.restore_capability(),
            drift: DriftState::Unchecked,
            backup_dir: self.backup_dir.clone(),
        }
    }
}

/// A row before it is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventDraft {
    /// Kind.
    pub kind: EventKind,
    /// Skill.
    pub skill: SkillName,
    /// Harness, when harness-scoped.
    pub harness: Option<AgentId>,
    /// `global` or `project`.
    pub scope: Option<String>,
    /// Project path, when project-scoped.
    pub project_path: Option<PathBuf>,
    /// Forward data.
    pub payload: serde_json::Value,
    /// Undo data with pre-mutation fingerprints.
    pub inverse: Option<serde_json::Value>,
    /// Relative backup directory from [`HistoryStore::backup_paths`].
    pub backup_dir: Option<String>,
}

/// Filter for [`HistoryStore::list`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventFilter {
    /// Restrict to one skill.
    pub skill: Option<SkillName>,
    /// Maximum rows.
    pub limit: u32,
    /// Only rows whose id sorts before this one (older), for paging.
    pub after: Option<EventId>,
}

/// One backed-up path.
///
/// Invariant: `fingerprint` is `None` exactly when `original` did not exist
/// at backup time (the host maps that case to and from the on-disk
/// manifest's `"absent"` literal). A restore reads `None` as "remove this
/// path" rather than "write these bytes".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BackupEntry {
    /// Original absolute path.
    pub original: PathBuf,
    /// Relative path inside the backup directory; empty when absent.
    pub relative: String,
    /// Fingerprint of the preserved bytes, or `None` for a path that did not
    /// exist when it was backed up.
    pub fingerprint: Option<Fingerprint>,
}

/// `manifest.json` inside a backup directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BackupManifest {
    /// Event the backup belongs to.
    pub event_id: EventId,
    /// Relative backup directory.
    pub backup_dir: String,
    /// Entries.
    pub entries: Vec<BackupEntry>,
}

/// What startup recovery did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecoveryReport {
    /// Rows flipped from `pending` to `interrupted`.
    pub interrupted: Vec<EventId>,
    /// Reserved for a future fingerprint-based recovery that finishes a row
    /// whose target already matches its proposed outcome. This PR mirrors
    /// the desktop's `reconcile_at_startup` exactly (a pure `pending` ->
    /// `interrupted` flip, no auto-completion), so this is always empty
    /// today; a caller must not depend on it being populated yet.
    pub completed: Vec<EventId>,
}

/// Flips every `pending` row to `interrupted`.
///
/// Ports the desktop's `reconcile_at_startup`
/// (`apps/desktop/src-tauri/src/skills/event_store.rs`) byte-for-byte in
/// behavior: a `pending` row only ever means the process died between
/// `record` and `finish`, so the safe, restorable state is `interrupted`,
/// never a guess at whether the write landed. Idempotent: `store.pending()`
/// only returns rows still in `pending` status, so a second call finds
/// nothing left to flip.
///
/// Runs only under the exclusive guard, before the first mutation of a
/// session, and never from a read operation. Reports through `sink` as
/// [`crate::ports::CoreNotice::Recovered`].
pub fn recover_interrupted(
    guard: &ExclusiveGuard,
    store: &mut dyn HistoryStore,
    fs: &dyn ScopeFs,
    sink: &dyn EventSink,
) -> Result<RecoveryReport, CoreError> {
    let _ = fs;
    let mut report = RecoveryReport::default();
    for row in store.pending()? {
        store.finish(guard, &row.id, EventStatus::Interrupted, None)?;
        report.interrupted.push(row.id);
    }
    if !report.interrupted.is_empty() {
        sink.notify(CoreNotice::Recovered {
            events: report.interrupted.clone(),
        });
    }
    Ok(report)
}

/// Content fingerprint for one path via [`ScopeFs`], tag+length framed
/// identically to the desktop's `fingerprint_path`/`hash_entry`
/// (`apps/desktop/src-tauri/src/skills/event_store.rs`) and to the host's
/// `hash_entry` (`crates/skill-studio-host/src/history.rs`), so a
/// fingerprint computed by any of the three matches for identical content.
/// Returns `None` for a path that does not exist.
///
/// Only files and symlinks are handled: PR5's only writer
/// (`apply_frontmatter_repair`) ever fingerprints `SKILL.md`, a regular
/// file, so a directory is out of scope here.
pub(crate) fn fingerprint_path(
    fs: &dyn ScopeFs,
    path: &Path,
) -> Result<Option<Fingerprint>, CoreError> {
    let meta = match fs.symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(CoreError::io(path, e)),
    };
    let buf = match meta.kind {
        FileKind::Symlink => {
            let target = fs.read_link(path).map_err(|e| CoreError::io(path, e))?;
            let mut buf = vec![b'L'];
            buf.extend_from_slice(target.to_string_lossy().as_bytes());
            buf
        }
        FileKind::Dir => {
            return Err(CoreError::new(
                ErrorCode::Unsupported,
                "fingerprint_path does not support directories",
            )
            .at(path));
        }
        FileKind::File | FileKind::Other => {
            let bytes = fs
                .read_capped(path, crate::ops::SKILL_MD_MAX_BYTES)
                .map_err(|e| CoreError::io(path, e))?;
            let mut buf = Vec::with_capacity(bytes.len() + 9);
            buf.push(b'F');
            buf.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            buf.extend_from_slice(&bytes);
            buf
        }
    };
    Ok(Some(Fingerprint::of_bytes(&buf)))
}

/// Builds the `restore_backup` inverse payload PR5's writer records:
/// byte-compatible with the desktop's `InverseOp::RestoreBackup`
/// (`apps/desktop/src-tauri/src/skills/event_store.rs`), whose `pre_fingerprint`/
/// `post_fingerprint` are plain strings using the literal `"absent"` for a
/// nonexistent path rather than `null`, so an event recorded by either
/// implementation restores under the other.
pub(crate) fn restore_backup_inverse(
    path: &Path,
    pre: Option<&Fingerprint>,
    post: Option<&Fingerprint>,
) -> serde_json::Value {
    fn as_str(f: Option<&Fingerprint>) -> String {
        f.map(|f| f.bare_hex().to_string())
            .unwrap_or_else(|| "absent".to_string())
    }
    serde_json::json!({
        "op": "restore_backup",
        "path": path,
        "pre_fingerprint": as_str(pre),
        "post_fingerprint": as_str(post),
    })
}

/// Reads a `restore_backup` inverse payload back into its path and
/// fingerprints. `None` for either fingerprint means `"absent"`. Returns
/// `None` when `inverse` is not a `restore_backup` op (an unrecognized op,
/// or a shape from another kind entirely).
pub(crate) fn parse_restore_backup_inverse(
    inverse: &serde_json::Value,
) -> Option<(PathBuf, Option<String>, Option<String>)> {
    let obj = inverse.as_object()?;
    if obj.get("op").and_then(|v| v.as_str()) != Some("restore_backup") {
        return None;
    }
    let path = PathBuf::from(obj.get("path")?.as_str()?);
    let pre = obj.get("pre_fingerprint").and_then(|v| v.as_str());
    let post = obj.get("post_fingerprint").and_then(|v| v.as_str());
    Some((
        path,
        pre.filter(|s| *s != "absent").map(str::to_string),
        post.filter(|s| *s != "absent").map(str::to_string),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_round_trips_through_its_literal() {
        for kind in EventKind::ALL {
            assert_eq!(EventKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(EventKind::parse("not_a_kind"), None);
    }

    #[test]
    fn unknown_kind_rows_still_load_and_are_not_restorable() {
        let row = EventRecord {
            id: EventId("01J".into()),
            ts: Utc::now(),
            kind: "future_kind".into(),
            skill: SkillName("x".into()),
            harness: None,
            scope: None,
            project_path: None,
            payload: serde_json::json!({}),
            inverse: Some(serde_json::json!({})),
            backup_dir: None,
            status: EventStatus::Done,
            reverted_by: None,
            restorable: true,
        };
        assert_eq!(row.restore_capability(), RestoreCapability::UnknownKind);
    }

    #[test]
    fn restorable_false_is_no_inverse_even_with_one_present_and_a_known_kind() {
        let row = EventRecord {
            id: EventId("01J".into()),
            ts: Utc::now(),
            kind: EventKind::Remove.as_str().to_string(),
            skill: SkillName("x".into()),
            harness: None,
            scope: None,
            project_path: None,
            payload: serde_json::json!({}),
            inverse: Some(serde_json::json!({})),
            backup_dir: None,
            status: EventStatus::Done,
            reverted_by: None,
            restorable: false,
        };
        assert_eq!(row.restore_capability(), RestoreCapability::NoInverse);
    }
}
