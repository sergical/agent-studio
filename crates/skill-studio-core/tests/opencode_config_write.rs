// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk integration tests for [`skill_studio_core::opencode_config`]'s
//! lease-guarded write path.
//!
//! Uses `skill-studio-host`'s real `RealFs`/`FileLease` rather than the
//! in-memory `FixtureFs`, since `FixtureFs::write_atomic` refuses every
//! call (see `registry_write.rs`) - this module's whole job is the write.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::opencode_config::{
    detect_config_kind, opencode_json_path, read_denied_patterns, read_skill_rules,
    set_skill_denied, set_skill_denied_with, OpencodeConfigKind,
};
use skill_studio_core::ports::{acquire_exclusive, Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::{FakeClock, FakeIds, NoHistory, RecordingSink};
use skill_studio_host::{FileLease, RealFs};

fn deny(config_dir: &Path, name: &str, denied: bool) {
    let fs = RealFs::new();
    let leases = FileLease::new(config_dir.join(".leases"));
    set_skill_denied(&leases, &fs, config_dir, name, denied).unwrap();
}

/// Flow: `opencode.json` already on disk carries a `theme` key and a
/// `$schema` the app never wrote, plus its own deny rule; a caller adds a
/// second skill's deny through the lease-guarded write path.
/// Expectation: `theme`, `$schema`, and the first skill's deny rule survive
/// byte-identical; only the new skill's rule is added.
/// Failure here (either key silently dropped) would mean the write path
/// clobbers config a person or another tool wrote.
#[test]
fn opencode_config_write_round_trips_unrelated_keys_and_keeps_schema_or_names_the_dropped_key() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path();
    std::fs::write(
        opencode_json_path(config_dir),
        r#"{"$schema": "https://opencode.ai/config.json", "theme": "dark", "permission": {"skill": {"find-bugs": "deny"}}}"#,
    )
    .unwrap();

    deny(config_dir, "write-tests", true);

    let fs = RealFs::new();
    let content = std::fs::read_to_string(opencode_json_path(config_dir)).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(value["theme"], "dark");
    assert_eq!(value["$schema"], "https://opencode.ai/config.json");
    let mut denied = read_denied_patterns(&fs, config_dir);
    denied.sort();
    assert_eq!(
        denied,
        vec!["find-bugs".to_string(), "write-tests".to_string()]
    );
}

/// Flow: no `opencode.json` exists yet.
/// Expectation: a fresh deny write creates it with the documented `$schema`.
#[test]
fn creates_opencode_json_with_schema_when_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join("config");

    deny(&config_dir, "find-bugs", true);

    let content = std::fs::read_to_string(opencode_json_path(&config_dir)).unwrap();
    assert!(content.contains("https://opencode.ai/config.json"));
}

/// Flow: a skill is denied, then the same write path clears it.
/// Expectation: the `permission` key disappears entirely rather than
/// leaving an empty `{"permission": {"skill": {}}}` behind.
#[test]
fn clearing_the_last_denied_skill_removes_the_permission_key() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path();
    std::fs::write(
        opencode_json_path(config_dir),
        r#"{"theme": "dark", "permission": {"skill": {"find-bugs": "deny"}}}"#,
    )
    .unwrap();

    deny(config_dir, "find-bugs", false);

    let content = std::fs::read_to_string(opencode_json_path(config_dir)).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(value["theme"], "dark");
    assert!(value.get("permission").is_none());
}

/// Flow: `~/.config/opencode` is itself a symlink to `~/dotfiles/opencode`
/// (a dotfiles layout), and no `opencode.json` exists yet under either path.
/// Expectation: the deny write succeeds and lands in the symlink's real
/// target, not the symlink path - `confine`'s canonical-parent check would
/// otherwise refuse the write, since the target sits outside
/// `~/.config`.
/// Failure here would mean a dotfiles user can never disable a skill for
/// `OpenCode` from Skill Studio.
#[cfg(unix)]
#[test]
fn opencode_deny_write_follows_a_symlinked_config_dir_or_names_the_refused_write() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let real_dir = home.join("dotfiles/opencode");
    std::fs::create_dir_all(&real_dir).unwrap();
    let config_dir = home.join(".config/opencode");
    std::fs::create_dir_all(config_dir.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real_dir, &config_dir).unwrap();

    deny(&config_dir, "find-bugs", true);

    let content = std::fs::read_to_string(opencode_json_path(&real_dir)).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(value["permission"]["skill"]["find-bugs"], "deny");
}

/// Flow: `set_skill_denied` writes a fresh deny for `epsilon`, and the same
/// module's own reader reads the file back.
/// Expectation: `read_denied_patterns` reports exactly `["epsilon"]`, and
/// the on-disk JSON actually holds the `permission.skill.epsilon` key the
/// write claims to have made - the reader's answer is checked against the
/// reader itself deriving from the write, not against a hand-typed shape
/// string.
/// Failure here (a mismatch, or the JSON pointer missing) would mean the
/// write and the read have silently drifted apart.
#[test]
fn set_skill_denied_writes_the_shape_read_denied_patterns_reads_back_or_names_the_key_it_wrote() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path();

    deny(config_dir, "epsilon", true);

    let content = std::fs::read_to_string(opencode_json_path(config_dir)).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        value.pointer("/permission/skill/epsilon"),
        Some(&serde_json::Value::String("deny".to_string())),
        "the JSON pointer for the written key is missing: {value}"
    );

    let fs = RealFs::new();
    assert_eq!(
        read_denied_patterns(&fs, config_dir),
        vec!["epsilon".to_string()]
    );
}

/// Flow: the caller already holds the exclusive lease on `config_dir`'s
/// parent (`home`) - the desktop disable command's `WriteLease`, e.g. when
/// `OPENCODE_CONFIG_DIR=$HOME/x` hashes the config write's scope root to the
/// same lease key as the outer `home` lease - and calls
/// `set_skill_denied_with` with that guard, on the SAME `FileLease` root the
/// production desktop code shares between the Codex and `OpenCode` arms.
/// Expectation: the write lands with the guard still held - proven by
/// reading the deny rule back afterward - with no second `acquire` in
/// between; a second real `acquire_exclusive` on the same lease root while
/// the first is held would flock-fail (`ScopeBusy`), which this test would
/// need to work around if `set_skill_denied_with` still acquired internally.
/// Failure: `set_skill_denied_with` errors (or hangs) here, which would mean
/// it still acquires its own lease and self-deadlocks against the caller's
/// held guard.
#[test]
fn opencode_disable_under_a_held_home_lease_writes_or_names_the_lease_it_deadlocked_on() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config_dir = home.join("x");
    std::fs::create_dir_all(&config_dir).unwrap();

    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(NoHistory),
        sink: Arc::new(RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(skill_studio_core::harness::HarnessCatalog::builtin()),
    };
    let rt = Runtime::new(&RuntimeScope::fixture(home), ports).unwrap();
    let fs = rt.ports.fs.as_ref();
    let guard = acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope).unwrap();

    set_skill_denied_with(fs, &guard, &config_dir, "epsilon", true)
        .expect("set_skill_denied_with deadlocked on its own caller's held lease");

    let rules = read_skill_rules(fs, &config_dir);
    assert!(rules.is_denied("epsilon"), "the write did not land");
}

/// Flow: only `opencode.jsonc` exists in the config directory (no `.json`
/// sibling).
/// Expectation: the write refuses rather than creating a `.json` file
/// `OpenCode` would then have to merge, or silently doing nothing.
#[test]
fn refuses_to_write_when_only_jsonc_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path();
    std::fs::write(config_dir.join("opencode.jsonc"), "// comment\n{}").unwrap();

    let fs = RealFs::new();
    let leases = FileLease::new(config_dir.join(".leases"));
    let err = set_skill_denied(&leases, &fs, config_dir, "find-bugs", true).unwrap_err();
    assert!(err.message.contains("opencode.jsonc"));
    assert_eq!(
        detect_config_kind(&fs, config_dir),
        Some(OpencodeConfigKind::Jsonc)
    );
}
