//! Persisted event records shared by adapters. Deserialization does not grant
//! authority to access any recorded path or execute an inverse operation.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

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
    pub restorable: bool,
}

#[derive(Clone, Copy)]
pub enum EventStatus {
    Done,
    Failed,
}

impl EventStatus {
    pub fn as_str(&self) -> &'static str {
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
    pub restorable: bool,
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
    pub fn destination(&self) -> &Path {
        match self {
            InverseOp::RecreateSymlink { link, .. } => link,
            InverseOp::RemoveSymlink { link, .. } => link,
            InverseOp::MoveBack { to, .. } => to,
            InverseOp::RestoreBackup { path, .. } => path,
            InverseOp::UndistributeFromShared { shared_dir, .. } => shared_dir,
        }
    }

    pub fn post_fingerprint(&self) -> Option<&String> {
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
    use serde_json::json;

    #[test]
    fn persisted_inverse_variants_preserve_the_existing_wire_format() {
        let cases = [
            (
                json!({"op":"recreate_symlink","link":"/fixture/link","target":"../shared/sample","pre_fingerprint":"before","post_fingerprint":"after"}),
                "/fixture/link",
            ),
            (
                json!({"op":"remove_symlink","link":"/fixture/link","pre_fingerprint":"absent","post_fingerprint":"after"}),
                "/fixture/link",
            ),
            (
                json!({"op":"move_back","from":"/fixture/parked","to":"/fixture/active","pre_fingerprint":"before","post_fingerprint":"after"}),
                "/fixture/active",
            ),
            (
                json!({"op":"restore_backup","path":"/fixture/SKILL.md","pre_fingerprint":"before","post_fingerprint":"after"}),
                "/fixture/SKILL.md",
            ),
            (
                json!({"op":"undistribute_from_shared","shared_dir":"/fixture/shared","copies":["/fixture/copy"],"copy_fingerprints":["copy-hash"],"symlinks":[["/fixture/link","../shared"]],"pre_fingerprint":"before","post_fingerprint":"after"}),
                "/fixture/shared",
            ),
        ];
        for (value, destination) in cases {
            let inverse: InverseOp = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(inverse.destination(), Path::new(destination));
            assert_eq!(
                inverse.post_fingerprint().map(String::as_str),
                Some("after")
            );
            assert_eq!(serde_json::to_value(inverse).unwrap(), value);
        }
        let pending: InverseOp = serde_json::from_value(
            json!({"op":"restore_backup","path":"/fixture/SKILL.md","pre_fingerprint":"before"}),
        )
        .unwrap();
        assert!(pending.post_fingerprint().is_none());
    }

    #[test]
    fn event_rows_and_backup_manifests_preserve_null_and_absent_records() {
        let value = json!({"id":"fixture-id","ts":"before","kind":"repair_skill_frontmatter","skill":"sample","harness":null,"scope":"global","project_path":null,"payload":{"mode":"apply-fix"},"inverse":null,"backup_dir":null,"status":"interrupted","reverted_by":null,"restorable":false});
        let row: EventRow = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(row).unwrap(), value);
        let value = json!({"entries":{"/fixture/missing":{"relative_path":"","fingerprint":"absent"},"/fixture/SKILL.md":{"relative_path":"0-SKILL.md","fingerprint":"before"}}});
        let manifest: BackupManifest = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(manifest).unwrap(), value);
        assert_eq!(EventStatus::Done.as_str(), "done");
        assert_eq!(EventStatus::Failed.as_str(), "failed");
    }
}
