// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk integration test for `harness::read_claude_skill_overrides`.
//!
//! Like `registry_write.rs`, this uses `skill-studio-host`'s real `RealFs`
//! rather than the in-memory `FixtureFs`. The fixture settings files are
//! written straight to disk: the app only reads this file.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::harness::read_claude_skill_overrides;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::unique_temp_dir;
use skill_studio_core::testing::{FakeClock, FakeIds, NoHistory, RecordingSink};

use skill_studio_host::{FileLease, RealFs};

fn runtime_for(home: &Path) -> Runtime {
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

        telemetry: std::sync::Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    Runtime::new(&RuntimeScope::fixture(home), ports).unwrap()
}

/// Flow: `~/.claude/settings.json` carries a `skillOverrides` map next to
/// keys the core knows nothing about (a person's own settings).
/// Expectation: the read returns exactly the `skillOverrides` entries.
/// Failure here (an entry missing, or an unrelated key leaking in) would
/// mean the app shows a skill as switched off, or on, when Claude Code
/// does not.
#[test]
fn claude_code_skill_overrides_reads_only_the_skill_overrides_map_or_names_the_wrong_entry() {
    let home = unique_temp_dir("claude_skill_overrides");
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"theme":"dark","enabledPlugins":{"foo@bar":true},"skillOverrides":{"gamma":"user-invocable-only"}}"#,
    )
    .unwrap();

    let rt = runtime_for(&home);
    let overrides = read_claude_skill_overrides(rt.ports.fs.as_ref(), &home, None);

    assert_eq!(
        overrides,
        serde_json::json!({"gamma": "user-invocable-only"})
            .as_object()
            .cloned()
            .unwrap(),
        "the read did not return exactly the skillOverrides entries"
    );
}

/// Flow: `CLAUDE_CONFIG_DIR` points a Claude Code install's config directory
/// somewhere other than `~/.claude` (the host resolves the env var and
/// passes it through as `config_dir_override`).
/// Expectation: a read with that override sees `<override>/settings.json`,
/// and a read without it sees nothing from that file.
/// Failure here (the read looking under `~/.claude` regardless) would mean
/// an override user's real settings file silently diverges from what the
/// app shows.
#[test]
fn claude_code_skill_overrides_follow_claude_config_dir_or_names_the_wrong_settings_file() {
    let home = unique_temp_dir("claude_skill_overrides_config_dir");
    let config_dir = home.join("custom-claude-config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("settings.json"),
        r#"{"skillOverrides":{"delta":"user-invocable-only"}}"#,
    )
    .unwrap();

    let rt = runtime_for(&home);
    let fs = rt.ports.fs.as_ref();

    let after = read_claude_skill_overrides(fs, &home, Some(&config_dir));
    assert_eq!(
        after.get("delta"),
        Some(&serde_json::Value::String(
            "user-invocable-only".to_string()
        )),
        "reading with the config dir override did not see the file"
    );

    let ignoring_override = read_claude_skill_overrides(fs, &home, None);
    assert!(
        ignoring_override.is_empty(),
        "reading with no override saw the override dir's overrides: {ignoring_override:?}"
    );
}
