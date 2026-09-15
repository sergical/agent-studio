//! Compatibility document replacement with process and directory coordination.
//! Ambient paths and surrounding operation effects still require migration to
//! the scoped mutation service.

// ============================================================================
// Skills Module - coordinated SKILL.md writes
// Serializes editor-style SKILL.md read-modify-write operations in this
// process and provides their atomic file-replacement primitive.
// ============================================================================

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

/// Counter appended to the atomic-write temp filename, on top of the pid and
/// a timestamp, so two saves in the same nanosecond still get distinct files.
static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

static SKILL_MD_WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug)]
pub enum DocumentWriteFailure {
    BeforeReplace(String),
    AfterReplace(String),
}

impl std::fmt::Display for DocumentWriteFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeforeReplace(message) | Self::AfterReplace(message) => {
                formatter.write_str(message)
            }
        }
    }
}
impl std::error::Error for DocumentWriteFailure {}

/// Capability for reading and replacing SKILL.md while the process-wide
/// write transaction is held. External editors do not participate in it.
pub struct SkillMdWriteTransaction {
    _guard: MutexGuard<'static, ()>,
    #[cfg(unix)]
    coordination: Option<(
        std::path::PathBuf,
        crate::skill_coordination::CoordinationGuard,
    )>,
}

impl SkillMdWriteTransaction {
    fn validate_document(&self, path: &Path) -> Result<(), String> {
        #[cfg(unix)]
        if let Some((document, guard)) = &self.coordination {
            if path != document {
                return Err("Document differs from the coordinated transaction target".into());
            }
            guard.revalidate().map_err(|error| error.to_string())?;
        }
        #[cfg(not(unix))]
        let _ = path;
        Ok(())
    }

    /// Reads text that will be checked or rewritten before this transaction
    /// replaces the same SKILL.md.
    pub fn read_to_string(&self, path: &Path) -> Result<String, String> {
        self.validate_document(path)?;
        fs::read_to_string(path).map_err(|error| {
            format!(
                "Failed to open {} during SKILL.md write transaction: {error}",
                path.display()
            )
        })
    }

    /// Reads bytes that will be checked before this transaction replaces the
    /// same SKILL.md.
    pub fn read(&self, path: &Path) -> Result<Vec<u8>, String> {
        self.validate_document(path)?;
        fs::read(path).map_err(|error| {
            format!(
                "Failed to read {} during SKILL.md write transaction: {error}",
                path.display()
            )
        })
    }

    /// Replaces bytes under the process transaction. Directory coordination
    /// covers publication, not an earlier read on this transaction.
    pub fn replace_bytes(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        self.replace_bytes_tracked(path, bytes)
            .map_err(|error| error.to_string())
    }

    /// Reports whether rename happened, even if directory durability failed.
    pub fn replace_bytes_tracked(
        &self,
        path: &Path,
        bytes: &[u8],
    ) -> Result<(), DocumentWriteFailure> {
        self.validate_document(path)
            .map_err(DocumentWriteFailure::BeforeReplace)?;
        #[cfg(unix)]
        let _coordination = if self.coordination.is_none() {
            Some(coordinate_document_entry(path).map_err(DocumentWriteFailure::BeforeReplace)?)
        } else {
            None
        };
        atomic_replace_skill_md_unlocked(path, bytes)
    }

    /// Replaces text through the same coordinated publication path.
    pub fn replace_text(&self, path: &Path, content: &str) -> Result<(), String> {
        self.replace_bytes(path, content.as_bytes())
    }
}

/// Starts one app-managed SKILL.md write transaction. Keep the returned
/// capability alive across every read or drift check and its replacement.
pub fn begin_skill_md_write_transaction() -> Result<SkillMdWriteTransaction, String> {
    let guard = SKILL_MD_WRITE_LOCK.lock().map_err(|_| {
        "SKILL.md write transaction lock is poisoned. Restart Skill Studio before editing SKILL.md again."
            .to_string()
    })?;
    Ok(SkillMdWriteTransaction {
        _guard: guard,
        #[cfg(unix)]
        coordination: None,
    })
}

/// Retains explicit entry locks across document reads and related publications.
/// The caller still owns path authority and recovery for the complete operation.
#[cfg(unix)]
pub fn begin_skill_md_write_transaction_with_entries(
    document: &Path,
    additional_entries: &[std::path::PathBuf],
    timeout: Option<std::time::Duration>,
) -> Result<SkillMdWriteTransaction, String> {
    use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
    let mut transaction = begin_skill_md_write_transaction()?;
    let effects = std::iter::once(document.to_path_buf())
        .chain(additional_entries.iter().cloned())
        .map(|path| DirectoryEffect::entry(path, CoordinationMode::Exclusive))
        .collect();
    let guard = CoordinationPlan::new(effects, timeout)
        .and_then(|plan| plan.acquire())
        .map_err(|error| error.to_string())?;
    transaction.coordination = Some((document.to_path_buf(), guard));
    Ok(transaction)
}

/// Atomically replaces SKILL.md in one app-managed write transaction.
pub fn write_skill_md(path: &Path, content: &str) -> Result<(), String> {
    begin_skill_md_write_transaction()?.replace_text(path, content)
}

/// Atomically replaces SKILL.md bytes in one app-managed write transaction.
#[cfg(any(test, feature = "test-support"))]
pub fn write_skill_md_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    begin_skill_md_write_transaction()?.replace_bytes(path, bytes)
}

/// Compares and atomically replaces SKILL.md in one app-managed transaction.
pub fn write_skill_md_compare_and_swap(
    path: &Path,
    expected_content: &str,
    content: &str,
) -> Result<(), String> {
    let transaction = begin_skill_md_write_transaction()?;
    compare_and_replace_skill_md(&transaction, path, expected_content, content, || {})
}

fn compare_and_replace_skill_md(
    transaction: &SkillMdWriteTransaction,
    path: &Path,
    expected_content: &str,
    content: &str,
    before_replace: impl FnOnce(),
) -> Result<(), String> {
    #[cfg(unix)]
    let _coordination = coordinate_document_entry(path)?;
    let current = transaction.read_to_string(path)?;
    if current != expected_content {
        return Err(
            "SKILL.md changed on disk since it was loaded. Reload the file and run the audit again."
                .to_string(),
        );
    }
    before_replace();
    atomic_replace_skill_md_unlocked(path, content.as_bytes()).map_err(|error| error.to_string())
}

#[cfg(any(test, feature = "test-support"))]
pub fn write_skill_md_compare_and_swap_with(
    path: &Path,
    expected_content: &str,
    content: &str,
    before_replace: impl FnOnce(),
) -> Result<(), String> {
    let transaction = begin_skill_md_write_transaction()?;
    compare_and_replace_skill_md(
        &transaction,
        path,
        expected_content,
        content,
        before_replace,
    )
}

#[cfg(any(test, feature = "test-support"))]
pub fn skill_md_write_transaction_is_held() -> bool {
    SKILL_MD_WRITE_LOCK.try_lock().is_err()
}

#[cfg(unix)]
fn coordinate_document_entry(
    path: &Path,
) -> Result<crate::skill_coordination::CoordinationGuard, String> {
    use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
    CoordinationPlan::new(
        vec![DirectoryEffect::entry(path, CoordinationMode::Exclusive)],
        None,
    )
    .and_then(|plan| plan.acquire())
    .map_err(|error| error.to_string())
}

fn atomic_replace_skill_md_unlocked(path: &Path, bytes: &[u8]) -> Result<(), DocumentWriteFailure> {
    atomic_replace_with_sync(path, bytes, |parent| {
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("Failed to sync skill directory: {error}"))
    })
}

fn atomic_replace_with_sync(
    path: &Path,
    bytes: &[u8],
    sync_directory: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), DocumentWriteFailure> {
    let parent = path.parent().ok_or_else(|| {
        DocumentWriteFailure::BeforeReplace(format!(
            "Failed to resolve parent directory of {}",
            path.display()
        ))
    })?;
    let permissions = fs::metadata(path)
        .map_err(|error| {
            DocumentWriteFailure::BeforeReplace(format!(
                "Failed to stat {}: {error}",
                path.display()
            ))
        })?
        .permissions();
    let counter = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp = parent.join(format!(
        ".SKILL.md.tmp-{}-{counter}-{nanos}",
        std::process::id()
    ));

    let mut replaced = false;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| format!("Failed to create {}: {error}", temp.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("Failed to write {}: {error}", temp.display()))?;
        file.set_permissions(permissions)
            .map_err(|error| format!("Failed to preserve SKILL.md permissions: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Failed to sync {}: {error}", temp.display()))?;
        fs::rename(&temp, path)
            .map_err(|error| format!("Failed to save {}: {error}", path.display()))?;
        replaced = true;
        sync_directory(parent)
    })();
    if result.is_err() && !replaced {
        let _ = fs::remove_file(temp);
    }
    result.map_err(|message| {
        if replaced {
            DocumentWriteFailure::AfterReplace(message)
        } else {
            DocumentWriteFailure::BeforeReplace(message)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn coordinated_transaction_refuses_an_unplanned_document() {
        let temp = tempfile::tempdir().unwrap();
        let document = temp.path().join("SKILL.md");
        let other = temp.path().join("other.md");
        fs::write(&document, "original").unwrap();
        fs::write(&other, "keep").unwrap();
        let transaction =
            begin_skill_md_write_transaction_with_entries(&document, &[], None).unwrap();
        assert!(transaction.read(&other).is_err());
        assert!(transaction.replace_text(&other, "changed").is_err());
        transaction.replace_text(&document, "first").unwrap();
        transaction.replace_text(&document, "second").unwrap();
        assert_eq!(transaction.read_to_string(&document).unwrap(), "second");
        assert_eq!(fs::read_to_string(&other).unwrap(), "keep");
    }

    #[cfg(unix)]
    #[test]
    fn compare_and_save_excludes_other_processes_until_publication_finishes() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        fs::write(&path, "original").unwrap();
        write_skill_md_compare_and_swap_with(&path, "original", "replacement", || {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "skill_document_write::tests::document_coordination_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("SKILL_STUDIO_DOCUMENT_LOCK_TEST_PATH", &path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        })
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "replacement");
        drop(
            CoordinationPlan::new(
                vec![DirectoryEffect::entry(&path, CoordinationMode::Exclusive)],
                Some(std::time::Duration::from_secs(2)),
            )
            .unwrap()
            .acquire()
            .unwrap(),
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "run by document coordination parent fixture"]
    fn document_coordination_child() {
        use crate::skill_coordination::{
            CoordinationFailure, CoordinationMode, CoordinationPlan, DirectoryEffect,
        };
        let path = std::path::PathBuf::from(
            std::env::var_os("SKILL_STUDIO_DOCUMENT_LOCK_TEST_PATH").unwrap(),
        );
        let result = CoordinationPlan::new(
            vec![DirectoryEffect::entry(path, CoordinationMode::Exclusive)],
            Some(std::time::Duration::from_millis(50)),
        )
        .unwrap()
        .acquire();
        assert!(matches!(result, Err(CoordinationFailure::Busy)));
    }

    #[test]
    fn write_failure_distinguishes_rename_from_directory_sync() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        let transaction = begin_skill_md_write_transaction().unwrap();
        assert!(matches!(
            transaction.replace_bytes_tracked(&path, b"new"),
            Err(DocumentWriteFailure::BeforeReplace(_))
        ));
        fs::write(&path, b"original").unwrap();
        let result = atomic_replace_with_sync(&path, b"replacement", |_| {
            Err("injected directory sync failure".into())
        });
        assert!(matches!(result, Err(DocumentWriteFailure::AfterReplace(_))));
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn shared_transaction_checks_drift_and_preserves_replacement_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        fs::write(&path, b"original\r\n").unwrap();
        assert!(write_skill_md_compare_and_swap(&path, "stale", "wrong").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original\r\n");
        let transaction = begin_skill_md_write_transaction().unwrap();
        assert!(skill_md_write_transaction_is_held());
        assert_eq!(transaction.read(&path).unwrap(), b"original\r\n");
        transaction
            .replace_bytes(&path, b"replacement\r\n")
            .unwrap();
        assert_eq!(
            transaction.read_to_string(&path).unwrap(),
            "replacement\r\n"
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn shared_replacement_preserves_permissions_and_refuses_missing_files() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        assert!(write_skill_md(&path, "new").is_err());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
        fs::write(&path, "before").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        write_skill_md_compare_and_swap(&path, "before", "after").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "after");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
