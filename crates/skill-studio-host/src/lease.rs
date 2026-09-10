//! [`LeaseProvider`] over advisory file locks.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use skill_studio_core::error::{CoreError, ErrorCode};
use skill_studio_core::identity::sha256_hex;
use skill_studio_core::ports::{LeaseHandle, LeaseKey, LeaseMode, LeaseProvider};

/// How long to sleep between two lock attempts while waiting for a lease.
const RETRY_INTERVAL: Duration = Duration::from_millis(20);

/// `LeaseProvider` backed by one advisory-locked file per canonical root,
/// under `lease_root`.
pub struct FileLease {
    lease_root: PathBuf,
}

impl FileLease {
    /// Builds a provider whose lock files live under `lease_root`.
    ///
    /// `lease_root` never sits inside a scope home or project; the adapter
    /// wiring is responsible for choosing an app-data directory.
    pub fn new(lease_root: PathBuf) -> Self {
        FileLease { lease_root }
    }

    fn lock_path(&self, key: &LeaseKey) -> PathBuf {
        let hex = sha256_hex(key.canonical_root.to_string_lossy().as_bytes());
        self.lease_root.join(format!("{hex}.lock"))
    }
}

/// Keys and open, locked files held for the lifetime of the handle.
///
/// Invariant: dropping the handle drops every `File`, which releases each
/// advisory lock; no explicit unlock call is needed.
struct FileLeaseHandle {
    keys: Vec<LeaseKey>,
    mode: LeaseMode,
    /// Kept only so the advisory locks release when this handle drops.
    _files: Vec<File>,
}

impl LeaseHandle for FileLeaseHandle {
    fn keys(&self) -> &[LeaseKey] {
        &self.keys
    }

    fn mode(&self) -> LeaseMode {
        self.mode
    }
}

fn try_lock(file: &File, mode: LeaseMode) -> Result<bool, io::Error> {
    let result = match mode {
        LeaseMode::Shared => file.try_lock_shared(),
        LeaseMode::Exclusive => file.try_lock(),
    };
    match result {
        Ok(()) => Ok(true),
        Err(fs::TryLockError::WouldBlock) => Ok(false),
        Err(fs::TryLockError::Error(e)) => Err(e),
    }
}

impl LeaseProvider for FileLease {
    fn acquire(
        &self,
        keys: &[LeaseKey],
        mode: LeaseMode,
        wait: Duration,
    ) -> Result<Box<dyn LeaseHandle>, CoreError> {
        let mut sorted_keys = keys.to_vec();
        sorted_keys.sort();

        fs::create_dir_all(&self.lease_root).map_err(|e| CoreError::io(&self.lease_root, e))?;

        let deadline = Instant::now() + wait;
        let mut files = Vec::with_capacity(sorted_keys.len());
        for key in &sorted_keys {
            let path = self.lock_path(key);
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
                .map_err(|e| CoreError::io(&path, e))?;
            loop {
                match try_lock(&file, mode) {
                    Ok(true) => break,
                    Ok(false) => {
                        if Instant::now() >= deadline {
                            return Err(CoreError::new(
                                ErrorCode::ScopeBusy,
                                format!(
                                    "another process holds the lease on {}",
                                    key.canonical_root.display()
                                ),
                            )
                            .at(&path));
                        }
                        std::thread::sleep(RETRY_INTERVAL);
                    }
                    Err(e) => return Err(CoreError::io(&path, e)),
                }
            }
            files.push(file);
        }

        Ok(Box::new(FileLeaseHandle {
            keys: sorted_keys,
            mode,
            _files: files,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> LeaseKey {
        LeaseKey {
            canonical_root: PathBuf::from(format!("/tmp/skill-studio-host-test/{name}")),
        }
    }

    #[test]
    fn two_shared_leases_over_the_same_key_both_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let lease = FileLease::new(dir.path().to_path_buf());
        let keys = [key("root-a")];
        let first = lease
            .acquire(&keys, LeaseMode::Shared, Duration::from_millis(100))
            .unwrap();
        let second = lease
            .acquire(&keys, LeaseMode::Shared, Duration::from_millis(100))
            .unwrap();
        assert_eq!(first.mode(), LeaseMode::Shared);
        assert_eq!(second.mode(), LeaseMode::Shared);
    }

    #[test]
    fn a_shared_lease_blocks_an_exclusive_lease_until_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let lease_root = dir.path().to_path_buf();
        let keys = vec![key("root-b")];

        let lease = FileLease::new(lease_root.clone());
        let shared = lease
            .acquire(&keys, LeaseMode::Shared, Duration::from_millis(100))
            .unwrap();

        let keys_for_writer = keys.clone();
        let writer = std::thread::spawn(move || {
            let writer_lease = FileLease::new(lease_root);
            writer_lease.acquire(
                &keys_for_writer,
                LeaseMode::Exclusive,
                Duration::from_millis(50),
            )
        });
        let busy = writer.join().unwrap();
        assert!(matches!(
            busy,
            Err(e) if e.code == ErrorCode::ScopeBusy
        ));

        drop(shared);

        let lease = FileLease::new(dir.path().to_path_buf());
        let exclusive = lease
            .acquire(&keys, LeaseMode::Exclusive, Duration::from_millis(500))
            .unwrap();
        assert_eq!(exclusive.mode(), LeaseMode::Exclusive);
    }

    #[test]
    fn exclusive_lease_releases_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let keys = vec![key("root-c")];
        let lease = FileLease::new(dir.path().to_path_buf());

        let first = lease
            .acquire(&keys, LeaseMode::Exclusive, Duration::from_millis(100))
            .unwrap();
        drop(first);

        let second = lease
            .acquire(&keys, LeaseMode::Exclusive, Duration::from_millis(100))
            .unwrap();
        assert_eq!(second.keys(), keys.as_slice());
    }
}
