//! Real-disk integration tests for `ops::set_harness_enabled`.
//!
//! Like `park_and_unpark.rs`, these use `skill-studio-host`'s real adapters:
//! the mutation writes real symlinks and config files a fake filesystem
//! can't stand in for.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::SetHarnessEnabledRequest;
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{AgentId, SkillName};
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::scope::{ProjectSelection, RuntimeScope};
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FailingFs, FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";
const CLAUDE_ROOT_RELATIVE: &str = ".claude/skills";
const CODEX_ROOT_RELATIVE: &str = ".codex/skills";

fn runtime_with(
    home: &Path,
    projects: Vec<std::path::PathBuf>,
    fs: Arc<dyn skill_studio_core::ports::ScopeFs>,
) -> Runtime {
    let history_root = home.join(".history");
    let db_path = history_root.join("events.sqlite3");
    let mut scope = RuntimeScope::fixture(home);
    scope.projects = ProjectSelection::Explicit { paths: projects };
    let ports = Ports {
        fs,
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(db_path)),
        sink: Arc::new(RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    Runtime::new(&scope, ports).unwrap()
}

fn runtime_for(home: &Path) -> Runtime {
    runtime_with(home, Vec::new(), Arc::new(RealFs::new()))
}

fn install_universal_skill(home: &Path, name: &str) {
    let dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: a switchable skill\n---\nBody.\n"),
    )
    .unwrap();
}

fn install_claude_link(home: &Path, name: &str) {
    let target = home.join(UNIVERSAL_ROOT_RELATIVE).join(name);
    let claude_skills = home.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&claude_skills).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, claude_skills.join(name)).unwrap();
}

/// each_of_the_four_harness_switch_tests_passes_against_its_fixture_home_or_names_the_wrong_file:
/// one case per harness, each asserting the exact file `enable-and-links.md`
/// names for that harness.
#[test]
fn each_of_the_four_harness_switch_tests_passes_against_its_fixture_home_or_names_the_wrong_file() {
    // Claude Code: the per-skill link under `.claude/skills/<name>` is
    // removed, then recreated pointing at the universal directory.
    {
        let home = unique_temp_dir("switch_claude_code");
        install_universal_skill(&home, "gamma");
        install_claude_link(&home, "gamma");
        let rt = runtime_for(&home);
        let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "fixture setup: {} should start linked",
            link.display()
        );

        ops::set_harness_enabled(
            &rt,
            &ctx(),
            &SetHarnessEnabledRequest {
                skill: SkillName("gamma".into()),
                harness: AgentId::from(AgentId::CLAUDE_CODE),
                enabled: false,
            },
        )
        .unwrap();
        assert!(
            std::fs::symlink_metadata(&link).is_err(),
            "claude code disable should remove {}",
            link.display()
        );

        ops::set_harness_enabled(
            &rt,
            &ctx(),
            &SetHarnessEnabledRequest {
                skill: SkillName("gamma".into()),
                harness: AgentId::from(AgentId::CLAUDE_CODE),
                enabled: true,
            },
        )
        .unwrap();
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "claude code enable should recreate {}",
            link.display()
        );
        std::fs::remove_dir_all(&home).ok();
    }

    // Codex: a `[[skills.config]]` row keyed by the universal `SKILL.md`
    // path lands in `.codex/config.toml`.
    {
        let home = unique_temp_dir("switch_codex");
        install_universal_skill(&home, "gamma");
        let rt = runtime_for(&home);
        ops::set_harness_enabled(
            &rt,
            &ctx(),
            &SetHarnessEnabledRequest {
                skill: SkillName("gamma".into()),
                harness: AgentId::from(AgentId::CODEX),
                enabled: false,
            },
        )
        .unwrap();
        let config_path = home.join(".codex/config.toml");
        let text = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
            panic!("codex disable should write {}: {e}", config_path.display())
        });
        assert!(
            text.contains("[[skills.config]]") && text.contains("gamma/SKILL.md"),
            "expected a disabled row for gamma in {}, got:\n{text}",
            config_path.display()
        );
        std::fs::remove_dir_all(&home).ok();
    }

    // OpenCode: `permission.skill.<name>` is set to `"deny"` in
    // `opencode.json`.
    {
        let home = unique_temp_dir("switch_opencode");
        install_universal_skill(&home, "gamma");
        let rt = runtime_for(&home);
        ops::set_harness_enabled(
            &rt,
            &ctx(),
            &SetHarnessEnabledRequest {
                skill: SkillName("gamma".into()),
                harness: AgentId::from(AgentId::OPEN_CODE),
                enabled: false,
            },
        )
        .unwrap();
        let config_path = home.join(".config/opencode/opencode.json");
        let text = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
            panic!(
                "opencode disable should write {}: {e}",
                config_path.display()
            )
        });
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value["permission"]["skill"]["gamma"],
            "deny",
            "expected permission.skill.gamma = deny in {}, got: {text}",
            config_path.display()
        );
        std::fs::remove_dir_all(&home).ok();
    }

    // pi: this build's stand-in switch, `skill-studio.disabledSkills` in
    // pi's own settings file.
    {
        let home = unique_temp_dir("switch_pi");
        install_universal_skill(&home, "gamma");
        let rt = runtime_for(&home);
        ops::set_harness_enabled(
            &rt,
            &ctx(),
            &SetHarnessEnabledRequest {
                skill: SkillName("gamma".into()),
                harness: AgentId::from(AgentId::PI),
                enabled: false,
            },
        )
        .unwrap();
        let config_path = home.join(".pi/agent/settings.json");
        let text = std::fs::read_to_string(&config_path)
            .unwrap_or_else(|e| panic!("pi disable should write {}: {e}", config_path.display()));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value["skill-studio"]["disabledSkills"][0],
            "gamma",
            "expected skill-studio.disabledSkills to include gamma in {}, got: {text}",
            config_path.display()
        );
        std::fs::remove_dir_all(&home).ok();
    }
}

/// set_harness_enabled_writes_a_journal_row_before_the_first_path_toggles_or_names_the_missing_step:
/// a failure on pi's single write still leaves a durable journal row - the
/// row was recorded before the write, not after.
#[test]
fn set_harness_enabled_writes_a_journal_row_before_the_first_path_toggles_or_names_the_missing_step(
) {
    let home = unique_temp_dir("switch_journal_first");
    install_universal_skill(&home, "gamma");
    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(&home, Vec::new(), failing_fs.clone());

    failing_fs.fail_next_write_atomic();
    let err = ops::set_harness_enabled(
        &rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            skill: SkillName("gamma".into()),
            harness: AgentId::from(AgentId::PI),
            enabled: false,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Io);

    let events = ops::list_events(
        &rt,
        &ctx(),
        &skill_studio_core::dto::ListEventsRequest::default(),
    )
    .unwrap();
    assert_eq!(
        events.len(),
        1,
        "the set_harness_enabled row must be recorded before the write step runs"
    );
    assert_eq!(events[0].kind, "harness_disable");
}

/// a_crash_mid_codex_loop_reports_n_of_m_paths_toggled_instead_of_failing_silently:
/// five projects each own a `.codex/skills/epsilon` copy; the fourth
/// `write_atomic` call fails, so the loop stops having toggled 3 of 5.
#[test]
fn a_crash_mid_codex_loop_reports_n_of_m_paths_toggled_instead_of_failing_silently() {
    let home = unique_temp_dir("switch_codex_crash");
    let mut projects = Vec::new();
    for n in 0..5 {
        let project = home.join(format!("project-{n}"));
        let dir = project.join(CODEX_ROOT_RELATIVE).join("epsilon");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: epsilon\ndescription: a five-path codex skill\n---\nBody.\n",
        )
        .unwrap();
        projects.push(project);
    }
    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(&home, projects, failing_fs.clone());

    // The first `write_atomic` backs up `config.toml`'s manifest, not the
    // config file itself, so it is not one of the loop's five writes; three
    // successes lets rows 1-3 land before row 4 fails.
    failing_fs.fail_write_atomic_after(3);
    let err = ops::set_harness_enabled(
        &rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            skill: SkillName("epsilon".into()),
            harness: AgentId::from(AgentId::CODEX),
            enabled: false,
        },
    )
    .unwrap_err();
    assert!(
        err.message.contains("3 of 5"),
        "expected the error to name how many of the five Codex paths toggled, got: {}",
        err.message
    );
}
