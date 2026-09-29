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

        telemetry: std::sync::Arc::new(skill_studio_core::ports::NoopTelemetry),
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

        telemetry: std::sync::Arc::new(skill_studio_core::ports::NoopTelemetry),
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

fn claude_request(name: &str, enabled: bool) -> SetHarnessEnabledRequest {
    SetHarnessEnabledRequest {
        skill: SkillName(name.into()),
        harness: AgentId::from(AgentId::CLAUDE_CODE),
        enabled,
        project_path: None,
    }
}

fn claude_settings_path(home: &Path) -> std::path::PathBuf {
    home.join(".claude/settings.json")
}

/// `skillOverrides.<name>` from the fixture's `~/.claude/settings.json`,
/// or `Null` when the file, the object, or the entry is missing.
fn claude_override(home: &Path, name: &str) -> serde_json::Value {
    std::fs::read_to_string(claude_settings_path(home))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .map_or(serde_json::Value::Null, |v| {
            v["skillOverrides"][name].clone()
        })
}

/// The settings file the maintainer's real home already has: another
/// skill's override and unrelated top-level keys that every switch write
/// must keep.
fn seed_claude_settings(home: &Path) {
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(
        claude_settings_path(home),
        r#"{
  "permissions": {"allow": ["Bash(ls)"]},
  "skillOverrides": {"code-review": "user-invocable-only"},
  "enabledPlugins": {"x@y": true}
}"#,
    )
    .unwrap();
}

/// Every entry under `~/.claude/skills` (and the folder itself) as
/// `(path, kind, link target)`, so a test can prove a switch write left the
/// Claude Code skills folder byte-for-byte alone.
fn claude_skills_tree(home: &Path) -> Vec<(String, &'static str, Option<std::path::PathBuf>)> {
    fn walk(path: &Path, out: &mut Vec<(String, &'static str, Option<std::path::PathBuf>)>) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        let display = path.display().to_string();
        if meta.file_type().is_symlink() {
            out.push((display, "link", std::fs::read_link(path).ok()));
        } else if meta.is_dir() {
            out.push((display, "dir", None));
            let mut children: Vec<_> = std::fs::read_dir(path)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            children.sort();
            for child in children {
                walk(&child, out);
            }
        } else {
            out.push((display, "file", None));
        }
    }
    let mut out = Vec::new();
    walk(&home.join(CLAUDE_ROOT_RELATIVE), &mut out);
    out
}

/// The scan's Claude Code deployment of `name` (a row under a Claude Code
/// harness root), if any.
fn claude_deployment(
    inventory: &skill_studio_core::dto::Inventory,
    name: &str,
) -> Option<skill_studio_core::dto::DeploymentDto> {
    inventory
        .skills
        .iter()
        .filter(|s| s.name.0 == name)
        .flat_map(|s| s.deployments.iter())
        .find(|d| {
            d.harness
                .as_ref()
                .is_some_and(|h| h.as_str() == AgentId::CLAUDE_CODE)
        })
        .cloned()
}

/// `each_of_the_three_harness_switch_tests_passes_against_its_fixture_home_or_names_the_wrong_file`:
/// one case per harness, each asserting the exact file `enable-and-links.md`
/// names for that harness.
#[test]
fn each_of_the_three_harness_switch_tests_passes_against_its_fixture_home_or_names_the_wrong_file()
{
    // Claude Code: `skillOverrides.<name>` is set to `"off"` in
    // `~/.claude/settings.json`, then removed again; the per-skill link is
    // left alone both ways.
    {
        let home = unique_temp_dir("switch_claude_code");
        install_universal_skill(&home, "gamma");
        install_claude_link(&home, "gamma");
        let rt = runtime_for(&home);
        let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");

        ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", false)).unwrap();
        assert_eq!(
            claude_override(&home, "gamma"),
            "off",
            "claude code disable should write skillOverrides.gamma = off in {}",
            claude_settings_path(&home).display()
        );
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "claude code disable must leave the link at {} in place",
            link.display()
        );

        ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", true)).unwrap();
        assert_eq!(
            claude_override(&home, "gamma"),
            serde_json::Value::Null,
            "claude code enable should remove skillOverrides.gamma from {}",
            claude_settings_path(&home).display()
        );
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "claude code enable must leave the link at {} in place",
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
}

/// `set_harness_enabled_writes_a_journal_row_before_the_first_path_toggles_or_names_the_missing_step`:
/// a failure on `OpenCode`'s single write still leaves a durable journal row - the
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
            harness: AgentId::from(AgentId::OPEN_CODE),
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
/// `OpenCode`'s `ensure_dir_all`, called after the journal row is recorded to create
/// `~/.config/opencode` on a fresh home but before `write_atomic`, fails. That row
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
            harness: AgentId::from(AgentId::OPEN_CODE),
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

/// `a_remove_symlink_row_without_a_target_is_force_only_or_names_the_row_it_refused_to_read`:
/// a `remove_symlink` inverse recorded before `target` existed (the shape a
/// row written by an older build, or by some other producer, would have)
/// must still read as `RestoreCapability::Yes`, not silently drop to
/// `UnknownKind`; and restoring it must only ever remove the link with
/// `force`, since there is nothing recorded to compare the live link
/// against.
#[test]
fn a_remove_symlink_row_without_a_target_is_force_only_or_names_the_row_it_refused_to_read() {
    let home = unique_temp_dir("remove_symlink_no_target");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);

    let claude_skills = home.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&claude_skills).unwrap();
    let link = claude_skills.join("epsilon");
    let target_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join("epsilon");
    std::fs::create_dir_all(&target_dir).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target_dir, &link).unwrap();

    let id = rt.ports.ids.next_event_id();
    let mut session = skill_studio_core::ports::MutationSession::begin(&rt, &ctx()).unwrap();
    // Old row shape: a `remove_symlink` inverse with no `target` key at
    // all, mirroring `events::remove_symlink_inverse`'s `None` branch (that
    // function is crate-private; this integration test only has the public
    // API).
    let inverse = serde_json::json!({ "op": "remove_symlink", "path": &link });
    let draft = skill_studio_core::events::EventDraft {
        kind: skill_studio_core::events::EventKind::HarnessEnable,
        skill: SkillName("epsilon".into()),
        harness: Some(AgentId::from(AgentId::CLAUDE_CODE)),
        scope: Some("global".to_string()),
        project_path: None,
        payload: serde_json::json!({}),
        inverse: Some(inverse),
        backup_dir: None,
    };
    session.store.record(&session.guard, &id, &draft).unwrap();
    session
        .store
        .finish(
            &session.guard,
            &id,
            skill_studio_core::events::EventStatus::Done,
            None,
        )
        .unwrap();
    drop(session);

    let store = rt
        .ports
        .history
        .open(&rt.scope, HistoryAccess::ReadIfExists)
        .unwrap()
        .expect("the hand-recorded row's store exists after the write above");
    let row = store.get(&id).unwrap().unwrap();
    assert_eq!(
        row.restore_capability(),
        skill_studio_core::dto::RestoreCapability::Yes,
        "a remove_symlink row missing target must still read as restorable, not fall to UnknownKind"
    );
    drop(store);

    let no_force_err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: id.clone(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(
        no_force_err.code,
        skill_studio_core::ErrorCode::DriftConflict,
        "restoring a target-less remove_symlink row without force must be refused, not silently no-op or panic"
    );
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "without force the link must still be at {}",
        link.display()
    );

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: id.clone(),
            force: true,
        },
    )
    .unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "with force the link at {} must be removed",
        link.display()
    );

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

/// `claude_code_undo_of_undo_of_a_created_link_removes_it_again_or_names_the_stale_inverse`:
/// an enable for a skill Claude Code cannot see creates the per-skill link
/// and journals `remove_symlink` as its inverse; undoing it removes the link
/// and must journal `recreate_symlink` (not another `remove_symlink`), so
/// undoing the undo puts the link back instead of trying to remove an
/// already-absent one.
#[test]
fn claude_code_undo_of_undo_of_a_created_link_removes_it_again_or_names_the_stale_inverse() {
    let home = unique_temp_dir("claude_undo_of_undo");
    install_universal_skill(&home, "gamma");
    std::fs::create_dir_all(home.join(CLAUDE_ROOT_RELATIVE)).unwrap();
    let rt = runtime_for(&home);
    let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");

    let enable = ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", true)).unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "enable should create {}",
        link.display()
    );

    let undo = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: enable.event_id.clone(),
            force: false,
        },
    )
    .unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "undoing the enable should remove {}",
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
        std::fs::symlink_metadata(&link).is_ok(),
        "undoing the undo should recreate the link at {}",
        link.display()
    );

    let store = rt
        .ports
        .history
        .open(&rt.scope, HistoryAccess::ReadIfExists)
        .unwrap()
        .expect("the store exists after the writes above");
    let enable_row = store.get(&enable.event_id).unwrap().unwrap();
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
        inverse_op(&enable_row).as_deref(),
        Some("remove_symlink"),
        "the enable created the link, so its inverse should remove it"
    );
    assert_eq!(
        inverse_op(&undo_row).as_deref(),
        Some("recreate_symlink"),
        "undoing the enable removed the link, so its inverse must recreate it, not remove it again"
    );
    assert_eq!(
        inverse_op(&undo_of_undo_row).as_deref(),
        Some("remove_symlink"),
        "undoing the undo recreated the link, so its inverse must remove it"
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

/// `project_scoped_claude_code_disable_is_refused_and_writes_nothing_or_names_the_file_it_changed`:
/// Claude Code's `skillOverrides` live in the global
/// `~/.claude/settings.json` and apply in every project, so a
/// project-scoped off would silently switch the skill off everywhere. It
/// must be refused, with neither settings file written and both links left
/// in place.
#[test]
fn project_scoped_claude_code_disable_is_refused_and_writes_nothing_or_names_the_file_it_changed() {
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

    let err = ops::set_harness_enabled(
        &rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            project_path: Some(project.clone()),
            ..claude_request("gamma", false)
        },
    )
    .unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Unsupported);
    assert!(
        err.message.contains("settings.json"),
        "the refusal should say the switch lives in settings.json, got: {}",
        err.message
    );
    for settings in [
        claude_settings_path(&home),
        project.join(".claude/settings.json"),
    ] {
        assert!(
            std::fs::symlink_metadata(&settings).is_err(),
            "a refused project-scoped off must not write {}",
            settings.display()
        );
    }
    for link in [&global_link, &project_link] {
        assert!(
            std::fs::symlink_metadata(link).is_ok(),
            "a refused project-scoped off must leave the link at {}",
            link.display()
        );
    }

    std::fs::remove_dir_all(&home).ok();
}

/// `disabling_a_skill_already_off_in_claude_settings_records_no_undo_or_names_the_bytes_undo_would_rewrite`:
/// `gamma` is already `"off"` in `skillOverrides`; switching it off again is
/// a no-op, so the file's bytes stay as they were and the journal row
/// carries no inverse. Restoring that row must be refused rather than
/// rewriting a file the no-op never touched.
#[test]
fn disabling_a_skill_already_off_in_claude_settings_records_no_undo_or_names_the_bytes_undo_would_rewrite(
) {
    let home = unique_temp_dir("claude_noop_disable");
    install_universal_skill(&home, "gamma");
    install_claude_link(&home, "gamma");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let before = "{\"skillOverrides\":{\"gamma\":\"off\"}}";
    std::fs::write(claude_settings_path(&home), before).unwrap();
    let rt = runtime_for(&home);

    let disable = ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", false)).unwrap();
    assert_eq!(
        std::fs::read_to_string(claude_settings_path(&home)).unwrap(),
        before,
        "a no-op off must not rewrite settings.json"
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
        "a no-op toggle must record no inverse, not one that would rewrite settings.json"
    );
    assert_eq!(
        row.restore_capability(),
        skill_studio_core::dto::RestoreCapability::NoInverse,
        "with no inverse, the row must refuse restore"
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

/// The three Claude Code layouts the scanner produces a Claude row for.
#[derive(Debug, Clone, Copy)]
enum ClaudeLayout {
    /// `~/.claude/skills` is a real folder holding a relative per-skill
    /// link `../../.agents/skills/<name>`.
    PerSkillLink,
    /// `~/.claude/skills` is itself a link to `~/.agents/skills`.
    WholeFolderLink,
    /// `~/.claude/skills/<name>` is a real copy of the skill.
    RealCopy,
}

fn install_claude_layout(home: &Path, name: &str, layout: ClaudeLayout) {
    let claude_skills = home.join(CLAUDE_ROOT_RELATIVE);
    match layout {
        ClaudeLayout::PerSkillLink => {
            install_universal_skill(home, name);
            std::fs::create_dir_all(&claude_skills).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(
                Path::new("../../.agents/skills").join(name),
                claude_skills.join(name),
            )
            .unwrap();
        }
        ClaudeLayout::WholeFolderLink => {
            install_universal_skill(home, name);
            std::fs::create_dir_all(home.join(".claude")).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(home.join(UNIVERSAL_ROOT_RELATIVE), &claude_skills).unwrap();
        }
        ClaudeLayout::RealCopy => {
            let dir = claude_skills.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: a real Claude Code copy\n---\nBody.\n"),
            )
            .unwrap();
        }
    }
}

/// `claude_code_off_writes_skill_overrides_for_every_layout_and_survives_rescan_and_restart_or_names_the_layout_that_lost_it`:
/// for a per-skill link, a whole-folder link, and a real copy alike, the
/// off switch writes `skillOverrides.<name> = "off"`, keeps the other
/// skill's `"user-invocable-only"` and every other key, and changes nothing
/// under `~/.claude/skills`. A rescan and a fresh runtime (an app restart)
/// both still list the Claude Code row, marked
/// `DisabledBy::ClaudeSkillOverrides`; the on switch clears it again.
#[test]
fn claude_code_off_writes_skill_overrides_for_every_layout_and_survives_rescan_and_restart_or_names_the_layout_that_lost_it(
) {
    for layout in [
        ClaudeLayout::PerSkillLink,
        ClaudeLayout::WholeFolderLink,
        ClaudeLayout::RealCopy,
    ] {
        let home = unique_temp_dir(&format!("claude_off_{layout:?}"));
        install_claude_layout(&home, "gamma", layout);
        seed_claude_settings(&home);
        let tree_before = claude_skills_tree(&home);
        let rt = runtime_for(&home);

        let before = ops::scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
        let row = claude_deployment(&before, "gamma").unwrap_or_else(|| {
            panic!("{layout:?}: fixture setup should give gamma a Claude Code row")
        });
        assert_eq!(row.disabled_by, None, "{layout:?}: gamma should start on");

        ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", false))
            .unwrap_or_else(|e| panic!("{layout:?}: off should succeed, got: {}", e.message));
        assert_eq!(
            claude_override(&home, "gamma"),
            "off",
            "{layout:?}: off should write skillOverrides.gamma = off"
        );
        assert_eq!(
            claude_override(&home, "code-review"),
            "user-invocable-only",
            "{layout:?}: off dropped another skill's override"
        );
        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(claude_settings_path(&home)).unwrap())
                .unwrap();
        assert!(
            settings.get("permissions").is_some() && settings.get("enabledPlugins").is_some(),
            "{layout:?}: off lost a top-level settings key: {settings}"
        );
        assert_eq!(
            claude_skills_tree(&home),
            tree_before,
            "{layout:?}: off changed something under ~/.claude/skills"
        );

        let rescanned = ops::scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
        let restarted = ops::scan(&runtime_for(&home), &ctx(), &ScanRequest::default()).unwrap();
        for (when, inventory) in [("rescan", &rescanned), ("restart", &restarted)] {
            let row = claude_deployment(inventory, "gamma").unwrap_or_else(|| {
                panic!("{layout:?}: the Claude Code row vanished after the {when}")
            });
            assert_eq!(
                row.disabled_by,
                Some(skill_studio_core::harness::DisabledBy::ClaudeSkillOverrides),
                "{layout:?}: the {when} should mark the Claude Code row off by skillOverrides"
            );
        }

        ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", true))
            .unwrap_or_else(|e| panic!("{layout:?}: on should succeed, got: {}", e.message));
        assert_eq!(
            claude_override(&home, "gamma"),
            serde_json::Value::Null,
            "{layout:?}: on should remove skillOverrides.gamma"
        );
        assert_eq!(
            claude_override(&home, "code-review"),
            "user-invocable-only",
            "{layout:?}: on dropped another skill's override"
        );
        assert_eq!(
            claude_skills_tree(&home),
            tree_before,
            "{layout:?}: on changed something under ~/.claude/skills"
        );
        let after = ops::scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
        assert_eq!(
            claude_deployment(&after, "gamma").map(|d| d.disabled_by),
            Some(None),
            "{layout:?}: after on, the Claude Code row should be on again"
        );

        std::fs::remove_dir_all(&home).ok();
    }
}

/// `claude_code_on_restores_the_override_the_off_replaced_or_names_the_value_it_dropped`:
/// `gamma` starts as `"user-invocable-only"`. Off replaces that with
/// `"off"`; on must put `"user-invocable-only"` back, not delete the key and
/// lose the user's own setting.
#[test]
fn claude_code_on_restores_the_override_the_off_replaced_or_names_the_value_it_dropped() {
    let home = unique_temp_dir("claude_on_restores_override");
    install_universal_skill(&home, "gamma");
    install_claude_link(&home, "gamma");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(
        claude_settings_path(&home),
        r#"{"skillOverrides": {"gamma": "user-invocable-only"}}"#,
    )
    .unwrap();
    let rt = runtime_for(&home);

    ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", false)).unwrap();
    assert_eq!(claude_override(&home, "gamma"), "off");
    ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", true)).unwrap();
    assert_eq!(
        claude_override(&home, "gamma"),
        "user-invocable-only",
        "on should restore the value off replaced, not drop it"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `claude_code_on_for_a_universal_skill_claude_cannot_see_links_it_or_names_the_missing_row`:
/// `~/.claude/skills` is a real folder with no entry for `gamma`, so Claude
/// Code cannot see the Universal skill: the scan lists `claude-code` among
/// its disabled readers and there is no Claude Code row. On creates the
/// per-skill link (and does not create `settings.json`); the next scan has
/// a Claude Code row that is on and no longer lists `claude-code` as a
/// disabled reader. This is also how a link an older build removed as its
/// off switch comes back.
#[test]
fn claude_code_on_for_a_universal_skill_claude_cannot_see_links_it_or_names_the_missing_row() {
    let home = unique_temp_dir("claude_on_links_universal");
    install_universal_skill(&home, "gamma");
    std::fs::create_dir_all(home.join(CLAUDE_ROOT_RELATIVE)).unwrap();
    let rt = runtime_for(&home);
    let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");
    let reads_claude = |inventory: &skill_studio_core::dto::Inventory| {
        inventory
            .skills
            .iter()
            .filter(|s| s.name.0 == "gamma")
            .flat_map(|s| s.deployments.iter())
            .any(|d| {
                d.disabled_readers
                    .iter()
                    .any(|r| r.as_str() == AgentId::CLAUDE_CODE)
            })
    };

    let before = ops::scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
    assert!(
        claude_deployment(&before, "gamma").is_none() && reads_claude(&before),
        "fixture setup: gamma should have no Claude Code row and claude-code as a disabled reader"
    );

    ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", true))
        .unwrap_or_else(|e| panic!("on should link gamma, got: {}", e.message));
    #[cfg(unix)]
    assert_eq!(
        std::fs::read_link(&link).ok(),
        Some(home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma")),
        "on should create a per-skill link at {} to the Universal folder",
        link.display()
    );
    assert!(
        std::fs::symlink_metadata(claude_settings_path(&home)).is_err(),
        "on for a skill with no override must not create settings.json"
    );

    let after = ops::scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
    assert_eq!(
        claude_deployment(&after, "gamma").map(|d| d.disabled_by),
        Some(None),
        "after on, gamma should have a Claude Code row that is on"
    );
    assert!(
        !reads_claude(&after),
        "after on, claude-code should no longer be a disabled reader of gamma"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `undo_of_a_failed_recreate_restore_is_refused_or_names_the_live_link_it_would_remove`:
/// an enable creates the Claude Code link (`E1`, inverse `remove_symlink`);
/// undoing it (`R1`) removes the link and journals a `Recreate` inverse. A
/// real directory occupies the link's slot before `R1` is undone, so that
/// undo's `fs.symlink` call fails: `R1` must stay restorable (its claim was
/// released, not consumed) and the new restore row `R2` must finish
/// `failed`. `restore_event` on `R2` must then be refused, naming `R2`'s
/// status - `RestoreCapability::NotCompleted` is what makes that refusal
/// possible; without it, undoing `R2` would apply `R2`'s `remove_symlink`
/// inverse to the occupant directory, deleting state `R2` never touched.
#[test]
fn undo_of_a_failed_recreate_restore_is_refused_or_names_the_live_link_it_would_remove() {
    let home = unique_temp_dir("claude_undo_failed_recreate");
    install_universal_skill(&home, "gamma");
    let rt = runtime_for(&home);
    let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");

    let enable = ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", true)).unwrap();
    let unlink = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: enable.event_id.clone(),
            force: false,
        },
    )
    .unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "undoing the enable should remove {}",
        link.display()
    );

    // The occupant: a real directory sits where `R1`'s `Recreate` inverse
    // wants to put the link back.
    std::fs::create_dir_all(&link).unwrap();
    std::fs::write(link.join("SKILL.md"), "---\nname: gamma\n---\n").unwrap();

    let undo_err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: unlink.restore_event_id.clone(),
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
    let unlink_row = store.get(&unlink.restore_event_id).unwrap().unwrap();
    assert_eq!(
        unlink_row.restore_capability(),
        skill_studio_core::dto::RestoreCapability::Yes,
        "the failed undo must release its claim, leaving R1 restorable again"
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
        .expect("the undo's own restore row R2 must be recorded and finished failed");
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
        "expected the refusal to name R2's status (failed), got: {}",
        redo_err.message
    );
    assert!(
        std::fs::metadata(&link).is_ok_and(|m| m.is_dir()),
        "undoing R2 must still be refused, so the occupant directory at {} must remain",
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
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&codex_home).unwrap();
    // `RuntimeScope::codex_home` has no canonical form and is checked
    // lexically only (see its doc comment): canonicalize here so a
    // symlinked temp dir (`/var/folders` -> `/private/var/folders` on
    // macOS) still matches what `confine` resolves for the config file's
    // parent.
    let codex_home = codex_home.canonicalize().unwrap();
    // Codex's own harness root, not the universal root: `native_disabled_by`
    // only attributes `DisabledBy::CodexConfig` to a `RootKind::Harness`
    // (Codex) deployment, so the scan assertion below needs one. With
    // `CODEX_HOME` set, Codex reads `$CODEX_HOME/skills`, not `~/.codex/skills`.
    let codex_dir = codex_home.join("skills").join("gamma");
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

/// `undo_of_a_symlink_refuses_a_parked_target_or_names_the_dangling_link_it_created`:
/// a `Recreate` inverse must refuse to run once the target it would point at
/// is gone - parked, or moved out from under it between the unlink and its
/// undo - rather than planting a dangling link and calling it a success.
#[test]
fn undo_of_a_symlink_refuses_a_parked_target_or_names_the_dangling_link_it_created() {
    let home = unique_temp_dir("undo_parked_target");
    install_universal_skill(&home, "gamma");
    let rt = runtime_for(&home);
    let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");
    let canonical_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");

    let enable = ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", true)).unwrap();
    let unlink = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: enable.event_id.clone(),
            force: false,
        },
    )
    .unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "undoing the enable should remove {}",
        link.display()
    );

    // Park the canonical directory the unlink's inverse recorded as its
    // recreate target - simulating the skill being parked or moved between
    // the unlink and its undo.
    let parked_dir = home.join("parked-gamma");
    std::fs::rename(&canonical_dir, &parked_dir).unwrap();

    let undo_err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: unlink.restore_event_id.clone(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(undo_err.code, skill_studio_core::ErrorCode::DriftConflict);
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "the refused undo must not plant a dangling link at {}",
        link.display()
    );

    // `force` must not bypass this refusal either: there is no live state
    // to force past, only a target that no longer exists.
    let forced_err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: unlink.restore_event_id.clone(),
            force: true,
        },
    )
    .unwrap_err();
    assert_eq!(forced_err.code, skill_studio_core::ErrorCode::DriftConflict);
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "force must not plant a dangling link at {} either",
        link.display()
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `undo_of_a_symlink_refuses_to_delete_a_regular_file_or_names_the_file_it_removed`:
/// a `Remove` inverse - the undo of an enable that created a link - must
/// refuse to delete whatever sits at the link's path once that path is no
/// longer a symlink, even with `force`: deleting a regular file the user put
/// there would destroy bytes this event never wrote.
#[test]
fn undo_of_a_symlink_refuses_to_delete_a_regular_file_or_names_the_file_it_removed() {
    let home = unique_temp_dir("undo_refuses_regular_file");
    install_universal_skill(&home, "gamma");
    let rt = runtime_for(&home);
    let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");

    let enable = ops::set_harness_enabled(
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
        "enable should create {}",
        link.display()
    );

    // Swap the link the enable created for a regular file - standing in for
    // the user replacing it by hand between the enable and the undo.
    std::fs::remove_file(&link).unwrap();
    let file_bytes = b"not a symlink, do not delete me".to_vec();
    std::fs::write(&link, &file_bytes).unwrap();

    let undo_err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: enable.event_id.clone(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(undo_err.code, skill_studio_core::ErrorCode::DriftConflict);
    assert_eq!(
        std::fs::read(&link).unwrap(),
        file_bytes,
        "the refused undo must leave the regular file's bytes untouched"
    );

    // `force` must not bypass this refusal either: a non-symlink is never
    // removed by this inverse, force or not.
    let forced_err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: enable.event_id.clone(),
            force: true,
        },
    )
    .unwrap_err();
    assert_eq!(forced_err.code, skill_studio_core::ErrorCode::DriftConflict);
    assert_eq!(
        std::fs::read(&link).unwrap(),
        file_bytes,
        "force must leave the regular file's bytes untouched too"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `codex_toggle_event_names_the_project_scope_or_names_the_row_filed_as_global`:
/// disabling a project-scoped skill for Codex must journal the row with
/// `scope: "project"` and the project's path, the same way the Claude Code
/// arm already does - not hardcode `"global"`/`None` regardless of which
/// scope the write actually touched.
#[test]
fn codex_toggle_event_names_the_project_scope_or_names_the_row_filed_as_global() {
    let home = unique_temp_dir("codex_project_scope_event");
    let project = home.join("proj");
    install_project_universal_skill(&project, "gamma");
    let rt = runtime_with(&home, vec![project.clone()], Arc::new(RealFs::new()));

    let disable = ops::set_harness_enabled(
        &rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            skill: SkillName("gamma".into()),
            harness: AgentId::from(AgentId::CODEX),
            enabled: false,
            project_path: Some(project.clone()),
        },
    )
    .unwrap();

    let events = ops::list_events(
        &rt,
        &ctx(),
        &skill_studio_core::dto::ListEventsRequest::default(),
    )
    .unwrap();
    let row = events
        .iter()
        .find(|e| e.id == disable.event_id)
        .expect("the Codex disable's own row must be in the history");
    assert_eq!(
        row.scope.as_deref(),
        Some("project"),
        "expected the project-scoped Codex row to carry scope \"project\", not \"global\""
    );
    assert_eq!(
        row.project_path.as_deref(),
        Some(project.as_path()),
        "expected the row to carry {}, not no project at all",
        project.display()
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `a_no_op_codex_toggle_records_no_undo_or_names_the_bytes_undo_would_rewrite`:
/// disabling an already-disabled Codex skill touches no bytes, so its
/// journal row must carry no inverse - like `set_claude_code_switch`'s
/// no-op branch. A `restore_backup` inverse recorded over identical bytes
/// would let `undo` "revert" a mutation that never happened, consuming the
/// claim on the real previous change underneath it.
#[test]
fn a_no_op_codex_toggle_records_no_undo_or_names_the_bytes_undo_would_rewrite() {
    let home = unique_temp_dir("codex_no_op_toggle");
    install_universal_skill(&home, "gamma");
    let rt = runtime_for(&home);
    let config_path = home.join(".codex/config.toml");

    let first = ops::set_harness_enabled(
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
    let text_after_first = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        text_after_first.contains("[[skills.config]]"),
        "the first disable should write a row, got:\n{text_after_first}"
    );

    let second = ops::set_harness_enabled(
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
    let text_after_second = std::fs::read_to_string(&config_path).unwrap();
    assert_eq!(
        text_after_second, text_after_first,
        "disabling an already-disabled skill must not rewrite the file's bytes"
    );

    let store = rt
        .ports
        .history
        .open(&rt.scope, HistoryAccess::ReadIfExists)
        .unwrap()
        .expect("the store exists after the writes above");
    let second_row = store.get(&second.event_id).unwrap().unwrap();
    assert!(
        second_row.inverse.is_none(),
        "the no-op toggle must record no inverse, got: {:?}",
        second_row.inverse
    );
    assert_eq!(
        second_row.restore_capability(),
        skill_studio_core::dto::RestoreCapability::NoInverse
    );

    let second_undo_err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: second.event_id.clone(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(
        second_undo_err.code,
        skill_studio_core::ErrorCode::Unsupported
    );

    // The no-op second event must not stand in the way of undoing the real
    // change: restoring the first event - the last one that actually
    // carries an inverse - must still succeed.
    let undo = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: first.event_id.clone(),
            force: false,
        },
    )
    .unwrap();
    assert_eq!(undo.reverted_event_id, first.event_id);
    let text_after_undo = std::fs::read_to_string(&config_path).unwrap_or_default();
    assert!(
        !text_after_undo.contains("[[skills.config]]"),
        "undoing the first disable should remove its row, restoring the pre-disable (absent) config, got:\n{text_after_undo}"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `undo_of_a_remove_row_against_a_relative_live_link_records_a_resolved_inverse_or_names_the_target_it_recorded`:
/// undoing a hand-recorded `remove_symlink` row against a relative live
/// link must itself record a resolved (absolute) `recreate_symlink`
/// inverse, not the raw `read_link` text - otherwise undoing *that* restore
/// hits `confine`'s "path must be absolute without .." refusal instead of
/// recreating the link.
#[test]
fn undo_of_a_remove_row_against_a_relative_live_link_records_a_resolved_inverse_or_names_the_target_it_recorded(
) {
    let home = unique_temp_dir("remove_row_relative_live_link");
    install_universal_skill(&home, "zeta");
    let canonical_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join("zeta");

    let claude_skills = home.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&claude_skills).unwrap();
    let link = claude_skills.join("zeta");
    #[cfg(unix)]
    std::os::unix::fs::symlink(Path::new("../../.agents/skills/zeta"), &link).unwrap();

    let rt = runtime_for(&home);

    // Hand-record a `remove_symlink` inverse whose `target` is the
    // resolved absolute path, the shape `set_claude_code_switch` writes
    // when an enable creates a link.
    let id = rt.ports.ids.next_event_id();
    let mut session = skill_studio_core::ports::MutationSession::begin(&rt, &ctx()).unwrap();
    let inverse = serde_json::json!({
        "op": "remove_symlink",
        "path": &link,
        "target": &canonical_dir,
    });
    let draft = skill_studio_core::events::EventDraft {
        kind: skill_studio_core::events::EventKind::HarnessEnable,
        skill: SkillName("zeta".into()),
        harness: Some(AgentId::from(AgentId::CLAUDE_CODE)),
        scope: Some("global".to_string()),
        project_path: None,
        payload: serde_json::json!({}),
        inverse: Some(inverse),
        backup_dir: None,
    };
    session.store.record(&session.guard, &id, &draft).unwrap();
    session
        .store
        .finish(
            &session.guard,
            &id,
            skill_studio_core::events::EventStatus::Done,
            None,
        )
        .unwrap();
    drop(session);

    // Restoring the hand-recorded row must succeed without force: the
    // drift compare resolves the relative live link to the same absolute
    // path as the recorded target.
    let first_undo = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: id.clone(),
            force: false,
        },
    )
    .unwrap_or_else(|e| {
        panic!("restoring the hand-recorded remove_symlink row must succeed, got: {e}")
    });
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "the restore should remove {}",
        link.display()
    );

    // Restoring the restore row's own inverse (a recreate) must succeed and
    // recreate the link, not fail with DriftConflict or "path must be
    // absolute without ..".
    let second_undo = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: first_undo.restore_event_id.clone(),
            force: false,
        },
    )
    .unwrap_or_else(|e| {
        panic!("restoring the first restore's own inverse must recreate the link, got: {e}")
    });
    assert_eq!(second_undo.reverted_event_id, first_undo.restore_event_id);
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "the second restore should recreate {}",
        link.display()
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `undo_of_a_claude_code_off_restores_settings_json_byte_for_byte_or_names_the_bytes_it_changed`:
/// the off's inverse is the backup of `settings.json` taken before the
/// write, so undoing it gives back the exact bytes - formatting, key order,
/// and the other skill's override included.
#[test]
fn undo_of_a_claude_code_off_restores_settings_json_byte_for_byte_or_names_the_bytes_it_changed() {
    let home = unique_temp_dir("claude_undo_off");
    install_universal_skill(&home, "gamma");
    install_claude_link(&home, "gamma");
    seed_claude_settings(&home);
    let before = std::fs::read_to_string(claude_settings_path(&home)).unwrap();
    let rt = runtime_for(&home);

    let off = ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", false)).unwrap();
    assert_eq!(claude_override(&home, "gamma"), "off");
    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: off.event_id.clone(),
            force: false,
        },
    )
    .unwrap_or_else(|e| panic!("undo of the off should succeed, got: {}", e.message));
    assert_eq!(
        std::fs::read_to_string(claude_settings_path(&home)).unwrap(),
        before,
        "undo of the off should restore settings.json exactly"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: `~/.claude/settings.json` is a link into a dotfiles folder, and
/// the user turns a skill off in Claude Code, then undoes it. Expect the
/// write and the undo to land in the dotfiles file, the link to stay a link,
/// and the file to keep its 0600 mode. Catches a rename that replaces the
/// link with a regular file (0755, copied from the link), which cuts the
/// settings off from the user's dotfiles.
#[test]
fn claude_code_off_writes_through_a_linked_settings_json_and_keeps_the_link_and_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let home = unique_temp_dir("claude_linked_settings");
    install_universal_skill(&home, "gamma");
    install_claude_link(&home, "gamma");
    let dotfiles = home.join("dotfiles/claude-settings.json");
    std::fs::create_dir_all(dotfiles.parent().unwrap()).unwrap();
    let before = "{\"permissions\":{\"allow\":[\"Bash(ls)\"]}}";
    std::fs::write(&dotfiles, before).unwrap();
    std::fs::set_permissions(&dotfiles, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::os::unix::fs::symlink(&dotfiles, claude_settings_path(&home)).unwrap();
    let rt = runtime_for(&home);

    let off = ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", false)).unwrap();

    let settings = claude_settings_path(&home);
    assert!(
        std::fs::symlink_metadata(&settings)
            .unwrap()
            .file_type()
            .is_symlink(),
        "settings.json must still be a link"
    );
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&dotfiles).unwrap()).unwrap();
    assert_eq!(written["skillOverrides"]["gamma"], "off");
    assert_eq!(
        std::fs::metadata(&dotfiles).unwrap().permissions().mode() & 0o777,
        0o600,
        "the linked file must keep its own mode"
    );

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: off.event_id,
            force: false,
        },
    )
    .unwrap();
    assert!(std::fs::symlink_metadata(&settings)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_to_string(&dotfiles).unwrap(), before);

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: `~/.claude/settings.json` is a link to a file that no longer
/// exists, and the user turns a skill off. Expect a refusal that names the
/// dangling link, with the link left in place. Catches a write that turns
/// the dangling link into a new regular file.
#[test]
fn claude_code_off_refuses_a_dangling_settings_json_link_and_leaves_it() {
    let home = unique_temp_dir("claude_dangling_settings");
    install_universal_skill(&home, "gamma");
    install_claude_link(&home, "gamma");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let missing = home.join("dotfiles/missing.json");
    std::os::unix::fs::symlink(&missing, claude_settings_path(&home)).unwrap();
    let rt = runtime_for(&home);

    let err = ops::set_harness_enabled(&rt, &ctx(), &claude_request("gamma", false)).unwrap_err();

    assert!(err.message.contains("does not exist"), "{}", err.message);
    assert_eq!(
        std::fs::read_link(claude_settings_path(&home)).unwrap(),
        missing
    );

    std::fs::remove_dir_all(&home).ok();
}
