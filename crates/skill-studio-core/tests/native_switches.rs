// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk tests for reading the per-skill settings Codex
//! (`[[skills.config]]`) and `OpenCode` (`permission.skill`) keep in their own
//! config files. Skill Studio never writes them. Every test runs against a
//! temp home.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::{DeploymentDto, Inventory};
use skill_studio_core::harness::{DisabledBy, HarnessCatalog};
use skill_studio_core::identity::{AgentId, RootKind};
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

fn runtime_with_scope(home: &Path, scope: &RuntimeScope) -> Runtime {
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
    Runtime::new(scope, ports).unwrap()
}

fn runtime_for(home: &Path) -> Runtime {
    runtime_with_scope(home, &RuntimeScope::fixture(home))
}

fn write_skill(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: a switchable skill\n---\nBody.\n"),
    )
    .unwrap();
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
fn codex_row_with_enabled_false_reads_as_off_and_enabled_true_reads_as_on_or_names_the_wrong_reading(
) {
    let home = unique_temp_dir("codex_row_readings");
    let skill_dir = home.join(".codex/skills/gamma");
    write_skill(&skill_dir, "gamma");
    let config_path = home.join(".codex/config.toml");
    let row = |enabled: bool| {
        format!(
            "model = \"o3\"\n\n[[skills.config]]\npath = \"{}\"\nenabled = {enabled}\n",
            skill_dir.join("SKILL.md").display()
        )
    };
    let rt = runtime_for(&home);

    std::fs::write(&config_path, row(true)).unwrap();
    assert_eq!(
        deployment_in(&scan(&rt), "gamma", &codex_root()).disabled_by,
        None,
        "the scan shows the skill off although the user's row says enabled = true"
    );

    std::fs::write(&config_path, row(false)).unwrap();
    assert_eq!(
        deployment_in(&scan(&rt), "gamma", &codex_root()).disabled_by,
        Some(DisabledBy::CodexConfig),
        "the scan does not show the Codex row off for a user-written enabled = false row"
    );
    std::fs::remove_dir_all(&home).ok();
}

#[cfg(unix)]
#[test]
fn codex_row_codex_wrote_through_a_symlinked_skills_folder_reads_as_off_or_names_the_missed_canonical_path(
) {
    let home = unique_temp_dir("codex_symlinked_root");
    let real_root = home.join("dotfiles/codex-skills");
    write_skill(&real_root.join("gamma"), "gamma");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::os::unix::fs::symlink(&real_root, home.join(".codex/skills")).unwrap();
    // Codex's own `/skills` toggle writes the canonical path.
    let canonical_skill_md = real_root.join("gamma/SKILL.md").canonicalize().unwrap();
    std::fs::write(
        home.join(".codex/config.toml"),
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
    std::fs::remove_dir_all(&home).ok();
}

#[cfg(unix)]
#[test]
fn codex_row_for_a_skill_in_a_symlinked_universal_folder_reads_as_off_in_the_overlay_check() {
    let home = unique_temp_dir("codex_symlinked_universal");
    let real_root = home.join("dotfiles/agents-skills");
    write_skill(&real_root.join("gamma"), "gamma");
    std::fs::create_dir_all(home.join(".agents")).unwrap();
    std::os::unix::fs::symlink(&real_root, home.join(".agents/skills")).unwrap();
    let universal_dir = home.join(".agents/skills/gamma");
    let canonical_skill_md = universal_dir.join("SKILL.md").canonicalize().unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(
        home.join(".codex/config.toml"),
        format!(
            "[[skills.config]]\npath = \"{}\"\nenabled = false\n",
            canonical_skill_md.display()
        ),
    )
    .unwrap();

    assert!(
        codex_config_turns_off(&home, &universal_dir),
        "the overlay check does not see the Universal skill as off for Codex"
    );
    std::fs::remove_dir_all(&home).ok();
}

fn opencode_root() -> RootKind {
    RootKind::Harness(AgentId::from(AgentId::OPEN_CODE))
}

#[test]
fn opencode_deny_in_the_config_root_the_scan_reads_shows_the_skill_off_or_names_the_ignored_root() {
    let home = unique_temp_dir("opencode_config_root");
    // An adapter that honours `XDG_CONFIG_HOME` hands the core this root,
    // and OpenCode then reads its skills from under it too.
    let config_root = home.join("xdg/opencode");
    write_skill(&config_root.join("skills/delta"), "delta");
    std::fs::write(
        config_root.join("opencode.json"),
        "{\"permission\": {\"skill\": {\"delta\": \"deny\"}}}",
    )
    .unwrap();
    let mut scope = RuntimeScope::fixture(&home);
    scope.opencode_config_root = Some(config_root);
    let rt = runtime_with_scope(&home, &scope);

    assert_eq!(
        deployment_in(&scan(&rt), "delta", &opencode_root()).disabled_by,
        Some(DisabledBy::OpencodePermission),
        "the scan does not show the OpenCode row off for a deny in the config root"
    );
    std::fs::remove_dir_all(&home).ok();
}
