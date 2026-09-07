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

/// Capability for reading and replacing SKILL.md while the process-wide
/// write transaction is held. External editors do not participate in it.
pub(crate) struct SkillMdWriteTransaction {
    _guard: MutexGuard<'static, ()>,
}

impl SkillMdWriteTransaction {
    /// Reads text that will be checked or rewritten before this transaction
    /// replaces the same SKILL.md.
    pub(crate) fn read_to_string(&self, path: &Path) -> Result<String, String> {
        fs::read_to_string(path).map_err(|error| {
            format!(
                "Failed to open {} during SKILL.md write transaction: {error}",
                path.display()
            )
        })
    }

    /// Reads bytes that will be checked before this transaction replaces the
    /// same SKILL.md.
    pub(crate) fn read(&self, path: &Path) -> Result<Vec<u8>, String> {
        fs::read(path).map_err(|error| {
            format!(
                "Failed to read {} during SKILL.md write transaction: {error}",
                path.display()
            )
        })
    }

    /// Atomically replaces SKILL.md bytes without reacquiring the transaction
    /// lock. Callers can safely use it after a read or drift check on `self`.
    pub(crate) fn replace_bytes(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        atomic_replace_skill_md_unlocked(path, bytes)
    }

    /// Atomically replaces SKILL.md text without reacquiring the transaction
    /// lock. Callers can safely use it after a read or drift check on `self`.
    pub(crate) fn replace_text(&self, path: &Path, content: &str) -> Result<(), String> {
        self.replace_bytes(path, content.as_bytes())
    }
}

/// Starts one app-managed SKILL.md write transaction. Keep the returned
/// capability alive across every read or drift check and its replacement.
pub(crate) fn begin_skill_md_write_transaction() -> Result<SkillMdWriteTransaction, String> {
    let guard = SKILL_MD_WRITE_LOCK.lock().map_err(|_| {
        "SKILL.md write transaction lock is poisoned. Restart Skill Studio before editing SKILL.md again."
            .to_string()
    })?;
    Ok(SkillMdWriteTransaction { _guard: guard })
}

/// Atomically replaces SKILL.md in one app-managed write transaction.
pub(crate) fn write_skill_md(path: &Path, content: &str) -> Result<(), String> {
    begin_skill_md_write_transaction()?.replace_text(path, content)
}

/// Atomically replaces SKILL.md bytes in one app-managed write transaction.
#[cfg(test)]
pub(crate) fn write_skill_md_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    begin_skill_md_write_transaction()?.replace_bytes(path, bytes)
}

/// Compares and atomically replaces SKILL.md in one app-managed transaction.
pub(crate) fn write_skill_md_compare_and_swap(
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
    let current = transaction.read_to_string(path)?;
    if current != expected_content {
        return Err(
            "SKILL.md changed on disk since it was loaded. Reload the file and run the audit again."
                .to_string(),
        );
    }
    before_replace();
    transaction.replace_text(path, content)
}

#[cfg(test)]
pub(crate) fn write_skill_md_compare_and_swap_with(
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

#[cfg(test)]
pub(crate) fn skill_md_write_transaction_is_held() -> bool {
    SKILL_MD_WRITE_LOCK.try_lock().is_err()
}

fn atomic_replace_skill_md_unlocked(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Failed to resolve parent directory of {}", path.display()))?;
    let permissions = fs::metadata(path)
        .map_err(|error| format!("Failed to stat {}: {error}", path.display()))?
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

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| format!("Failed to create {}: {error}", temp.display()))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("Failed to write {}: {error}", temp.display()))?;
        fs::set_permissions(&temp, permissions)
            .map_err(|error| format!("Failed to preserve SKILL.md permissions: {error}"))?;
        fs::rename(&temp, path)
            .map_err(|error| format!("Failed to save {}: {error}", path.display()))?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("Failed to sync skill directory: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
