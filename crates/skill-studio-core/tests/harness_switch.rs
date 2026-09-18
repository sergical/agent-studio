//! Real-disk integration tests for `ops::set_harness_enabled`.
//!
//! Like `park_and_unpark.rs`, these use `skill-studio-host`'s real adapters:
//! the mutation writes real symlinks and config files a fake filesystem
//! can't stand in for.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::{RestoreRequest, SetHarnessEnabledRequest};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{AgentId, SkillName};
use skill_studio_core::ops;
use skill_studio_core::ports::{HistoryAccess, Ports, Runtime};
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
                project_path: None,
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
                project_path: None,
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
                project_path: None,
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
                project_path: None,
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
                project_path: None,
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
            project_path: None,
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
            project_path: None,
        },
    )
    .unwrap_err();
    assert!(
        err.message.contains("3 of 5"),
        "expected the error to name how many of the five Codex paths toggled, got: {}",
        err.message
    );
}

fn install_project_universal_skill(project: &Path, name: &str) {
    let dir = project.join(UNIVERSAL_ROOT_RELATIVE).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: a project-scoped switchable skill\n---\nBody.\n"),
    )
    .unwrap();
}

/// set_harness_enabled_accepts_opencode_and_open_code_spellings_or_names_the_rejected_id:
/// `AgentId::parse_harness` must fold `opencode`, `open-code`, and
/// `open_code` (any case) onto the same wire id `set_harness_enabled`
/// matches on, and still reject a spelling that names no harness at all.
#[test]
fn set_harness_enabled_accepts_opencode_and_open_code_spellings_or_names_the_rejected_id() {
    let home = unique_temp_dir("opencode_spellings");
    install_universal_skill(&home, "gamma");
    let rt = runtime_for(&home);
    let config_path = home.join(".config/opencode/opencode.json");

    for (n, spelling) in ["opencode", "open-code", "open_code", "OpenCode"]
        .into_iter()
        .enumerate()
    {
        let harness = AgentId::parse_harness(spelling)
            .unwrap_or_else(|e| panic!("{spelling} should parse as a harness id: {}", e.message));
        assert_eq!(harness.as_str(), AgentId::OPEN_CODE, "spelling: {spelling}");

        // Alternate disable/enable so each iteration writes a real toggle.
        let enabled = n % 2 == 1;
        ops::set_harness_enabled(
            &rt,
            &ctx(),
            &SetHarnessEnabledRequest {
                skill: SkillName("gamma".into()),
                harness,
                enabled,
                project_path: None,
            },
        )
        .unwrap_or_else(|e| panic!("{spelling} should toggle OpenCode: {}", e.message));
        let text = std::fs::read_to_string(&config_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        if enabled {
            assert!(
                value["permission"]["skill"]["gamma"].is_null(),
                "spelling {spelling} should have cleared permission.skill.gamma on enable, got: {text}"
            );
        } else {
            assert_eq!(
                value["permission"]["skill"]["gamma"], "deny",
                "spelling {spelling} should have written permission.skill.gamma = deny"
            );
        }
    }

    let rejected = AgentId::parse_harness("not a harness!").unwrap_err();
    assert!(
        rejected.message.contains("not a harness!"),
        "expected the rejected id in the error, got: {}",
        rejected.message
    );

    std::fs::remove_dir_all(&home).ok();
}

/// claude_code_undo_of_undo_removes_the_recreated_link_or_names_the_stale_inverse:
/// disable removes the link and journals `recreate_symlink` as its inverse;
/// undoing that disable recreates the link and must journal `remove_symlink`
/// as its own inverse (not another `recreate_symlink`), so undoing the undo
/// removes the link again instead of trying to recreate an already-present
/// one.
#[test]
fn claude_code_undo_of_undo_removes_the_recreated_link_or_names_the_stale_inverse() {
    let home = unique_temp_dir("claude_undo_of_undo");
    install_universal_skill(&home, "gamma");
    install_claude_link(&home, "gamma");
    let rt = runtime_for(&home);
    let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");

    let disable = ops::set_harness_enabled(
        &rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            skill: SkillName("gamma".into()),
            harness: AgentId::from(AgentId::CLAUDE_CODE),
            enabled: false,
            project_path: None,
        },
    )
    .unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "disable should remove {}",
        link.display()
    );

    let undo = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: disable.event_id.clone(),
            force: false,
        },
    )
    .unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "undoing the disable should recreate {}",
        link.display()
    );

    let undo_of_undo = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: undo.restore_event_id.clone(),
            force: false,
        },
    )
    .unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "undoing the undo should remove the recreated link at {} again",
        link.display()
    );

    let store = rt
        .ports
        .history
        .open(&rt.scope, HistoryAccess::ReadIfExists)
        .unwrap()
        .expect("the store exists after the writes above");
    let disable_row = store.get(&disable.event_id).unwrap().unwrap();
    let undo_row = store.get(&undo.restore_event_id).unwrap().unwrap();
    let undo_of_undo_row = store.get(&undo_of_undo.restore_event_id).unwrap().unwrap();
    let inverse_op = |row: &skill_studio_core::events::EventRecord| {
        row.inverse
            .as_ref()
            .and_then(|v| v.get("op"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    assert_eq!(
        inverse_op(&disable_row).as_deref(),
        Some("recreate_symlink"),
        "the disable's own inverse should recreate the link"
    );
    assert_eq!(
        inverse_op(&undo_row).as_deref(),
        Some("remove_symlink"),
        "undoing the disable recreated the link, so its inverse must remove it, not recreate it again"
    );
    assert_eq!(
        inverse_op(&undo_of_undo_row).as_deref(),
        Some("recreate_symlink"),
        "undoing the undo removed the link, so its inverse must recreate it"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// claude_code_enable_creates_the_skills_dir_on_a_fresh_home_or_names_the_confine_error:
/// a home with no `.claude` directory at all must still let the first
/// enable succeed - `confine` needs the link's parent to exist, and nothing
/// else in this build creates `~/.claude/skills` first.
#[test]
fn claude_code_enable_creates_the_skills_dir_on_a_fresh_home_or_names_the_confine_error() {
    let home = unique_temp_dir("claude_enable_fresh_home");
    install_universal_skill(&home, "gamma");
    let rt = runtime_for(&home);
    let claude_skills_dir = home.join(CLAUDE_ROOT_RELATIVE);
    assert!(
        std::fs::symlink_metadata(&claude_skills_dir).is_err(),
        "fixture setup: {} should not exist yet",
        claude_skills_dir.display()
    );

    ops::set_harness_enabled(
        &rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            skill: SkillName("gamma".into()),
            harness: AgentId::from(AgentId::CLAUDE_CODE),
            enabled: true,
            project_path: None,
        },
    )
    .unwrap_or_else(|e| panic!("enable on a fresh home should succeed: {}", e.message));
    let link = claude_skills_dir.join("gamma");
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "enable should have created {}",
        link.display()
    );

    std::fs::remove_dir_all(&home).ok();
}

/// project_scoped_claude_code_disable_removes_the_project_link_or_names_the_global_link_it_touched_instead:
/// `gamma` is installed both globally and inside one project, each with its
/// own Claude Code link. A disable scoped to the project must remove only
/// `<project>/.claude/skills/gamma`, leaving the unrelated global link at
/// `<home>/.claude/skills/gamma` untouched - the opposite of what the
/// unscoped code did before, which always resolved the home slot.
#[test]
fn project_scoped_claude_code_disable_removes_the_project_link_or_names_the_global_link_it_touched_instead(
) {
    let home = unique_temp_dir("claude_project_scope");
    install_universal_skill(&home, "gamma");
    install_claude_link(&home, "gamma");
    let project = home.join("proj");
    install_project_universal_skill(&project, "gamma");
    let project_claude_skills = project.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&project_claude_skills).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        project.join(UNIVERSAL_ROOT_RELATIVE).join("gamma"),
        project_claude_skills.join("gamma"),
    )
    .unwrap();
    let rt = runtime_with(&home, vec![project.clone()], Arc::new(RealFs::new()));
    let global_link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");
    let project_link = project_claude_skills.join("gamma");

    ops::set_harness_enabled(
        &rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            skill: SkillName("gamma".into()),
            harness: AgentId::from(AgentId::CLAUDE_CODE),
            enabled: false,
            project_path: Some(project.clone()),
        },
    )
    .unwrap();

    assert!(
        std::fs::symlink_metadata(&project_link).is_err(),
        "project-scoped disable should remove {}",
        project_link.display()
    );
    assert!(
        std::fs::symlink_metadata(&global_link).is_ok(),
        "project-scoped disable must leave the global link at {} untouched, not names the wrong global slot it also removed",
        global_link.display()
    );

    std::fs::remove_dir_all(&home).ok();
}

/// opencode_toggle_refuses_a_skill_name_installed_in_two_locations_or_names_the_global_deny_leak:
/// `gamma` sits both at the global universal root and inside one project's
/// universal root; `permission.skill.gamma` is written once, globally, so a
/// project-scoped toggle must be refused rather than silently also denying
/// the unrelated global copy.
#[test]
fn opencode_toggle_refuses_a_skill_name_installed_in_two_locations_or_names_the_global_deny_leak() {
    let home = unique_temp_dir("opencode_collision");
    install_universal_skill(&home, "gamma");
    let project = home.join("proj");
    install_project_universal_skill(&project, "gamma");
    let rt = runtime_with(&home, vec![project.clone()], Arc::new(RealFs::new()));

    let err = ops::set_harness_enabled(
        &rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            skill: SkillName("gamma".into()),
            harness: AgentId::from(AgentId::OPEN_CODE),
            enabled: false,
            project_path: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::InvalidRequest);
    let global_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    let project_dir = project.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    assert!(
        err.message.contains(&global_dir.display().to_string())
            && err.message.contains(&project_dir.display().to_string()),
        "expected the error to name both {} and {}, got: {}",
        global_dir.display(),
        project_dir.display(),
        err.message
    );
    let config_path = home.join(".config/opencode/opencode.json");
    assert!(
        std::fs::symlink_metadata(&config_path).is_err(),
        "a refused toggle must not touch opencode.json at all"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// claude_code_toggle_marks_the_event_failed_when_the_link_write_fails_or_names_the_pending_row:
/// the journal row for a Claude Code enable is written before the symlink
/// call; when that call fails, the row must be finished `failed`, not left
/// `pending` (which `recover_interrupted` would later flip to `interrupted`
/// rather than a plain, retryable failure).
#[test]
fn claude_code_toggle_marks_the_event_failed_when_the_link_write_fails_or_names_the_pending_row() {
    let home = unique_temp_dir("claude_toggle_write_fails");
    install_universal_skill(&home, "gamma");
    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(&home, Vec::new(), failing_fs.clone());

    failing_fs.fail_next_symlink();
    let err = ops::set_harness_enabled(
        &rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            skill: SkillName("gamma".into()),
            harness: AgentId::from(AgentId::CLAUDE_CODE),
            enabled: true,
            project_path: None,
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
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].status, "failed",
        "a failed link write must leave the row failed, not pending"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// claude_code_disable_refuses_a_real_directory_or_whole_dir_link_or_names_the_removed_directory:
/// presence at `~/.claude/skills/<name>` only means "already linked" when it
/// is a symlink. A real directory there (a plain copy) or a whole-directory
/// link at `~/.claude/skills` itself must be refused, not torn down by
/// `remove_file`.
#[test]
fn claude_code_disable_refuses_a_real_directory_or_whole_dir_link_or_names_the_removed_directory() {
    // A real directory sits at the per-skill slot.
    {
        let home = unique_temp_dir("claude_disable_real_dir");
        install_universal_skill(&home, "gamma");
        let rt = runtime_for(&home);
        let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");
        std::fs::create_dir_all(&link).unwrap();
        std::fs::write(link.join("SKILL.md"), "---\nname: gamma\n---\n").unwrap();

        let err = ops::set_harness_enabled(
            &rt,
            &ctx(),
            &SetHarnessEnabledRequest {
                skill: SkillName("gamma".into()),
                harness: AgentId::from(AgentId::CLAUDE_CODE),
                enabled: false,
                project_path: None,
            },
        )
        .unwrap_err();
        assert_eq!(err.code, skill_studio_core::ErrorCode::InvalidRequest);
        assert!(
            err.message.contains(&link.display().to_string()),
            "expected the error to name {}, got: {}",
            link.display(),
            err.message
        );
        assert!(
            std::fs::metadata(&link).is_ok_and(|m| m.is_dir()),
            "the real directory at {} must not be removed",
            link.display()
        );
        std::fs::remove_dir_all(&home).ok();
    }

    // `~/.claude/skills` itself is a whole-directory link into the shared root.
    {
        let home = unique_temp_dir("claude_disable_whole_dir_link");
        install_universal_skill(&home, "gamma");
        let claude_skills_dir = home.join(CLAUDE_ROOT_RELATIVE);
        let universal_dir = home.join(UNIVERSAL_ROOT_RELATIVE);
        std::fs::create_dir_all(claude_skills_dir.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&universal_dir, &claude_skills_dir).unwrap();
        let rt = runtime_for(&home);

        let err = ops::set_harness_enabled(
            &rt,
            &ctx(),
            &SetHarnessEnabledRequest {
                skill: SkillName("gamma".into()),
                harness: AgentId::from(AgentId::CLAUDE_CODE),
                enabled: false,
                project_path: None,
            },
        )
        .unwrap_err();
        assert_eq!(err.code, skill_studio_core::ErrorCode::InvalidRequest);
        assert!(
            err.message
                .contains(&claude_skills_dir.display().to_string()),
            "expected the error to name {}, got: {}",
            claude_skills_dir.display(),
            err.message
        );
        assert!(
            std::fs::symlink_metadata(&claude_skills_dir).is_ok_and(|m| m.file_type().is_symlink()),
            "the whole-directory link at {} must not be removed",
            claude_skills_dir.display()
        );
        std::fs::remove_dir_all(&home).ok();
    }
}

/// undo_of_a_failed_recreate_restore_is_refused_or_names_the_live_link_it_would_remove:
/// disabling Claude Code removes the link and journals a `Recreate` inverse
/// on `E1`. A real directory occupies the link's slot before the undo runs,
/// so the undo's `fs.symlink` call fails: `E1` must stay restorable (its
/// claim was released, not consumed) and its own restore row `R` must finish
/// `failed`. `restore_event` on `R` must then be refused, naming `R`'s
/// status - `RestoreCapability::NotCompleted` is what makes that refusal
/// possible; without it, undoing `R` would apply `R`'s `remove_symlink`
/// inverse to the occupant directory, deleting state `R` never touched.
#[test]
fn undo_of_a_failed_recreate_restore_is_refused_or_names_the_live_link_it_would_remove() {
    let home = unique_temp_dir("claude_undo_failed_recreate");
    install_universal_skill(&home, "gamma");
    install_claude_link(&home, "gamma");
    let rt = runtime_for(&home);
    let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");

    let disable = ops::set_harness_enabled(
        &rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            skill: SkillName("gamma".into()),
            harness: AgentId::from(AgentId::CLAUDE_CODE),
            enabled: false,
            project_path: None,
        },
    )
    .unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "disable should remove {}",
        link.display()
    );

    // The occupant: a real directory sits where the undo's `Recreate`
    // inverse wants to put the link back.
    std::fs::create_dir_all(&link).unwrap();
    std::fs::write(link.join("SKILL.md"), "---\nname: gamma\n---\n").unwrap();

    let undo_err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: disable.event_id.clone(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(undo_err.code, skill_studio_core::ErrorCode::Io);
    assert!(
        std::fs::metadata(&link).is_ok_and(|m| m.is_dir()),
        "the occupant directory at {} must survive the failed undo",
        link.display()
    );

    let store = rt
        .ports
        .history
        .open(&rt.scope, HistoryAccess::ReadIfExists)
        .unwrap()
        .expect("the store exists after the writes above");
    let disable_row = store.get(&disable.event_id).unwrap().unwrap();
    assert_eq!(
        disable_row.restore_capability(),
        skill_studio_core::dto::RestoreCapability::Yes,
        "the failed undo must release its claim, leaving E1 restorable again"
    );

    let events = ops::list_events(
        &rt,
        &ctx(),
        &skill_studio_core::dto::ListEventsRequest::default(),
    )
    .unwrap();
    let failed_restore = events
        .iter()
        .find(|e| e.kind == "restore" && e.status == "failed")
        .expect("the undo's own restore row R must be recorded and finished failed");
    let restore_row = store.get(&failed_restore.id).unwrap().unwrap();
    assert_eq!(
        restore_row.status,
        skill_studio_core::events::EventStatus::Failed
    );

    let redo_err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: failed_restore.id.clone(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(redo_err.code, skill_studio_core::ErrorCode::InvalidRequest);
    assert!(
        redo_err.message.contains("failed"),
        "expected the refusal to name R's status (failed), got: {}",
        redo_err.message
    );
    assert!(
        std::fs::metadata(&link).is_ok_and(|m| m.is_dir()),
        "undoing R must still be refused, so the occupant directory at {} must remain",
        link.display()
    );

    std::fs::remove_dir_all(&home).ok();
}
