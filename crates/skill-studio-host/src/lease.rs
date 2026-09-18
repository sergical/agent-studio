//! [`LeaseProvider`] over advisory file locks.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use skill_studio_core::error::{CoreError, ErrorCode, LeaseBusy};
use skill_studio_core::identity::sha256_hex;
use skill_studio_core::ports::{LeaseHandle, LeaseKey, LeaseMode, LeaseProvider};

/// How long to sleep between two lock attempts while waiting for a lease.
const RETRY_INTERVAL: Duration = Duration::from_millis(20);

/// Upper bound on how long a takeover of a lease whose recorded holder is
/// gone may take. The OS releases the advisory lock as part of tearing down
/// the dead process's file descriptors; this window only bridges the small
/// gap between that teardown and our next lock attempt, independent of the
/// caller's own `wait` budget (which may be zero).
const STALE_TAKEOVER_TIMEOUT: Duration = Duration::from_millis(500);
const STALE_RETRY_INTERVAL: Duration = Duration::from_millis(5);

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

/// Records this process as the holder: pid and the wall-clock time of
/// acquisition, so a later `Busy` error can name both. Written only for
/// `LeaseMode::Exclusive`; a shared lease has no single holder to name.
///
/// Best effort: a write failure here only means a later reader sees no
/// holder info, never a wrong one, so the caller ignores its result.
fn write_holder(file: &File) -> io::Result<()> {
    let pid = std::process::id();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut file = file;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    write!(file, "{pid}|{now_ms}")?;
    file.sync_all()
}

/// Reads the holder an earlier `write_holder` recorded, if any and if
/// parseable. Returns the pid and how long ago it acquired the lease.
fn read_holder(path: &Path) -> Option<(u32, Duration)> {
    let content = fs::read_to_string(path).ok()?;
    let (pid_str, ts_str) = content.trim().split_once('|')?;
    let pid: u32 = pid_str.parse().ok()?;
    let recorded_ms: u128 = ts_str.parse().ok()?;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    let age_ms = now_ms.saturating_sub(recorded_ms);
    Some((
        pid,
        Duration::from_millis(u64::try_from(age_ms).unwrap_or(u64::MAX)),
    ))
}

/// Whether `pid` still names a live process. `sysinfo`, not `libc`'s
/// `kill(pid, 0)`, because this crate forbids unsafe code; it refreshes only
/// the one process, so the check stays cheap.
fn pid_alive(pid: u32) -> bool {
    let mut system = sysinfo::System::new();
    system.refresh_processes(
        sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
        true,
    );
    system.process(sysinfo::Pid::from_u32(pid)).is_some()
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
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|e| CoreError::io(&path, e))?;
            let stale_deadline = Instant::now() + STALE_TAKEOVER_TIMEOUT;
            let mut attempted = false;
            loop {
                match try_lock(&file, mode) {
                    Ok(true) => {
                        if mode == LeaseMode::Exclusive {
                            let _ = write_holder(&file);
                        }
                        break;
                    }
                    Ok(false) => {
                        let holder = read_holder(&path);
                        let holder_alive = holder.map(|(pid, _)| pid_alive(pid)).unwrap_or(true);
                        if !holder_alive && Instant::now() < stale_deadline {
                            // The recorded holder is dead; the OS releases
                            // its advisory lock as part of exiting, usually
                            // before we even observe `WouldBlock`. Bridge
                            // the rare remaining gap instead of reporting a
                            // holder that is already gone.
                            std::thread::sleep(STALE_RETRY_INTERVAL);
                            continue;
                        }
                        // A zero (or already-elapsed) `wait` collapses
                        // `deadline` to "now", which would otherwise turn a
                        // single spurious `WouldBlock` - the OS can report
                        // one for a moment right after another fd on this
                        // process closes and releases the same lock under
                        // heavy concurrent load - into a false "busy". Always
                        // re-check once before trusting the first read.
                        if Instant::now() >= deadline && attempted {
                            let mut err = CoreError::new(
                                ErrorCode::ScopeBusy,
                                format!(
                                    "another process holds the lease on {}",
                                    key.canonical_root.display()
                                ),
                            )
                            .at(&path);
                            if let Some((pid, age)) = holder {
                                err = err.with_busy(LeaseBusy { pid, age });
                            }
                            return Err(err);
                        }
                        attempted = true;
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
