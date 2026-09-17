//! Real-disk integration test for `harness::{read_claude_skill_overrides,
//! write_claude_skill_overrides}`.
//!
//! Like `registry_write.rs`, this uses `skill-studio-host`'s real
//! `RealFs`/`FileLease` rather than the in-memory `FixtureFs`, since
//! `FixtureFs::write_atomic` refuses every call (see its doc comment in
//! `testing.rs`) - this module's whole job is the write.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::harness::{read_claude_skill_overrides, write_claude_skill_overrides};
use skill_studio_core::ports::{acquire_exclusive, Ports, Runtime};
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
    };
    Runtime::new(&RuntimeScope::fixture(home), ports).unwrap()
}

/// Flow: `~/.claude/settings.json` already carries keys the core knows
/// nothing about (a person's own settings, or a field a newer Claude Code
/// build added) alongside no `skillOverrides` yet. A caller reads the
/// current overrides (empty), adds one, and writes it back.
/// Expectation: the unrelated keys (`theme`, `enabledPlugins`) survive
/// byte-for-byte, and a re-read of `skillOverrides` sees the new entry.
/// Failure here (an unrelated key disappearing) would mean writing a
/// skill's override silently discards a person's own Claude Code settings.
#[test]
fn claude_code_skill_overrides_round_trips_unrelated_keys_in_settings_json_or_names_the_dropped_key(
) {
    let home = unique_temp_dir("claude_skill_overrides");
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let settings_path = claude_dir.join("settings.json");
    std::fs::write(
        &settings_path,
        r#"{"theme":"dark","enabledPlugins":{"foo@bar":true}}"#,
    )
    .unwrap();

    let rt = runtime_for(&home);
    let fs = rt.ports.fs.as_ref();
    let before = read_claude_skill_overrides(fs, &home);
    assert!(before.is_empty(), "no skillOverrides were written yet");

    let mut overrides = before;
    overrides.insert(
        "gamma".to_string(),
        serde_json::Value::String("user-invocable-only".to_string()),
    );
    let guard = acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope).unwrap();
    write_claude_skill_overrides(fs, &rt.scope, &guard, &home, overrides).unwrap();

    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(
        on_disk.get("theme"),
        Some(&serde_json::Value::String("dark".to_string())),
        "an unrelated top-level key was dropped by the skillOverrides write"
    );
    assert_eq!(
        on_disk.get("enabledPlugins"),
        Some(&serde_json::json!({"foo@bar": true})),
        "the enabledPlugins map, read by a different function, was dropped"
    );

    let after = read_claude_skill_overrides(fs, &home);
    assert_eq!(
        after.get("gamma"),
        Some(&serde_json::Value::String(
            "user-invocable-only".to_string()
        )),
        "the new override was not read back"
    );
}
