// unwrap/expect are fine in test code; production code must use ?
// or an explicit error.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! Real-world adapters for `skill-studio-core`.
//!
//! This crate holds no policy. Every type here implements one port trait
//! from `skill_studio_core::ports` over the actual operating system: real
//! files, the wall clock, fresh ids, advisory file locks, and a notice
//! sink. Its dependencies are `std`, `skill-studio-core`, and a small set of
//! parsing and platform crates (`ulid`, `chrono`, `serde_json`, `toml`,
//! `rusqlite`); no `tokio`, no `tauri`. Adapters that need an async runtime
//! or Tauri state wrap these types rather than reimplement them.
//!
//! [`default_ports`] wires the common case: real filesystem, real clock,
//! monotonic ULIDs, file-lock leases, no history store yet, and discarded
//! notices. A caller that needs a process spawner, project discovery, or a
//! `PATH` lookup sets those fields on the returned `Ports` itself.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod builder;
mod clock;
mod discovery;
mod fs;
mod harness_detect;
mod history;
mod ids;
mod lease;
mod opencode_db;
mod sink;
mod skill_uses;
mod tools;

pub use builder::{default_ports, default_ports_with_discovery, default_ports_with_history};
pub use clock::SystemClock;
pub use discovery::{discover_skill_projects, discovery_harnesses, HostProjectDiscovery};
pub use fs::RealFs;
pub use harness_detect::RealProcessSpawner;
pub use history::{hash_entry, NoHistoryOpener, SqliteHistoryOpener};
pub use ids::UlidIds;
pub use lease::FileLease;
#[cfg(feature = "error-reporting")]
pub use sink::HttpReportTransport;
pub use sink::{NoopSink, QueuedReportSink, ReportTransport, StderrSink, REPORT_ENDPOINT_ENV};
pub use skill_uses::{
    is_skill_use_change, skill_use_watch_paths, SkillInvocationIndex, SkillUseRefreshReport,
    SkillUseWatchPath,
};
pub use tools::PathToolLookup;
