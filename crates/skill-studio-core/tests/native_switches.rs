// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk tests for the per-harness switches other than Claude Code's:
//! Codex's `[[skills.config]]` row, OpenCode's `permission.skill` deny, and
//! pi, which has no switch. Every test runs against a temp home.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::{DeploymentDto, Inventory, SetHarnessEnabledRequest};
use skill_studio_core::harness::{DisabledBy, HarnessCatalog};
use skill_studio_core::identity::{AgentId, RootKind, SkillName};
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

fn runtime_with_scope(home: &Path, scope: RuntimeScope) -> Runtime {
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(
            home.join(".history").join("events.sqlite3"),
        )),
        sink: Arc::new(RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
        telemetry: Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    Runtime::new(&scope, ports).unwrap()
}

fn runtime_for(home: &Path) -> Runtime {
    runtime_with_scope(home, RuntimeScope::fixture(home))
}

fn write_skill(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: a switchable skill\n---\nBody.\n"),
    )
    .unwrap();
}

fn switch(rt: &Runtime, name: &str, harness: &'static str, enabled: bool) {
    ops::set_harness_enabled(
        rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            skill: SkillName(name.into()),
            harness: AgentId::from(harness),
            enabled,
            project_path: None,
        },
    )
    .unwrap_or_else(|e| panic!("the {harness} switch for {name} failed: {}", e.message));
}

fn scan(rt: &Runtime) -> Inventory {
    ops::scan(rt, &ctx(), &Default::default()).unwrap()
}

fn deployment_in<'a>(inventory: &'a Inventory, name: &str, kind: &RootKind) -> &'a DeploymentDto {
    inventory
        .skills
        .iter()
        .find(|s| s.name.0 == name)
        .unwrap_or_else(|| panic!("the scan lost {name}"))
        .deployments
        .iter()
        .find(|d| &d.root.kind == kind)
        .unwrap_or_else(|| panic!("the scan found no {kind:?} deployment of {name}"))
}

fn codex_root() -> RootKind {
    RootKind::Harness(AgentId::from(AgentId::CODEX))
}

/// The overlay's question, asked the way the desktop asks it: does Codex's
/// config turn off the `SKILL.md` under `skill_dir`?
fn codex_config_turns_off(home: &Path, skill_dir: &Path) -> bool {
    let fs = RealFs::new();
    ops::codex_disabled_skill_md_paths(&fs, &home.join(".codex"))
        .contains(&ops::codex_path_form(&fs, &skill_dir.join("SKILL.md")))
}

#[test]
fn codex_disable_over_a_row_left_enabled_turns_it_off_and_the_rescan_shows_the_skill_off() {
    let home = unique_temp_dir("codex_enabled_row");
    let skill_dir = home.join(".codex/skills/gamma");
    write_skill(&skill_dir, "gamma");
    let config_path = home.join(".codex/config.toml");
    std::fs::write(
        &config_path,
        format!(
            "model = \"o3\"\n\n[[skills.config]]\npath = \"{}\"\nenabled = true\n",
            skill_dir.join("SKILL.md").display()
        ),
    )
    .unwrap();
    let rt = runtime_for(&home);

    switch(&rt, "gamma", AgentId::CODEX, false);

    let text = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        text.contains("enabled = false") && !text.contains("enabled = true"),
        "the disable left the user's enabled = true row in place, so Codex still loads the skill:\n{text}"
    );
    assert_eq!(
        text.matches("[[skills.config]]").count(),
        1,
        "the disable added a second row instead of turning the existing one off:\n{text}"
    );
    assert!(
        text.starts_with("model = \"o3\""),
        "the disable dropped unrelated config:\n{text}"
    );
    assert_eq!(
        deployment_in(&scan(&rt), "gamma", &codex_root()).disabled_by,
        Some(DisabledBy::CodexConfig),
        "the rescan does not show the Codex row off after the disable"
    );

    switch(&rt, "gamma", AgentId::CODEX, true);
    assert_eq!(
        deployment_in(&scan(&rt), "gamma", &codex_root()).disabled_by,
        None,
        "the rescan still shows the skill off after the enable"
    );
    std::fs::remove_dir_all(&home).ok();
}

#[cfg(unix)]
#[test]
fn codex_row_codex_wrote_through_a_symlinked_skills_folder_reads_as_off_and_the_enable_clears_it() {
    let home = unique_temp_dir("codex_symlinked_root");
    let real_root = home.join("dotfiles/codex-skills");
    write_skill(&real_root.join("gamma"), "gamma");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::os::unix::fs::symlink(&real_root, home.join(".codex/skills")).unwrap();
    // Codex's own `/skills` toggle writes the canonical path.
    let canonical_skill_md = real_root.join("gamma/SKILL.md").canonicalize().unwrap();
    let config_path = home.join(".codex/config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[[skills.config]]\npath = \"{}\"\nenabled = false\n",
            canonical_skill_md.display()
        ),
    )
    .unwrap();
    let rt = runtime_for(&home);

    assert_eq!(
        deployment_in(&scan(&rt), "gamma", &codex_root()).disabled_by,
        Some(DisabledBy::CodexConfig),
        "the scan compared the lexical deployment path with Codex's canonical row and missed that the skill is off"
    );

    switch(&rt, "gamma", AgentId::CODEX, true);

    let text = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !text.contains("enabled = false"),
        "the enable did not find Codex's canonical row, so the skill stays off:\n{text}"
    );
    std::fs::remove_dir_all(&home).ok();
}

#[cfg(unix)]
#[test]
fn codex_disable_through_a_symlinked_universal_folder_reads_as_off_in_the_overlay_check() {
    let home = unique_temp_dir("codex_symlinked_universal");
    let real_root = home.join("dotfiles/agents-skills");
    write_skill(&real_root.join("gamma"), "gamma");
    std::fs::create_dir_all(home.join(".agents")).unwrap();
    std::os::unix::fs::symlink(&real_root, home.join(".agents/skills")).unwrap();
    let rt = runtime_for(&home);
    let universal_dir = home.join(".agents/skills/gamma");

    switch(&rt, "gamma", AgentId::CODEX, false);
    assert!(
        codex_config_turns_off(&home, &universal_dir),
        "after the disable, the overlay check does not see the Universal skill as off for Codex"
    );

    switch(&rt, "gamma", AgentId::CODEX, true);
    assert!(
        !codex_config_turns_off(&home, &universal_dir),
        "after the enable, the overlay check still sees the Universal skill as off for Codex"
    );
    std::fs::remove_dir_all(&home).ok();
}
