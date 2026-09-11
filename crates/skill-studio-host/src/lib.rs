//! Real-world adapters for `skill-studio-core`.
//!
//! This crate holds no policy. Every type here implements one port trait
//! from `skill_studio_core::ports` over the actual operating system: real
//! files, the wall clock, fresh ids, advisory file locks, and a notice
//! sink. It depends on nothing but `std`, `skill-studio-core`, `ulid`,
//! `chrono`, and `serde_json`; no `tokio`, no `tauri`. Adapters that need an
//! async runtime or Tauri state wrap these types rather than reimplement
//! them.
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
mod history;
mod ids;
mod lease;
mod sink;
mod tools;

pub use builder::{default_ports, default_ports_with_discovery, default_ports_with_history};
pub use clock::SystemClock;
pub use discovery::TranscriptProjectDiscovery;
pub use fs::RealFs;
pub use history::{hash_entry, NoHistoryOpener, SqliteHistoryOpener};
pub use ids::UlidIds;
pub use lease::FileLease;
pub use sink::{NoopSink, StderrSink};
pub use tools::PathToolLookup;
