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

use skill_studio_core::opencode_config::{
    detect_config_kind, opencode_json_path, read_denied_patterns, set_skill_denied,
    OpencodeConfigKind,
};
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
