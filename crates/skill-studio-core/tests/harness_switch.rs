// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk integration tests for `ops::set_harness_enabled`.
//!
//! Like `park_and_unpark.rs`, these use `skill-studio-host`'s real adapters:
//! the mutation writes real symlinks and config files a fake filesystem
//! can't stand in for.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::{RestoreRequest, ScanRequest, SetHarnessEnabledRequest};
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

/// Like [`runtime_with`], but with `RuntimeScope::codex_home` set to
/// `codex_home` instead of defaulting to `home/.codex` - for asserting a
/// Codex write follows `CODEX_HOME` rather than the general home directory.
fn runtime_with_codex_home(home: &Path, codex_home: &Path) -> Runtime {
    let history_root = home.join(".history");
    let db_path = history_root.join("events.sqlite3");
    let scope = RuntimeScope::fixture(home).with_codex_home(codex_home);
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
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

/// `each_of_the_four_harness_switch_tests_passes_against_its_fixture_home_or_names_the_wrong_file`:
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

/// `set_harness_enabled_writes_a_journal_row_before_the_first_path_toggles_or_names_the_missing_step`:
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

/// `any_error_after_the_event_is_recorded_marks_it_failed_or_names_the_row_left_pending`:
/// pi's `ensure_dir_all`, called after the journal row is recorded to create
/// `~/.pi/agent` on a fresh home but before `write_atomic`, fails. That row
/// must finish `failed`, not stay `pending` - `recover_interrupted` would
/// later read a `pending` row as a crash mid-write rather than a plain,
/// retryable failure the caller already saw returned as an error.
#[test]
fn any_error_after_the_event_is_recorded_marks_it_failed_or_names_the_row_left_pending() {
    let home = unique_temp_dir("switch_post_record_failure");
    install_universal_skill(&home, "gamma");
    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(&home, Vec::new(), failing_fs.clone());

    failing_fs.fail_next_create_dir_all();
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
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].status, "failed",
        "an error after the journal row was recorded must leave it failed, not pending"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `a_crash_mid_codex_loop_reports_n_of_m_paths_toggled_instead_of_failing_silently`:
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

/// `a_failed_codex_toggle_with_a_backup_restores_with_force_or_names_the_row_it_refused`:
/// the same `fail_write_atomic_after(3)` crash as the test above leaves a
/// `Failed` event whose `restore_backup` inverse and `backup_dir` still
/// name a real pre-toggle snapshot of `config.toml`. `restore_capability`
/// must let that row through to the ordinary drift-checked `restore_backup`
/// path: refused without `force` (the three written rows are live, not
/// `pre`), applied with `force`.
#[test]
fn a_failed_codex_toggle_with_a_backup_restores_with_force_or_names_the_row_it_refused() {
    let home = unique_temp_dir("switch_codex_crash_restore");
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
    let config_path = home.join(".codex/config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let pre_bytes =
        b"[[skills.config]]\npath = \"/pre-existing/SKILL.md\"\nenabled = false\n".to_vec();
    std::fs::write(&config_path, &pre_bytes).unwrap();

    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(&home, projects, failing_fs.clone());
    failing_fs.fail_write_atomic_after(3);
    ops::set_harness_enabled(
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

    let events = ops::list_events(
        &rt,
        &ctx(),
        &skill_studio_core::dto::ListEventsRequest::default(),
    )
    .unwrap();
    let failed = events
        .iter()
        .find(|e| e.kind == "harness_disable" && e.status == "failed")
        .expect("the crashed disable must be recorded failed with its backup intact");

    // The budget from `fail_write_atomic_after(3)` above is exhausted, so
    // every later `write_atomic` call - including the restore's own -
    // would otherwise keep failing; the crash is over, so lift it.
    failing_fs.fail_write_atomic_after(u32::MAX);

    let no_force_err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: failed.id.clone(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(
        no_force_err.code,
        skill_studio_core::ErrorCode::DriftConflict
    );

    let outcome = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: failed.id.clone(),
            force: true,
        },
    )
    .unwrap();
    assert_eq!(outcome.reverted_event_id, failed.id);

    let restored = std::fs::read(&config_path).unwrap();
    assert_eq!(
        restored, pre_bytes,
        "force restore must put back exactly the bytes config.toml held before the toggle"
    );

    let events_after = ops::list_events(
        &rt,
        &ctx(),
        &skill_studio_core::dto::ListEventsRequest::default(),
    )
    .unwrap();
    assert!(
        events_after
            .iter()
            .any(|e| e.kind == "restore" && e.id == outcome.restore_event_id),
        "the restore itself must be recorded as its own event"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `a_pending_row_stays_unrestorable_or_names_the_crash_it_pretended_finished`:
/// a hand-recorded `pending` row with a `restore_backup` inverse and a
/// `backup_dir` - what a process death between `record` and `finish` leaves
/// behind - must stay `NotCompleted`: `pending` never proves the write it
/// describes ever ran, so the backup drift check below it has nothing
/// trustworthy to compare against. Reads the row back and calls
/// `restore_capability()` directly rather than through `ops::restore_event`:
/// that op always opens a `MutationSession`, whose `recover_interrupted`
/// step would first promote this stale `pending` row to `interrupted`
/// (crash recovery's own job, exercised elsewhere), masking the check this
/// test is for.
#[test]
fn a_pending_row_stays_unrestorable_or_names_the_crash_it_pretended_finished() {
    let home = unique_temp_dir("switch_pending_row");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let file_path = home.join("pending-target.txt");
    std::fs::write(&file_path, b"before").unwrap();

    let id = rt.ports.ids.next_event_id();
    let mut session = skill_studio_core::ports::MutationSession::begin(&rt, &ctx()).unwrap();
    let manifest = session
        .store
        .backup_paths(&session.guard, &id, std::slice::from_ref(&file_path))
        .unwrap();
    let pre_fingerprint = manifest.entries.first().and_then(|e| e.fingerprint.clone());
    // Mirrors `events::restore_backup_inverse`'s payload shape (that
    // function is crate-private; this integration test only has the
    // public API), so `parse_restore_backup_inverse` reads it the same way
    // a real `restore_backup` row would.
    let inverse = serde_json::json!({
        "op": "restore_backup",
        "path": &file_path,
        "pre_fingerprint": pre_fingerprint
            .as_ref()
            .map_or_else(|| "absent".to_string(), |f| f.bare_hex().to_string()),
        "post_fingerprint": "absent",
    });
    let draft = skill_studio_core::events::EventDraft {
        kind: skill_studio_core::events::EventKind::HarnessDisable,
        skill: SkillName("pending-skill".into()),
        harness: Some(AgentId::from(AgentId::CODEX)),
        scope: Some("global".to_string()),
        project_path: None,
        payload: serde_json::json!({}),
        inverse: Some(inverse),
        backup_dir: Some(manifest.backup_dir.clone()),
    };
    session.store.record(&session.guard, &id, &draft).unwrap();
    drop(session);

    let store = rt
        .ports
        .history
        .open(&rt.scope, HistoryAccess::ReadIfExists)
        .unwrap()
        .expect("the hand-recorded row's store exists after the write above");
    let row = store.get(&id).unwrap().unwrap();
    assert_eq!(
        row.status,
        skill_studio_core::events::EventStatus::Pending,
        "the row must still read back pending: nothing here ever called finish()"
    );
    match row.restore_capability() {
        skill_studio_core::dto::RestoreCapability::NotCompleted { status } => {
            assert_eq!(status, "pending");
        }
        other => panic!(
            "expected NotCompleted naming pending despite a backup_dir and a restore_backup \
             inverse, got: {other:?}"
        ),
    }

    std::fs::remove_dir_all(&home).ok();
}

/// `codex_disable_writes_rows_only_for_paths_codex_reads_or_names_the_foreign_path_it_wrote`:
/// `gamma` has a canonical universal copy plus an independent (not linked)
/// Claude Code copy - Codex never reads `.claude/skills`, so a Codex disable
/// must write exactly one `[[skills.config]]` row, for the universal path,
/// and none naming the Claude Code copy.
#[test]
fn codex_disable_writes_rows_only_for_paths_codex_reads_or_names_the_foreign_path_it_wrote() {
    let home = unique_temp_dir("switch_codex_visible_root");
    install_universal_skill(&home, "gamma");
    let claude_dir = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("SKILL.md"),
        "---\nname: gamma\ndescription: an independent claude code copy\n---\nBody.\n",
    )
    .unwrap();
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
    let text = std::fs::read_to_string(&config_path).unwrap();
    let claude_skill_md = claude_dir.join("SKILL.md");
    assert!(
        !text.contains(&claude_skill_md.display().to_string()),
        "expected no row naming the Claude Code copy at {}, got:\n{text}",
        claude_skill_md.display()
    );
    let rows = text.matches("[[skills.config]]").count();
    assert_eq!(
        rows, 1,
        "expected exactly one row, for the universal path Codex actually reads, got {rows} in:\n{text}"
    );

    std::fs::remove_dir_all(&home).ok();
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

/// `set_harness_enabled_accepts_opencode_and_open_code_spellings_or_names_the_rejected_id`:
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

/// `claude_code_undo_of_undo_removes_the_recreated_link_or_names_the_stale_inverse`:
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

/// `claude_code_enable_creates_the_skills_dir_on_a_fresh_home_or_names_the_confine_error`:
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

/// `project_scoped_claude_code_disable_removes_the_project_link_or_names_the_global_link_it_touched_instead`:
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

/// `disabling_an_already_disabled_claude_code_skill_records_no_undo_or_names_the_link_the_undo_would_delete`:
/// `gamma` starts with no Claude Code link at all; disabling it again is a
/// no-op on disk, so its journal row must carry no inverse. Restoring that
/// row must be refused rather than removing a link the no-op never created.
#[test]
fn disabling_an_already_disabled_claude_code_skill_records_no_undo_or_names_the_link_the_undo_would_delete(
) {
    let home = unique_temp_dir("claude_noop_disable");
    install_universal_skill(&home, "gamma");
    let rt = runtime_for(&home);
    let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "fixture setup: {} should start unlinked",
        link.display()
    );

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
        "a no-op disable must not create {}",
        link.display()
    );

    let store = rt
        .ports
        .history
        .open(&rt.scope, HistoryAccess::ReadIfExists)
        .unwrap()
        .expect("the store exists after the write above");
    let row = store.get(&disable.event_id).unwrap().unwrap();
    assert_eq!(
        row.inverse, None,
        "a no-op toggle must record no inverse, not one that would delete a link it never created"
    );
    assert_eq!(
        row.restore_capability(),
        skill_studio_core::dto::RestoreCapability::NoInverse,
        "with no inverse, the row must refuse restore rather than name the link an undo would delete"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `opencode_toggle_refuses_a_skill_name_installed_in_two_locations_or_names_the_global_deny_leak`:
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

/// `claude_code_toggle_marks_the_event_failed_when_the_link_write_fails_or_names_the_pending_row`:
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

/// `claude_code_disable_refuses_a_real_directory_or_whole_dir_link_or_names_the_removed_directory`:
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

/// `undo_of_a_failed_recreate_restore_is_refused_or_names_the_live_link_it_would_remove`:
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

/// `codex_switch_writes_the_config_under_codex_home_or_names_the_file_it_wrote_instead`:
/// with `RuntimeScope::codex_home` pointed at a directory distinct from
/// `home/.codex`, a Codex disable must write its `[[skills.config]]` row
/// under that `codex_home`, not under the general home directory, and a
/// fresh scan must see the deployment as disabled.
#[test]
fn codex_switch_writes_the_config_under_codex_home_or_names_the_file_it_wrote_instead() {
    let home = unique_temp_dir("switch_codex_home");
    let codex_home = unique_temp_dir("switch_codex_home_custom");
    std::fs::create_dir_all(&codex_home).unwrap();
    // `RuntimeScope::codex_home` has no canonical form and is checked
    // lexically only (see its doc comment): canonicalize here so a
    // symlinked temp dir (`/var/folders` -> `/private/var/folders` on
    // macOS) still matches what `confine` resolves for the config file's
    // parent.
    let codex_home = codex_home.canonicalize().unwrap();
    // Codex's own harness root, not the universal root: `native_disabled_by`
    // only attributes `DisabledBy::CodexConfig` to a `RootKind::Harness`
    // (Codex) deployment, so the scan assertion below needs one.
    let codex_dir = home.join(CODEX_ROOT_RELATIVE).join("gamma");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(
        codex_dir.join("SKILL.md"),
        "---\nname: gamma\ndescription: a codex-home-scoped skill\n---\nBody.\n",
    )
    .unwrap();
    let rt = runtime_with_codex_home(&home, &codex_home);

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

    let default_config_path = home.join(".codex/config.toml");
    assert!(
        std::fs::metadata(&default_config_path).is_err(),
        "expected no config.toml written under the general home's .codex at {}",
        default_config_path.display()
    );
    let custom_config_path = codex_home.join("config.toml");
    let text = std::fs::read_to_string(&custom_config_path).unwrap_or_else(|e| {
        panic!(
            "expected the row under CODEX_HOME at {}: {e}",
            custom_config_path.display()
        )
    });
    assert!(
        text.contains("[[skills.config]]") && text.contains("gamma/SKILL.md"),
        "expected a disabled row for gamma in {}, got:\n{text}",
        custom_config_path.display()
    );

    let inventory = ops::scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
    let skill = inventory
        .skills
        .iter()
        .find(|s| s.name.0 == "gamma")
        .expect("gamma must still be in the inventory");
    assert!(
        skill.deployments.iter().any(|d| d.disabled_by.is_some()),
        "expected a fresh scan to report gamma disabled after the CODEX_HOME-scoped write"
    );

    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&codex_home).ok();
}

/// `claude_code_disable_records_the_links_real_target_or_names_the_body_undo_would_relink`:
/// a per-skill link retargeted by hand - pointing somewhere other than the
/// canonical universal deployment - must have its disable inverse recreate
/// *that* target, not the universal directory. Recording the canonical
/// directory instead would make undo relink the body at whatever the
/// universal directory holds now, not what the link pointed at before the
/// disable.
#[test]
fn claude_code_disable_records_the_links_real_target_or_names_the_body_undo_would_relink() {
    let home = unique_temp_dir("claude_disable_real_target");
    install_universal_skill(&home, "gamma");
    let canonical_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");

    // A body the link points at instead of the canonical universal
    // deployment - standing in for a link retargeted by hand or left over
    // from a moved skill.
    let foreign_dir = home.join("foreign-target");
    std::fs::create_dir_all(&foreign_dir).unwrap();
    std::fs::write(foreign_dir.join("marker.txt"), "foreign body").unwrap();

    let claude_skills = home.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&claude_skills).unwrap();
    let link = claude_skills.join("gamma");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&foreign_dir, &link).unwrap();

    let rt = runtime_for(&home);
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

    let store = rt
        .ports
        .history
        .open(&rt.scope, HistoryAccess::ReadIfExists)
        .unwrap()
        .expect("the store exists after the write above");
    let row = store.get(&disable.event_id).unwrap().unwrap();
    let recorded_target = row
        .inverse
        .as_ref()
        .and_then(|v| v.get("target"))
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .expect("a recreate_symlink inverse must carry a target");
    assert_eq!(
        recorded_target,
        foreign_dir,
        "the inverse should recreate the link's real target {}, not the canonical dir {}",
        foreign_dir.display(),
        canonical_dir.display()
    );
    assert_ne!(
        recorded_target, canonical_dir,
        "recording the canonical dir would relink undo to the wrong body"
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
        "undo should recreate {}",
        link.display()
    );
    let _ = undo;
    #[cfg(unix)]
    {
        let relinked_target = std::fs::read_link(&link).unwrap();
        assert_eq!(
            relinked_target,
            foreign_dir,
            "undo should relink to the recorded real target {}, not the canonical dir",
            foreign_dir.display()
        );
    }

    std::fs::remove_dir_all(&home).ok();
}
