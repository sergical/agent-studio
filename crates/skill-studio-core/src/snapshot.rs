//! Snapshot revisions shared by the desktop and the MCP subscription.

use std::sync::{Arc, RwLock};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Monotonic snapshot revision.
///
/// Invariant: a subscriber accepts a snapshot only when its revision is
/// greater than the one it holds. Revisions restart at `0` per process.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct Revision(pub u64);

/// The current snapshot and its revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Versioned<T> {
    /// Revision of `value`.
    pub revision: Revision,
    /// The snapshot.
    pub value: T,
}

/// A cell that holds the latest snapshot and bumps the revision on publish.
///
/// Invariant: `publish` assigns `previous + 1` inside the write lock, so two
/// publishers never produce the same revision.
pub struct SnapshotCell<T> {
    inner: RwLock<Option<Arc<Versioned<T>>>>,
}

impl<T> Default for SnapshotCell<T> {
    fn default() -> Self {
        SnapshotCell {
            inner: RwLock::new(None),
        }
    }
}

impl<T> SnapshotCell<T> {
    /// An empty cell.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores `value` and returns its revision.
    pub fn publish(&self, value: T) -> Revision {
        let mut slot = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let next = Revision(slot.as_ref().map(|v| v.revision.0 + 1).unwrap_or(1));
        *slot = Some(Arc::new(Versioned {
            revision: next,
            value,
        }));
        next
    }

    /// The current snapshot, if any was published.
    pub fn current(&self) -> Option<Arc<Versioned<T>>> {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The current snapshot only when it is newer than `seen`.
    pub fn newer_than(&self, seen: Revision) -> Option<Arc<Versioned<T>>> {
        self.current().filter(|v| v.revision > seen)
    }
}

/// Publishes snapshots to subscribers (desktop emit, MCP notification).
pub trait SnapshotPublisher<T>: Send + Sync {
    /// Publishes `value` and returns the revision assigned to it.
    fn publish(&self, value: T) -> Revision;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_than_filters_stale_readers() {
        let cell = SnapshotCell::new();
        assert!(cell.newer_than(Revision(0)).is_none());
        let r1 = cell.publish("a");
        assert_eq!(r1, Revision(1));
        assert!(cell.newer_than(r1).is_none());
        let r2 = cell.publish("b");
        assert_eq!(cell.newer_than(r1).unwrap().revision, r2);
    }
}
