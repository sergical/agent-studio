//! Wires the default set of host adapters into one [`Ports`].

use std::path::PathBuf;
use std::sync::Arc;

use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::ports::Ports;

use crate::clock::SystemClock;
use crate::discovery::TranscriptProjectDiscovery;
use crate::fs::RealFs;
use crate::history::{NoHistoryOpener, SqliteHistoryOpener};
use crate::ids::UlidIds;
use crate::lease::FileLease;
use crate::sink::NoopSink;
use crate::tools::PathToolLookup;

/// Builds the default `Ports` for a real desktop or CLI host: real
/// filesystem, real clock, monotonic ULIDs, file-lock leases rooted at
/// `lease_root`, no history store yet, and notices discarded.
///
/// `spawner`, `discovery`, and `tools` are left `None`; a caller that needs
/// them replaces the field on the returned `Ports`, or calls
/// [`default_ports_with_discovery`] for the common case of wanting both.
pub fn default_ports(lease_root: PathBuf, catalog: Arc<HarnessCatalog>) -> Ports {
    Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(SystemClock::new()),
        ids: Arc::new(UlidIds::new()),
        leases: Arc::new(FileLease::new(lease_root)),
        history: Arc::new(NoHistoryOpener),
        sink: Arc::new(NoopSink),
        spawner: None,
        discovery: None,
        tools: None,
        catalog,
    }
}

/// [`default_ports`], plus [`TranscriptProjectDiscovery`] for
/// `ProjectSelection::Discover` and [`PathToolLookup`] for `PATH` lookups.
///
/// [`ProjectSelection::Discover`]: skill_studio_core::scope::ProjectSelection::Discover
pub fn default_ports_with_discovery(lease_root: PathBuf, catalog: Arc<HarnessCatalog>) -> Ports {
    Ports {
        discovery: Some(Arc::new(TranscriptProjectDiscovery::new())),
        tools: Some(Arc::new(PathToolLookup::new())),
        ..default_ports(lease_root, catalog)
    }
}

/// [`default_ports`], with [`SqliteHistoryOpener`] bound to `db_path` in
/// place of [`NoHistoryOpener`], so mutations that need [`HistoryAccess::ReadWrite`]
/// work against a real event log.
///
/// [`HistoryAccess::ReadWrite`]: skill_studio_core::ports::HistoryAccess::ReadWrite
pub fn default_ports_with_history(
    lease_root: PathBuf,
    catalog: Arc<HarnessCatalog>,
    db_path: PathBuf,
) -> Ports {
    Ports {
        history: Arc::new(SqliteHistoryOpener::new(db_path)),
        ..default_ports(lease_root, catalog)
    }
}
