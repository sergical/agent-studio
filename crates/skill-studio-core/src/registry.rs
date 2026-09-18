//! The lease-guarded write path for a JSON document an adapter stores
//! directly under a scope home - on the desktop, `~/.agents/skill-studio.json`
//! (see `skill_fork_registry.rs`).
//!
//! The document's own shape (forks, trials, parked skills, ...) stays with
//! the adapter that owns it, since it carries adapter-specific types the
//! core does not know about. This module owns only the write path: take the
//! exclusive lease over the document's home, bump a monotonic write
//! counter, write atomically. [`RegistryDocument::write_version`] is
//! distinct from any schema-format version field a document also carries
//! (the desktop's `ForkRegistry.version`, for example, marks a schema
//! migration and is set explicitly by the code that performs it) - this one
//! exists only so a lease loser can tell its in-memory copy went stale, per
//! `docs/action-map/harnesses/shared-root.md`, "What is not there yet".

use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{CoreError, ErrorCode};
use crate::ports::{acquire_exclusive, confine, LeaseProvider, ScopeFs};
use crate::scope::{
    HistoryBinding, NormalizedScope, ProjectSelection, RuntimeScope, ScopeKind,
    DEFAULT_READ_TIMEOUT_MS, DEFAULT_WRITE_TIMEOUT_MS,
};

/// A JSON document stored directly under a scope home, written only through
/// [`write_registry_document`].
///
/// Invariant: `write_version` is set only by [`write_registry_document`];
/// a caller that sets it by hand loses the "did another writer race past me"
/// signal the field exists for.
pub trait RegistryDocument: Serialize + DeserializeOwned {
    /// The write counter as last read from disk (0 for a document that has
    /// never gone through a lease-guarded write, including one written
    /// before this field existed).
    fn write_version(&self) -> u64;
    /// Sets the write counter. Called only by [`write_registry_document`].
    fn set_write_version(&mut self, version: u64);
}

/// Builds the narrowest [`NormalizedScope`] that covers `home` and nothing
/// else - a registry write never touches a project, so it needs no
/// discovery port, just a home that exists. `pub(crate)` so other
/// home-only, lease-guarded JSON writers (e.g. [`crate::opencode_config`])
/// can share it instead of re-deriving the same scope.
pub(crate) fn home_only_scope(home: &Path, fs: &dyn ScopeFs) -> Result<NormalizedScope, CoreError> {
    let raw = RuntimeScope {
        kind: ScopeKind::Live,
        home_root: home.to_path_buf(),
        projects: ProjectSelection::Explicit { paths: Vec::new() },
        // Unused by this write path (no history store is opened here); a
        // registry write needs only the home root for its lease key.
        history_root: home.to_path_buf(),
        history_binding: HistoryBinding::Default,
        cache_root: None,
        data_root: None,
        opencode_config_root: None,
        read_timeout_ms: DEFAULT_READ_TIMEOUT_MS,
        write_timeout_ms: DEFAULT_WRITE_TIMEOUT_MS,
    };
    NormalizedScope::normalize(&raw, fs)
}

/// Takes the exclusive lease over `home`, bumps `document`'s write counter
/// by one, and writes it as pretty JSON to `path` (which must sit directly
/// under `home`), creating `path`'s parent directory first if needed.
pub fn write_registry_document<T: RegistryDocument>(
    leases: &dyn LeaseProvider,
    fs: &dyn ScopeFs,
    home: &Path,
    path: &Path,
    document: &mut T,
) -> Result<(), CoreError> {
    let scope = home_only_scope(home, fs)?;
    let guard = acquire_exclusive(leases, &scope)?;

    if let Some(parent) = path.parent() {
        let scoped_parent = confine(&scope, fs, parent)?;
        fs.create_dir_all(&guard, &scoped_parent)
            .map_err(|e| CoreError::io(parent, e))?;
    }

    document.set_write_version(document.write_version() + 1);
    let bytes = serde_json::to_vec_pretty(document).map_err(|e| {
        CoreError::new(
            ErrorCode::Io,
            format!("failed to serialize registry document: {e}"),
        )
        .at(path)
    })?;

    let scoped = confine(&scope, fs, path)?;
    fs.write_atomic(&guard, &scoped, &bytes)
        .map_err(|e| CoreError::io(path, e))
}
