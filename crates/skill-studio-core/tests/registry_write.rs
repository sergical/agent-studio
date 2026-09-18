//! Real-disk integration tests for [`skill_studio_core::registry`]'s
//! lease-guarded write path.
//!
//! Uses `skill-studio-host`'s real `RealFs`/`FileLease` rather than the
//! in-memory `FixtureFs`, since `FixtureFs::write_atomic` refuses every
//! call (see its doc comment in `testing.rs`) - this module's whole job is
//! the write.

use std::path::Path;

use serde::{Deserialize, Serialize};
use skill_studio_core::registry::{write_registry_document, RegistryDocument};
use skill_studio_host::{FileLease, RealFs};

/// A minimal registry document: one known field plus a flatten catch-all,
/// standing in for the desktop's `ForkRegistry` without pulling its
/// adapter-specific types into a core test.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct TestDoc {
    #[serde(default)]
    write_version: u64,
    #[serde(default)]
    known_field: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl RegistryDocument for TestDoc {
    fn write_version(&self) -> u64 {
        self.write_version
    }
    fn set_write_version(&mut self, version: u64) {
        self.write_version = version;
    }
}

fn write(home: &Path, path: &Path, doc: &mut TestDoc) {
    let fs = RealFs::new();
    let leases = FileLease::new(home.join(".leases"));
    write_registry_document(&leases, &fs, home, path, doc).unwrap();
}

/// Flow: a registry document already on disk carries a key
/// (`a_future_field`) the current schema does not know about (an older or
/// newer build wrote it). A caller reads it, changes a known field, and
/// writes it back through the lease-guarded path.
/// Expectation: `a_future_field` survives byte-identical.
/// Failure here (the key silently disappearing) would mean a lease-guarded
/// write and an older/newer build's registry can no longer share the file.
#[test]
fn registry_flatten_catch_all_round_trips_an_unknown_key_or_names_the_dropped_key() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let path = home.join(".agents").join("skill-studio.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"write_version":0,"known_field":"a","a_future_field":{"nested":true}}"#,
    )
    .unwrap();

    let mut doc: TestDoc = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        doc.extra.get("a_future_field"),
        Some(&serde_json::json!({"nested": true}))
    );
    doc.known_field = "b".to_string();
    write(home, &path, &mut doc);

    let reloaded: TestDoc = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        reloaded.extra.get("a_future_field"),
        Some(&serde_json::json!({"nested": true}))
    );
    assert_eq!(reloaded.known_field, "b");
}

/// Flow: a document at write-version 3 goes through one lease-guarded
/// write.
/// Expectation: the write-version on disk (and on the in-memory value the
/// caller holds) becomes 4.
/// Failure here means a lease loser's stale-read check (a follow-up) would
/// have nothing to compare against - the counter never moves.
#[test]
fn write_registry_document_bumps_write_version_by_one_or_names_the_stuck_value() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let path = home.join(".agents").join("skill-studio.json");

    let mut doc = TestDoc {
        write_version: 3,
        ..Default::default()
    };
    write(home, &path, &mut doc);
    assert_eq!(doc.write_version, 4);

    let reloaded: TestDoc = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(reloaded.write_version, 4);
}

/// Flow: a document written before this field existed (absent from the
/// JSON) is read through the trait's `#[serde(default)]`.
/// Expectation: it starts at write-version 0.
/// Failure here means an old registry file would panic or misbehave on
/// first read instead of starting the counter cleanly.
#[test]
fn missing_write_version_defaults_to_zero_or_names_the_thrown_error() {
    let json = r#"{"known_field":"a"}"#;
    let doc: TestDoc = serde_json::from_str(json).unwrap();
    assert_eq!(doc.write_version(), 0);
}
