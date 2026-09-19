// ============================================================================
// Skill Studio - write_lease
// Replaces the desktop's old process-wide mutation mutex (fork, park,
// unpark, harness-disable, add, and frontmatter-repair writes all used to
// serialize on it). `WriteLease` is a `FileLease` keyed to the root
// the write touches (`home` for every one of today's call sites), so a
// concurrent CLI or MCP write on the same root serializes with the desktop
// too - a global in-process mutex never could.
// ============================================================================

use std::path::Path;
use std::time::Duration;

use skill_studio_core::ports::{ExclusiveGuard, LeaseKey, LeaseMode, LeaseProvider};
use skill_studio_host::FileLease;

use super::core_runtime;

/// Holds the write lease until dropped, releasing it.
pub struct WriteLeaseGuard(ExclusiveGuard);

impl WriteLeaseGuard {
    /// The proof-of-lease token for a nested write to the same root - e.g.
    /// `write_fork_registry_locked`, which would otherwise need to take a
    /// second, conflicting lease. Advisory locks don't nest within one
    /// process, so anything writing the registry while this guard is held
    /// must go through it instead of acquiring its own lease.
    pub fn as_exclusive_guard(&self) -> &ExclusiveGuard {
        &self.0
    }
}

/// One lease per root, backed by a `FileLease` rooted at the same
/// `<data_root>/leases` directory `build_runtime_write` uses for park and
/// unpark.
pub struct WriteLease {
    lease: FileLease,
}

impl Default for WriteLease {
    fn default() -> Self {
        WriteLease {
            lease: FileLease::new(core_runtime::data_root().join("leases")),
        }
    }
}

impl WriteLease {
    /// Builds a lease provider rooted at `lease_root` instead of the real
    /// machine's data directory, so a test can hold a lease without
    /// touching it.
    #[cfg(test)]
    pub fn with_lease_root(lease_root: std::path::PathBuf) -> Self {
        WriteLease {
            lease: FileLease::new(lease_root),
        }
    }

    /// Acquires an exclusive, non-blocking lease on `root`. `Err` mirrors
    /// the old mutation mutex's message shape when another writer already
    /// holds it, naming its pid and how long it has held the lease when the
    /// lease reports one.
    pub fn try_acquire(&self, root: &Path) -> Result<WriteLeaseGuard, String> {
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let key = LeaseKey { canonical_root };
        self.lease
            .acquire(&[key], LeaseMode::Exclusive, Duration::ZERO)
            .map(|handle| WriteLeaseGuard(ExclusiveGuard::from_handle(handle)))
            .map_err(|e| match e.busy {
                Some(busy) => format!(
                    "Another write is in progress (pid {}, held for {:?})",
                    busy.pid, busy.age
                ),
                None => "Another write operation is in progress".to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_acquire_on_the_same_root_is_refused_until_the_first_drops() {
        let dir = tempfile::tempdir().unwrap();
        let lease_root = dir.path().join("leases");
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();

        let lease = WriteLease::with_lease_root(lease_root);
        let first = lease.try_acquire(&root).unwrap();
        let second = lease.try_acquire(&root);
        assert!(second.err().unwrap().contains("Another write"));
        drop(first);
        assert!(lease.try_acquire(&root).is_ok());
    }

    /// A command holds `WriteLease` on `home` for its whole run (fork, add,
    /// pack, harness) and, inside that, writes the fork registry.
    /// Advisory locks don't nest within one process, so a registry write
    /// that takes its own second exclusive lease over the same root reports
    /// the caller's own lease as busy instead of writing - see
    /// `write_fork_registry_locked`.
    #[test]
    fn a_command_holding_the_root_lease_can_write_the_fork_registry_or_names_the_self_deadlock() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        let write_lease = WriteLease::default();
        let guard = write_lease.try_acquire(&home).unwrap();

        let registry = super::super::skill_fork_registry::ForkRegistry::default();
        let result =
            super::super::skill_fork_registry::write_fork_registry_locked(&guard, &home, &registry);
        assert!(
            result.is_ok(),
            "writing the fork registry while the caller holds the root's write lease must \
             not need a second lease on the same root: {result:?}"
        );
    }

    #[test]
    fn two_different_roots_never_block_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let lease_root = dir.path().join("leases");
        let root_a = dir.path().join("a");
        let root_b = dir.path().join("b");
        std::fs::create_dir_all(&root_a).unwrap();
        std::fs::create_dir_all(&root_b).unwrap();

        let lease = WriteLease::with_lease_root(lease_root);
        let _a = lease.try_acquire(&root_a).unwrap();
        assert!(lease.try_acquire(&root_b).is_ok());
    }
}
