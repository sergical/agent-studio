// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so the
// same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk integration tests for `ops::install` and `ops::install_preferences`.
//!
//! Follows `park_and_unpark.rs`'s pattern: `skill-studio-host`'s real
//! adapters, since `Copy`'s stage/swap and `Dotagents`/`SkillsSh`'s CLI
//! invocation both write real bytes a fake filesystem can't stand in for.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use skill_studio_core::dto::{
    InstallFile, InstallHarnessResult, InstallLinkMode, InstallMethod, InstallOutcome,
    InstallRequest, ListEventsRequest, RestoreRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{AgentId, RootScope, SkillName};
use skill_studio_core::ops;
use skill_studio_core::ports::{
    CancelToken, MutationSession, Ports, ProcessOutput, ProcessSpawner, ProcessSpec, Runtime,
};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FailingFs, FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";

/// Stands in for `npx skills add <source> ...` / `npx -y @sentry/dotagents
/// add <source> ...`: writes a minimal `SKILL.md` under
/// `<cwd or home>/.agents/skills/<skill>` on the real filesystem, the same
/// shape the real CLI leaves. Named by parsing the `--skill`/`--name` flag
/// out of argv (per F3, neither builder puts the skill name last), and
/// falls back to `home` for the process cwd, since a global-scope install
/// (either method) never sets one - a project scope sets the process cwd to
/// the project directory itself for both `Dotagents` and `SkillsSh`; neither
/// CLI has a `--project`/`--cwd` flag that carries the target instead.
///
/// R3: when argv carries `--agent claude-code`, also creates
/// `<cwd or home>/.claude/skills/<skill>` as a real symlink into the
/// universal dir it just wrote - the same double-write the real `npx
/// skills add ... --agent claude-code` makes, which `link_claude_code`
/// must tolerate instead of failing on `EEXIST`.
///
/// R5: records every call's argv and cwd (`recorded`), so a test can assert
/// the exact shape `cli_args_and_cwd` built without duplicating its own
/// logic to predict it.
///
/// R3 (round 1): `trace_files`, when set, replaces the single hardcoded
/// `SKILL.md` with exactly the files a recorded (or hand-built) CLI trace
/// names, each written verbatim under the skill's universal-root directory -
/// lets a trace-parity test assert the resulting tree byte for byte instead
/// of only the argv the fake never actually exercises against disk.
struct FakeNpxSpawner {
    home: PathBuf,
    recorded: Mutex<Vec<(Vec<String>, Option<PathBuf>)>>,
    trace_files: Option<Vec<(PathBuf, String)>>,
}

impl FakeNpxSpawner {
    fn new(home: PathBuf) -> Self {
        FakeNpxSpawner {
            home,
            recorded: Mutex::new(Vec::new()),
            trace_files: None,
        }
    }

    fn with_trace_files(home: PathBuf, trace_files: Vec<(PathBuf, String)>) -> Self {
        FakeNpxSpawner {
            home,
            recorded: Mutex::new(Vec::new()),
            trace_files: Some(trace_files),
        }
    }
}

impl ProcessSpawner for FakeNpxSpawner {
    fn run(
        &self,
        spec: &ProcessSpec,
        _cancel: &dyn CancelToken,
    ) -> Result<ProcessOutput, skill_studio_core::CoreError> {
        assert_eq!(spec.program, "npx");
        self.recorded
            .lock()
            .unwrap()
            .push((spec.args.clone(), spec.cwd.clone()));
        let skill = spec
            .args
            .iter()
            .position(|a| a == "--skill" || a == "--name")
            .and_then(|i| spec.args.get(i + 1))
            .expect("--skill or --name flag with a value")
            .clone();
        let cwd = spec.cwd.clone().unwrap_or_else(|| self.home.clone());
        // The real CLI copies into the one folder when every `--agent` shares
        // it (`uniqueDirs.size <= 1`), and writes no shared copy.
        let agent_folders: std::collections::BTreeSet<&str> = spec
            .args
            .windows(2)
            .filter(|w| w[0] == "--agent")
            .map(|w| match w[1].as_str() {
                "claude-code" => ".claude/skills",
                "pi" => ".pi/skills",
                _ => UNIVERSAL_ROOT_RELATIVE,
            })
            .collect();
        let single_folder = agent_folders.len() == 1;
        let dir = match agent_folders.iter().next() {
            Some(folder) if single_folder => cwd.join(folder).join(&skill),
            _ => cwd.join(UNIVERSAL_ROOT_RELATIVE).join(&skill),
        };
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(trace_files) = &self.trace_files {
            for (relative_path, content) in trace_files {
                let path = dir.join(relative_path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(&path, content).unwrap();
            }
        } else {
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {skill}\ndescription: installed by a fake CLI\n---\nBody.\n"),
            )
            .unwrap();
        }
        let has_claude_code_agent = spec
            .args
            .windows(2)
            .any(|w| w[0] == "--agent" && w[1] == "claude-code");
        if has_claude_code_agent && !single_folder {
            let claude_dir = cwd.join(".claude").join("skills");
            std::fs::create_dir_all(&claude_dir).unwrap();
            let link_path = claude_dir.join(&skill);
            if std::fs::symlink_metadata(&link_path).is_err() {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&dir, &link_path).unwrap();
            }
        }
        Ok(ProcessOutput {
            status: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        })
    }
}

fn runtime_with(
    home: &std::path::Path,
    fs: Arc<dyn skill_studio_core::ports::ScopeFs>,
    spawner: Option<Arc<dyn ProcessSpawner>>,
) -> Runtime {
    runtime_in(&RuntimeScope::fixture(home), home, fs, spawner)
}

fn runtime_in(
    scope: &RuntimeScope,
    home: &std::path::Path,
    fs: Arc<dyn skill_studio_core::ports::ScopeFs>,
    spawner: Option<Arc<dyn ProcessSpawner>>,
) -> Runtime {
    let history_root = home.join(".history");
    let db_path = history_root.join("events.sqlite3");
    let ports = Ports {
        fs,
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(db_path)),
        sink: Arc::new(RecordingSink::default()),
        spawner,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),

        telemetry: std::sync::Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    Runtime::new(scope, ports).unwrap()
}

fn runtime_for(home: &std::path::Path) -> Runtime {
    runtime_with(
        home,
        Arc::new(RealFs::new()),
        Some(Arc::new(FakeNpxSpawner::new(home.to_path_buf()))),
    )
}

/// Today's default set: the shared folder plus a Claude Code link.
fn universal_and_claude() -> Vec<AgentId> {
    vec![
        AgentId::from("universal"),
        AgentId::from(AgentId::CLAUDE_CODE),
    ]
}

fn copy_request(skill: &str) -> InstallRequest {
    InstallRequest {
        skill: SkillName(skill.to_string()),
        method: InstallMethod::Copy,
        scope: RootScope::Global,
        harnesses: universal_and_claude(),
        files: vec![InstallFile {
            relative_path: PathBuf::from("SKILL.md"),
            contents: format!("---\nname: {skill}\ndescription: a copied skill\n---\nBody.\n")
                .into_bytes(),
            mode: None,
        }],
        source: None,
        trust_identity: None,
        trust_confirmed: false,
        save_as_preference: true,
        link_mode: InstallLinkMode::Link,
        destination: skill_studio_core::identity::SkillDestination::Universal,
    }
}

fn cli_request(skill: &str, method: InstallMethod) -> InstallRequest {
    InstallRequest {
        skill: SkillName(skill.to_string()),
        method,
        scope: RootScope::Global,
        harnesses: universal_and_claude(),
        files: Vec::new(),
        source: Some(skill.to_string()),
        trust_identity: None,
        // F5: a `Dotagents` install's trust identity always comes from
        // `source` itself, so this must set `trust_confirmed` for it to
        // pass the gate - `trust_identity` staying `None` no longer skips
        // the gate for `Dotagents` the way it still does for the other
        // methods.
        trust_confirmed: method == InstallMethod::Dotagents,
        save_as_preference: true,
        link_mode: InstallLinkMode::Link,
        destination: skill_studio_core::identity::SkillDestination::Universal,
    }
}

/// `install_records_the_journal_row_before_the_first_write_for_every_method`:
/// each method's `install` call leaves exactly one `install` event, `done`,
/// with the deployment on disk - proof the row landed as part of the same
/// call that wrote the bytes, for all three methods `ops::install` supports.
#[test]
fn install_records_the_journal_row_before_the_first_write_for_every_method() {
    for (label, method) in [
        ("copy", InstallMethod::Copy),
        ("dotagents", InstallMethod::Dotagents),
        ("skills_sh", InstallMethod::SkillsSh),
    ] {
        let home = unique_temp_dir(&format!("install_journal_{label}"));
        std::fs::create_dir_all(&home).unwrap();
        let rt = runtime_for(&home);
        let skill = format!("alpha-{label}");
        let req = match method {
            InstallMethod::Copy => copy_request(&skill),
            _ => cli_request(&skill, method),
        };

        let outcome = ops::install(&rt, &ctx(), &req).unwrap();
        let InstallOutcome::Installed {
            deployment_path, ..
        } = outcome
        else {
            panic!("expected Installed for {label}");
        };
        assert!(deployment_path.join("SKILL.md").exists());

        let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
        assert_eq!(events.len(), 1, "{label}: exactly one event recorded");
        assert_eq!(events[0].kind, "install");
        assert_eq!(events[0].status, "done");

        std::fs::remove_dir_all(&home).ok();
    }
}

/// `install_crash_after_each_step_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder`:
/// the red check for `Copy`'s stage/swap. A fresh install's destination
/// does not exist yet, so `swap` lands it with a rename, not an exchange
/// (`fsops.rs`'s `swap`: the exchange path only fires when something is
/// already at `final_name`) - failing that rename mid-`swap` must never
/// leave a half-swapped deployment: either the destination never existed
/// (the before state) or it holds a complete deployment, and any staged
/// temp folder left behind is exactly the one `journal::reconcile` (run by
/// the next `MutationSession::begin`) removes.
#[test]
fn install_crash_after_each_step_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder(
) {
    let home = unique_temp_dir("install_crash_window");
    std::fs::create_dir_all(&home).unwrap();
    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(
        &home,
        failing_fs.clone(),
        Some(Arc::new(FakeNpxSpawner::new(home.clone()))),
    );
    let req = copy_request("beta");

    // The journal's own manifest and plan writes (`begin`) and the `Stage`
    // and `Swap` steps' own `record_step` calls each go through
    // `fsops_rename` too, ahead of `swap`'s own landing rename - the 5th
    // `fsops_rename` call this install makes, counting from a clean plan:
    // 1 (manifest), 2 (plan), 3 (record_stage), 4 (record_swap), 5 (the
    // rename `swap` itself runs).
    failing_fs.fail_nth_fsops_rename(5);
    let err = ops::install(&rt, &ctx(), &req).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Io);

    let destination = home.join(UNIVERSAL_ROOT_RELATIVE).join("beta");
    // Crash invariant: the destination is not a half-written deployment -
    // it is either absent (the before state: `swap` never landed) or a
    // complete folder (the after state), never a folder missing files or a
    // dangling stage temp name in its place.
    if destination.exists() {
        assert!(
            destination.join("SKILL.md").exists(),
            "a landed deployment must be complete, not half-swapped"
        );
    } else {
        // The stage step did complete (recorded before the exchange that
        // was made to fail), so its temp folder is the stray this crash
        // window is allowed to leave - named under the journal's own
        // `.skill-studio-stage-*` convention, and swept by the next
        // `MutationSession::begin`'s reconciliation.
        let universal_root = home.join(UNIVERSAL_ROOT_RELATIVE);
        if universal_root.exists() {
            let stray: Vec<_> = std::fs::read_dir(&universal_root)
                .unwrap()
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(".skill-studio-stage-"))
                .collect();
            assert!(
                stray.len() <= 1,
                "at most one stray stage folder, named by the journal's own convention: {stray:?}"
            );
        }
    }

    // Recovery: the next mutation session reconciles the interrupted plan,
    // and a retry (with the filesystem working again) completes the
    // install a crash mid-swap could not.
    let session = MutationSession::begin(&rt, &ctx()).unwrap();

    // (R4) `begin` alone - before any retry - must already have swept the
    // stray stage folder: `MutationSession::begin` reconciles
    // `ops_install::journal_root` on every call, not just a later
    // `ops::install`. Pinning this here, separately from the retry below,
    // is the red check for accidentally dropping that
    // `crate::journal::reconcile(..)` call from `begin` - every other
    // install test in this file stays green even with it removed, since
    // they all go on to retry (which reconciles too, via its own `begin`).
    let universal_root = home.join(UNIVERSAL_ROOT_RELATIVE);
    if universal_root.exists() {
        let stray: Vec<_> = std::fs::read_dir(&universal_root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".skill-studio-stage-"))
            .collect();
        assert!(
            stray.is_empty(),
            "begin's own reconcile must sweep the stray stage folder, not just a later install's: {stray:?}"
        );
    }
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert_eq!(events.len(), 1, "the crashed install left exactly one row");
    assert_eq!(
        events[0].status, "failed",
        "begin's reconcile must resolve the interrupted plan and leave the row failed, not pending"
    );

    session.finish(&rt, &ctx());
    let retry = ops::install(&rt, &ctx(), &copy_request("beta")).unwrap();
    let InstallOutcome::Installed {
        deployment_path, ..
    } = retry
    else {
        panic!("expected Installed on retry");
    };
    assert!(deployment_path.join("SKILL.md").exists());

    std::fs::remove_dir_all(&home).ok();
}

/// `install_preferences_round_trips_a_saved_method_and_harnesses`: a call
/// with `save_as_preference: true` is exactly what the next
/// `install_preferences` call for the same scope returns - not the
/// environment default `install_preferences` falls back to when nothing has
/// been saved yet.
#[test]
fn install_preferences_round_trips_a_saved_method_and_harnesses() {
    let home = unique_temp_dir("install_preferences_roundtrip");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);

    let before = ops::install_preferences(&rt, &ctx(), &RootScope::Global).unwrap();
    assert!(
        !before.saved,
        "nothing saved yet: this must be the environment default"
    );

    let mut req = copy_request("gamma");
    req.save_as_preference = true;
    ops::install(&rt, &ctx(), &req).unwrap();

    let after = ops::install_preferences(&rt, &ctx(), &RootScope::Global).unwrap();
    assert!(after.saved);
    assert_eq!(after.method, InstallMethod::Copy);
    assert_eq!(after.harnesses, universal_and_claude());

    std::fs::remove_dir_all(&home).ok();
}

/// `direct_ops_call_leaves_the_disk_state_every_surface_shares`: parity
/// stand-in for the CLI trace test the unit brief asks for. `apps/cli`'s
/// `add` subcommand is a thin wrapper over `ops::install` (no logic of its
/// own, matching `run_park` for `ops::park`), so the one thing that could
/// differ between the CLI and a direct call is the disk state left behind;
/// this asserts that state directly against the layout
/// `docs/action-map/install.md` names for `Copy`.
///
/// Caveat, tracked as a follow-up: this is not yet a byte-for-byte
/// comparison against a captured CLI stdout/stderr trace fixture, since
/// `apps/cli`'s `add` subcommand did not exist yet when this test was
/// written (unit 3.5a shipped the core op first; 3.5b wires the CLI).
#[test]
fn direct_ops_call_leaves_the_disk_state_every_surface_shares() {
    let home = unique_temp_dir("install_parity");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let req = copy_request("delta");

    let outcome = ops::install(&rt, &ctx(), &req).unwrap();
    let InstallOutcome::Installed {
        deployment_path,
        linked_harnesses,
        ..
    } = outcome
    else {
        panic!("expected Installed");
    };

    assert_eq!(
        deployment_path,
        home.join(UNIVERSAL_ROOT_RELATIVE).join("delta")
    );
    assert!(deployment_path.join("SKILL.md").exists());
    assert_eq!(linked_harnesses, vec![AgentId::from(AgentId::CLAUDE_CODE)]);
    let link = home.join(".claude").join("skills").join("delta");
    assert_eq!(
        std::fs::canonicalize(&link).unwrap(),
        std::fs::canonicalize(&deployment_path).unwrap()
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `install_untrusted_dotagents_source_returns_needs_trust_or_names_the_bytes_it_wrote`
/// (F5): an unconfirmed `Dotagents` install of a source this scope has never
/// trusted returns `NeedsTrust` and writes nothing - not even the registry's
/// `trusted_dotagents_sources` list, since nothing was confirmed.
#[test]
fn install_untrusted_dotagents_source_returns_needs_trust_or_names_the_bytes_it_wrote() {
    let home = unique_temp_dir("install_untrusted_dotagents");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let mut req = cli_request("epsilon", InstallMethod::Dotagents);
    req.trust_confirmed = false;

    let outcome = ops::install(&rt, &ctx(), &req).unwrap();
    let InstallOutcome::NeedsTrust { identity } = outcome else {
        panic!("expected NeedsTrust for an unconfirmed dotagents source");
    };
    assert_eq!(identity, "epsilon");

    let deployment = home.join(UNIVERSAL_ROOT_RELATIVE).join("epsilon");
    assert!(
        !deployment.exists(),
        "NeedsTrust must not write the skill's bytes: {deployment:?}"
    );
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert!(
        events.is_empty(),
        "NeedsTrust must not record a journal row"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `install_confirmed_dotagents_source_records_trust_and_installs_or_names_the_missing_write`
/// (F5): a confirmed `Dotagents` install both records the source as trusted
/// and installs it; a second, unconfirmed install of the same source then
/// succeeds too, since the first call already recorded it as trusted.
#[test]
fn install_confirmed_dotagents_source_records_trust_and_installs_or_names_the_missing_write() {
    let home = unique_temp_dir("install_confirmed_dotagents");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let mut req = cli_request("zeta", InstallMethod::Dotagents);
    req.trust_confirmed = true;

    let outcome = ops::install(&rt, &ctx(), &req).unwrap();
    assert!(matches!(outcome, InstallOutcome::Installed { .. }));

    let mut second = cli_request("zeta-again", InstallMethod::Dotagents);
    second.source = Some("zeta".to_string());
    second.trust_confirmed = false;
    let second_outcome = ops::install(&rt, &ctx(), &second).unwrap();
    assert!(
        matches!(second_outcome, InstallOutcome::Installed { .. }),
        "a source already trusted must not need re-confirmation"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `install_crash_after_the_cli_wrote_the_folder_marks_the_row_failed_or_names_the_unowned_folder`
/// (F10): when the registry write after a `Dotagents`/`SkillsSh` CLI call
/// fails, the journal row is marked `Failed`, not left `Pending` - F9's
/// unified write-and-link step must cover the registry write too, not just
/// the skill's own bytes. The folder the CLI wrote is removed again, so no
/// unowned folder stays behind.
#[test]
fn install_crash_after_the_cli_wrote_the_folder_marks_the_row_failed_or_names_the_unowned_folder() {
    let home = unique_temp_dir("install_crash_after_cli_write");
    std::fs::create_dir_all(&home).unwrap();
    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(
        &home,
        failing_fs.clone(),
        Some(Arc::new(FakeNpxSpawner::new(home.clone()))),
    );
    let req = cli_request("eta", InstallMethod::SkillsSh);

    failing_fs.fail_next_write_atomic();
    let err = ops::install(&rt, &ctx(), &req).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Io);

    let deployment = home.join(UNIVERSAL_ROOT_RELATIVE).join("eta");
    assert!(
        std::fs::symlink_metadata(&deployment).is_err(),
        "the failed install must remove the folder the CLI wrote"
    );

    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].status, "failed",
        "the row must not be left pending when the registry write fails"
    );
    // R6: a failed row must still carry a `restore_backup`-shaped inverse
    // events.rs can parse, not the old `remove_install` shape nothing
    // recognized - `restore_capability` only returns `Yes` for a
    // `Failed`/`Interrupted` row when both `backup_dir` and a parseable
    // inverse are present.
    assert_eq!(
        events[0].restore,
        skill_studio_core::dto::RestoreCapability::Yes,
        "a failed install's row must be restorable via the shared restore_backup inverse shape"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `link_install_for_claude_code_and_pi_then_undo_removes_the_shared_folder_and_both_links_or_names_the_orphan`:
/// a global Link install for Claude Code and pi writes three paths; undoing
/// it with no force removes all three. Fails when undo reports drift on the
/// folder it wrote, or leaves a link or the shared folder behind.
#[test]
fn link_install_for_claude_code_and_pi_then_undo_removes_the_shared_folder_and_both_links_or_names_the_orphan(
) {
    let home = unique_temp_dir("install_link_then_undo");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let req = harness_set_request(
        "sigma",
        &[AgentId::CLAUDE_CODE, AgentId::PI],
        InstallLinkMode::Link,
    );
    let InstallOutcome::Installed { event_id, .. } = ops::install(&rt, &ctx(), &req).unwrap()
    else {
        panic!("expected Installed");
    };
    let written = [
        home.join(UNIVERSAL_ROOT_RELATIVE).join("sigma"),
        home.join(".claude/skills/sigma"),
        home.join(".pi/agent/skills/sigma"),
    ];
    for path in &written {
        assert!(
            std::fs::symlink_metadata(path).is_ok(),
            "installed: {path:?}"
        );
    }

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id,
            force: false,
        },
    )
    .unwrap();

    for path in &written {
        assert!(
            std::fs::symlink_metadata(path).is_err(),
            "undo must remove {path:?}"
        );
    }

    std::fs::remove_dir_all(&home).ok();
}

/// `install_that_fails_part_way_removes_the_shared_copy_and_the_link_it_wrote_or_names_the_orphan`:
/// with `~/.pi/agent` a regular file, a Link install for Claude Code and pi
/// writes the shared copy and Claude Code's link, then fails on pi's
/// folder. Both written paths must be gone and the row `failed`. Fails when
/// the shared copy or the Claude Code link stays behind.
#[test]
fn install_that_fails_part_way_removes_the_shared_copy_and_the_link_it_wrote_or_names_the_orphan() {
    let home = unique_temp_dir("install_part_way_cleanup");
    std::fs::create_dir_all(home.join(".pi")).unwrap();
    std::fs::write(home.join(".pi/agent"), b"not a folder").unwrap();
    let rt = runtime_for(&home);
    let req = harness_set_request(
        "tau",
        &[AgentId::CLAUDE_CODE, AgentId::PI],
        InstallLinkMode::Link,
    );

    ops::install(&rt, &ctx(), &req).unwrap_err();

    for path in [
        home.join(UNIVERSAL_ROOT_RELATIVE).join("tau"),
        home.join(".claude/skills/tau"),
    ] {
        assert!(
            std::fs::symlink_metadata(&path).is_err(),
            "a failed install must remove {path:?}"
        );
    }
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status, "failed");

    std::fs::remove_dir_all(&home).ok();
}

/// `install_for_a_harness_folder_linked_out_of_the_scope_is_refused_before_its_journal_row_or_names_the_write`:
/// with `~/.pi` a link to a folder outside the scope, a Link install for
/// Claude Code and pi is refused before it records a row or writes a byte.
/// Fails when the refusal comes only after the shared copy was written.
#[test]
fn install_for_a_harness_folder_linked_out_of_the_scope_is_refused_before_its_journal_row_or_names_the_write(
) {
    let home = unique_temp_dir("install_pi_linked_out");
    let outside = unique_temp_dir("install_pi_linked_out_target");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(outside.join("agent/skills")).unwrap();
    std::os::unix::fs::symlink(&outside, home.join(".pi")).unwrap();
    let rt = runtime_for(&home);
    let req = harness_set_request(
        "upsilon",
        &[AgentId::CLAUDE_CODE, AgentId::PI],
        InstallLinkMode::Link,
    );

    let err = ops::install(&rt, &ctx(), &req).unwrap_err();

    assert_eq!(err.code, skill_studio_core::ErrorCode::InvalidRequest);
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert!(
        events.is_empty(),
        "no journal row may be recorded: {events:?}"
    );
    assert!(
        !home.join(UNIVERSAL_ROOT_RELATIVE).join("upsilon").exists(),
        "no shared copy may be written"
    );

    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&outside).ok();
}

/// `copy_install_under_a_project_scope_is_classified_as_owned_or_names_the_deployment_left_manual`
/// (R1): `ops::classify_owner`'s `Copy` branch matches against
/// `ownership::read_home_registry`, which only ever reads the *home*
/// registry file - never a project's own `.agents/skill-studio.json`. A
/// `Copy` install under `RootScope::Project` must therefore write its
/// `copies` entry to the home registry too, or the deployment is left
/// `Manual` forever, even though `install` itself reports success.
#[test]
fn copy_install_under_a_project_scope_is_classified_as_owned_or_names_the_deployment_left_manual() {
    let home = unique_temp_dir("install_project_scope_ownership");
    std::fs::create_dir_all(&home).unwrap();
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let mut scope = RuntimeScope::fixture(&home);
    scope.projects = skill_studio_core::scope::ProjectSelection::Explicit {
        paths: vec![project.clone()],
    };
    let history_root = home.join(".history");
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(
            history_root.join("events.sqlite3"),
        )),
        sink: Arc::new(RecordingSink::default()),
        spawner: Some(Arc::new(FakeNpxSpawner::new(home.clone())) as Arc<dyn ProcessSpawner>),
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),

        telemetry: std::sync::Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    let rt = Runtime::new(&scope, ports).unwrap();

    let mut req = copy_request("iota");
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(project.clone()));
    // Nothing in this install ever touches the project's own registry
    // document (its `copies` entry lands in the *home* registry, per this
    // test's own doc, and `save_as_preference` is the only other write that
    // ever reaches it) - so the scope write must be skipped entirely rather
    // than creating `<project>/.agents/skill-studio.json` holding nothing
    // but a bumped `write_version`.
    req.save_as_preference = false;
    let outcome = ops::install(&rt, &ctx(), &req).unwrap();
    assert!(matches!(outcome, InstallOutcome::Installed { .. }));
    assert!(
        !project.join(".agents").join("skill-studio.json").exists(),
        "an unchanged project-scope registry document must not be written at all"
    );

    let inventory =
        ops::scan(&rt, &ctx(), &skill_studio_core::dto::ScanRequest::default()).unwrap();
    let skill = inventory
        .skills
        .iter()
        .find(|s| s.name.0 == "iota")
        .expect("the installed skill must appear in the scan");
    let deployment = skill
        .deployments
        .first()
        .expect("the project-scope install must leave exactly one deployment");
    assert_eq!(
        deployment.owner_kind,
        skill_studio_core::identity::LifecycleOwnerKind::Copy,
        "a project-scope Copy install must be classified as owned, not left Manual: {:?}",
        deployment.owner_kind
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `skills_sh_install_with_claude_code_keeps_the_cli_link_or_names_the_eexist_failure`
/// (R3): `cli_args_and_cwd` passes `--agent claude-code` for `SkillsSh`, so
/// the CLI itself creates `.claude/skills/<skill>` as part of its own run -
/// `link_claude_code` must treat an already-existing link at that path as
/// success, not fail with `EEXIST`.
#[test]
fn skills_sh_install_with_claude_code_keeps_the_cli_link_or_names_the_eexist_failure() {
    let home = unique_temp_dir("install_skills_sh_claude_code_link");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let req = cli_request("theta", InstallMethod::SkillsSh);

    let outcome = ops::install(&rt, &ctx(), &req).unwrap();
    let InstallOutcome::Installed {
        deployment_path,
        linked_harnesses,
        ..
    } = outcome
    else {
        panic!("expected Installed, not a failure over the CLI's own pre-existing link");
    };
    assert_eq!(linked_harnesses, vec![AgentId::from(AgentId::CLAUDE_CODE)]);

    let link = home.join(".claude").join("skills").join("theta");
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "the CLI-created link must still be there: {link:?}"
    );
    assert_eq!(
        std::fs::canonicalize(&link).unwrap(),
        std::fs::canonicalize(&deployment_path).unwrap()
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `skills_sh_project_install_runs_npx_in_the_project_dir_not_via_a_cwd_flag_or_names_the_stray_write`:
/// `skills@1.7.0` has no `--cwd` flag, so a project-scope skills.sh install
/// must carry the project path as the spawned process's own cwd, not as an
/// argv token - otherwise the CLI writes into whatever directory the host
/// process happened to start in instead of the project (the bug this test
/// pins fixed).
#[test]
fn skills_sh_project_install_runs_npx_in_the_project_dir_not_via_a_cwd_flag_or_names_the_stray_write(
) {
    let home = unique_temp_dir("install_skills_sh_project_cwd");
    std::fs::create_dir_all(&home).unwrap();
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let spawner = Arc::new(FakeNpxSpawner::new(home.clone()));
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(spawner.clone()));
    let mut req = cli_request("kappa", InstallMethod::SkillsSh);
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(project.clone()));

    let outcome = ops::install(&rt, &ctx(), &req).unwrap();
    assert!(
        matches!(outcome, InstallOutcome::Installed { .. }),
        "expected Installed, got {outcome:?}"
    );

    let recorded = spawner.recorded.lock().unwrap();
    assert_eq!(
        recorded.len(),
        1,
        "install_via_cli must call npx exactly once"
    );
    let (args, cwd) = &recorded[0];
    assert!(
        !args.contains(&"--cwd".to_string()),
        "skills@1.7.0 has no --cwd flag; the argv must not carry one: {args:?}"
    );
    assert_eq!(
        cwd.as_deref(),
        Some(project.as_path()),
        "the process cwd itself must be the project path"
    );

    let skill_dir = project.join(UNIVERSAL_ROOT_RELATIVE).join("kappa");
    assert!(
        skill_dir.join("SKILL.md").exists(),
        "the skill must land under the project, not under home: {skill_dir:?}"
    );
    assert!(
        !home.join(UNIVERSAL_ROOT_RELATIVE).join("kappa").exists(),
        "the skill must not also land in the process's original cwd"
    );

    // `cli_request` names `AgentId::CLAUDE_CODE`, so the CLI's own
    // `--agent claude-code` link must land under the project too, not home -
    // the same cwd the universal root above already proved.
    let claude_link = project.join(".claude").join("skills").join("kappa");
    assert!(
        std::fs::symlink_metadata(&claude_link).is_ok(),
        "the Claude Code link must be under the project: {claude_link:?}"
    );
    assert!(
        !home.join(".claude").join("skills").join("kappa").exists(),
        "the Claude Code link must not also land in the process's original cwd"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `skills_sh_project_install_leaves_a_skipped_pi_out_of_the_cli_agents_or_names_the_agent_it_passed`:
/// in a project with no `.pi` folder, the plan skips pi, so the `npx skills`
/// argv names Claude Code and not pi. Fails when `--agent pi` reaches the
/// CLI, which would then make the `.pi` folder the plan refused to make.
#[test]
fn skills_sh_project_install_leaves_a_skipped_pi_out_of_the_cli_agents_or_names_the_agent_it_passed(
) {
    let home = unique_temp_dir("install_skills_sh_skipped_pi");
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let spawner = Arc::new(FakeNpxSpawner::new(home.clone()));
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(spawner.clone()));
    let mut req = cli_request("pi-skip", InstallMethod::SkillsSh);
    req.harnesses = vec![
        AgentId::from(AgentId::CLAUDE_CODE),
        AgentId::from(AgentId::PI),
    ];
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(project.clone()));

    let (_, results) = harness_results(ops::install(&rt, &ctx(), &req).unwrap());

    let recorded = spawner.recorded.lock().unwrap();
    let agents: Vec<&str> = recorded[0]
        .0
        .windows(2)
        .filter(|w| w[0] == "--agent")
        .map(|w| w[1].as_str())
        .collect();
    assert_eq!(
        agents,
        vec!["claude-code"],
        "the CLI argv: {:?}",
        recorded[0].0
    );
    assert!(
        matches!(&results[1], InstallHarnessResult::Skipped { harness, .. } if harness.as_str() == AgentId::PI),
        "pi must be reported as skipped: {results:?}"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `skills_sh_project_install_with_one_served_harness_copies_into_its_folder_or_names_the_missing_destination`:
/// `add --harness claude-code --harness pi` in a project with no `.pi`. The
/// plan skips pi, so only the Claude Code folder is served. The skills CLI
/// sees one distinct folder and copies into `.claude/skills/<name>` on its
/// own, so core must expect a copy there. Fails when core still expects
/// a shared copy plus a link: the install errors with "the CLI did not create
/// the expected destination".
#[test]
fn skills_sh_project_install_with_one_served_harness_copies_into_its_folder_or_names_the_missing_destination(
) {
    let home = unique_temp_dir("install_skills_sh_one_served");
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let spawner = Arc::new(FakeNpxSpawner::new(home.clone()));
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(spawner));
    let mut req = cli_request("one-served", InstallMethod::SkillsSh);
    req.harnesses = vec![
        AgentId::from(AgentId::CLAUDE_CODE),
        AgentId::from(AgentId::PI),
    ];
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(project.clone()));

    let outcome = ops::install(&rt, &ctx(), &req).expect("the install must succeed");

    let InstallOutcome::Installed {
        deployment_path, ..
    } = outcome
    else {
        panic!("expected Installed");
    };
    assert_eq!(
        deployment_path,
        project.join(".claude/skills/one-served"),
        "the one served harness holds the real copy"
    );
    assert!(
        deployment_path.join("SKILL.md").exists(),
        "the copy must hold the skill"
    );
    std::fs::remove_dir_all(&home).ok();
}

/// `skills_sh_global_install_keeps_the_global_flag_and_no_process_cwd_or_names_the_over_eager_fix`:
/// a global-scope skills.sh install must still pass `--global` and run with
/// no process cwd override - guards against the project-scope `--cwd` fix
/// spilling into the global path, which never needed one.
#[test]
fn skills_sh_global_install_keeps_the_global_flag_and_no_process_cwd_or_names_the_over_eager_fix() {
    let home = unique_temp_dir("install_skills_sh_global_cwd");
    std::fs::create_dir_all(&home).unwrap();
    let spawner = Arc::new(FakeNpxSpawner::new(home.clone()));
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(spawner.clone()));
    let req = cli_request("lambda", InstallMethod::SkillsSh);

    let outcome = ops::install(&rt, &ctx(), &req).unwrap();
    assert!(matches!(outcome, InstallOutcome::Installed { .. }));

    let recorded = spawner.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    let (args, cwd) = &recorded[0];
    assert!(
        args.contains(&"--global".to_string()),
        "a global-scope install must still pass --global: {args:?}"
    );
    assert_eq!(
        cwd, &None,
        "a global-scope install must not set a process cwd"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `skills_sh_project_install_over_a_missing_project_path_names_it_before_spawning_npx_or_names_the_opaque_shell_error`:
/// a project path that doesn't exist must fail before `npx` ever runs, with
/// the path named in the error - otherwise the failure only ever surfaces
/// as the shell's own opaque "npx: no such file or directory".
#[test]
fn skills_sh_project_install_over_a_missing_project_path_names_it_before_spawning_npx_or_names_the_opaque_shell_error(
) {
    let home = unique_temp_dir("install_skills_sh_missing_project");
    std::fs::create_dir_all(&home).unwrap();
    let missing_project = home.join("does-not-exist");
    let spawner = Arc::new(FakeNpxSpawner::new(home.clone()));
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(spawner.clone()));
    let mut req = cli_request("nu", InstallMethod::SkillsSh);
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(
        missing_project.clone(),
    ));

    let err = ops::install(&rt, &ctx(), &req).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::InvalidRequest);
    assert!(
        err.message.contains(&missing_project.display().to_string()),
        "the error must name the missing project path: {}",
        err.message
    );
    assert_eq!(
        spawner.recorded.lock().unwrap().len(),
        0,
        "npx must never be spawned over a missing project path"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `skills_sh_project_install_that_lands_at_the_home_fallback_names_it_instead_of_a_bare_missing_destination_error`:
/// when the project-scope destination never appears but the CLI's
/// home-fallback location (`<home>/.agents/skills/<name>`) newly does, the
/// error must say the CLI installed there instead of just "did not create
/// the expected destination" - the same shape the `--cwd`-flag bug this PR
/// fixes would otherwise have produced silently.
#[test]
fn skills_sh_project_install_that_lands_at_the_home_fallback_names_it_instead_of_a_bare_missing_destination_error(
) {
    let home = unique_temp_dir("install_skills_sh_home_fallback");
    std::fs::create_dir_all(&home).unwrap();
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();

    /// A spawner that ignores the process cwd it's given and always writes
    /// under `home` - stands in for a spawner (or a future CLI) that drops
    /// `ProcessSpec::cwd` on the floor, the exact failure mode item 2(b)
    /// guards against.
    struct IgnoresCwdSpawner {
        home: PathBuf,
    }
    impl ProcessSpawner for IgnoresCwdSpawner {
        fn run(
            &self,
            spec: &ProcessSpec,
            _cancel: &dyn CancelToken,
        ) -> Result<ProcessOutput, skill_studio_core::CoreError> {
            let skill = spec
                .args
                .iter()
                .position(|a| a == "--skill")
                .and_then(|i| spec.args.get(i + 1))
                .unwrap()
                .clone();
            let dir = self.home.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), "installed at the wrong place\n").unwrap();
            Ok(ProcessOutput {
                status: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            })
        }
    }

    let spawner = Arc::new(IgnoresCwdSpawner { home: home.clone() });
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(spawner));
    let mut req = cli_request("xi", InstallMethod::SkillsSh);
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(project.clone()));

    let err = ops::install(&rt, &ctx(), &req).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Io);
    let fallback = home.join(UNIVERSAL_ROOT_RELATIVE).join("xi");
    assert!(
        err.message.contains(&fallback.display().to_string()),
        "the error must name the home fallback the CLI actually wrote to: {}",
        err.message
    );
    assert!(
        err.message.contains("instead of the project"),
        "the error must say this landed instead of the project: {}",
        err.message
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `project_skills_lock_json_classifies_a_skills_sh_install_as_owned_not_manual_or_names_the_lost_provenance`:
/// the CLI writes `<project>/skills-lock.json` (schema version 1) for a
/// project-scope skills.sh install, a different file from the shared
/// `.skill-lock.json` `ownership::read_scope_ledgers` already reads - a
/// tracked project's scan must still classify the skill as `SkillsSh`, not
/// fall back to `Manual` for want of a matching ledger entry.
#[test]
fn project_skills_lock_json_classifies_a_skills_sh_install_as_owned_not_manual_or_names_the_lost_provenance(
) {
    let home = unique_temp_dir("install_project_skills_lock_json");
    std::fs::create_dir_all(&home).unwrap();
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();

    let mut scope = RuntimeScope::fixture(&home);
    scope.projects = skill_studio_core::scope::ProjectSelection::Explicit {
        paths: vec![project.clone()],
    };
    let history_root = home.join(".history");
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(
            history_root.join("events.sqlite3"),
        )),
        sink: Arc::new(RecordingSink::default()),
        spawner: Some(Arc::new(FakeNpxSpawner::new(home.clone())) as Arc<dyn ProcessSpawner>),
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),

        telemetry: std::sync::Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    let rt = Runtime::new(&scope, ports).unwrap();

    let mut req = cli_request("mu", InstallMethod::SkillsSh);
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(project.clone()));
    req.save_as_preference = false;
    let outcome = ops::install(&rt, &ctx(), &req).unwrap();
    assert!(matches!(outcome, InstallOutcome::Installed { .. }));

    // The CLI's own project-scope lock file - `install_via_cli` never writes
    // this itself, so the test writes it the way `npx skills add --cwd
    // <project>` would, per PR #295's real trace.
    std::fs::write(
        project.join("skills-lock.json"),
        r#"{"version":1,"skills":{"mu":{"source":"owner/repo","sourceType":"github","computedHash":"abc123"}}}"#,
    )
    .unwrap();

    let inventory =
        ops::scan(&rt, &ctx(), &skill_studio_core::dto::ScanRequest::default()).unwrap();
    let skill = inventory
        .skills
        .iter()
        .find(|s| s.name.0 == "mu")
        .expect("the installed skill must appear in the scan");
    let deployment = skill
        .deployments
        .first()
        .expect("the project-scope install must leave exactly one deployment");
    assert_eq!(
        deployment.owner_kind,
        skill_studio_core::identity::LifecycleOwnerKind::SkillsSh,
        "skills-lock.json must classify the skill as skills-sh-owned, not left Manual: {:?}",
        deployment.owner_kind
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `install_over_a_non_object_registry_document_fails_before_any_write_or_names_the_wiped_registry`
/// (R7): `<home>/.agents/skill-studio.json` holding `[]` - valid JSON, but
/// not an object - must fail `read_registry_document` instead of silently
/// downgrading to an empty document, which would wipe `added_folders`,
/// `forks`, and the trust list on the write-back. The read happens before
/// `session.store.record`, so nothing this install would otherwise do -
/// the destination folder, the journal row - must exist afterward either.
#[test]
fn install_over_a_non_object_registry_document_fails_before_any_write_or_names_the_wiped_registry()
{
    let home = unique_temp_dir("install_non_object_registry");
    std::fs::create_dir_all(home.join(".agents")).unwrap();
    let registry_path = home.join(".agents").join("skill-studio.json");
    std::fs::write(&registry_path, b"[]").unwrap();
    let original_bytes = std::fs::read(&registry_path).unwrap();
    let rt = runtime_for(&home);
    let req = copy_request("kappa");

    let err = ops::install(&rt, &ctx(), &req).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Io);

    assert_eq!(
        std::fs::read(&registry_path).unwrap(),
        original_bytes,
        "a registry document that fails to read must not be rewritten"
    );
    assert!(
        !home.join(UNIVERSAL_ROOT_RELATIVE).join("kappa").exists(),
        "no destination folder may exist when the registry read fails before the first write"
    );
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert!(
        events.is_empty(),
        "the registry read happens before session.store.record, so no journal row exists either"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// One recorded (here, hand-built) `npx` call's shape, plus the files it
/// leaves on disk:
/// `crates/skill-studio-core/tests/fixtures/cli_traces/*.trace.json`.
#[derive(serde::Deserialize)]
struct CliTrace {
    program: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    files: Vec<CliTraceFile>,
}

/// One file the trace's CLI call wrote, relative to the skill's own folder
/// (`.agents/skills/<skill>/`, or `.claude/skills/<skill>/` for the file
/// `link_claude_code` reaches through its symlink).
#[derive(serde::Deserialize)]
struct CliTraceFile {
    relative_path: PathBuf,
    content: String,
}

/// Walks `root` (already known to exist), returning every regular file's
/// path relative to `root` and its bytes, sorted by path so two trees
/// compare deterministically regardless of read-dir order.
fn walk_files_relative(root: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            // `symlink_metadata` (not `metadata`) so the `.claude` link
            // itself is never mistaken for a directory to recurse into -
            // its target is a file already counted under `.agents/skills`.
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, out);
            } else {
                let contents = std::fs::read(&path).unwrap();
                out.push((path.strip_prefix(root).unwrap().to_path_buf(), contents));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// `cli_add_matches_the_npx_skills_add_trace_byte_for_byte_apart_from_timestamps_or_names_the_diverging_file`
/// (issue #163's parity checkbox): `docs/action-map/definition-of-done.md`
/// check 4's parity test (nine recorded CLI traces, diffed against our own
/// result tree) is unit 5.4's - recording a real `npx skills add` run needs
/// a real `npx`, which this worktree cannot do. This is its narrower stand-in
/// for `install_via_cli` alone: the fixture names every file a `skills.sh`
/// global-scope, Claude-Code-harness `add` call leaves under
/// `.agents/skills/<skill>` and `.claude/skills/<skill>`, `FakeNpxSpawner`
/// writes those exact bytes (not its own hardcoded `SKILL.md`), and this
/// test diffs the resulting tree against the fixture byte for byte - not
/// just the argv the table test in `ops_install_cli.rs` already covers.
/// Follow-up: swap the hand-built fixture for a recorded one once 5.4 exists.
#[test]
fn cli_add_matches_the_npx_skills_add_trace_byte_for_byte_apart_from_timestamps_or_names_the_diverging_file(
) {
    let fixture_bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cli_traces/skills_sh_add_global_claude_code.trace.json"
    ))
    .unwrap();
    let trace: CliTrace = serde_json::from_slice(&fixture_bytes).unwrap();
    assert_eq!(
        trace.program, "npx",
        "the fixture's own program must be npx"
    );

    let home = unique_temp_dir("install_cli_trace_parity");
    std::fs::create_dir_all(&home).unwrap();
    let trace_files: Vec<(PathBuf, String)> = trace
        .files
        .iter()
        .map(|f| (f.relative_path.clone(), f.content.clone()))
        .collect();
    let spawner = Arc::new(FakeNpxSpawner::with_trace_files(home.clone(), trace_files));
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(spawner.clone()));
    let mut req = cli_request("owner-repo-skill", InstallMethod::SkillsSh);
    req.source = Some("owner/repo".to_string());

    let outcome = ops::install(&rt, &ctx(), &req).unwrap();
    assert!(matches!(outcome, InstallOutcome::Installed { .. }));

    let recorded = spawner.recorded.lock().unwrap();
    assert_eq!(
        recorded.len(),
        1,
        "install_via_cli must call npx exactly once"
    );
    let (args, cwd) = &recorded[0];
    assert_eq!(
        args, &trace.args,
        "install_via_cli's argv drifted from the recorded skills.sh trace"
    );
    assert_eq!(
        cwd, &trace.cwd,
        "install_via_cli's cwd drifted from the recorded skills.sh trace"
    );

    // The universal root's tree must match the trace's files byte for byte.
    let universal_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join("owner-repo-skill");
    let actual = walk_files_relative(&universal_dir);
    let mut expected: Vec<(PathBuf, Vec<u8>)> = trace
        .files
        .iter()
        .map(|f| (f.relative_path.clone(), f.content.clone().into_bytes()))
        .collect();
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, expected_bytes) in &expected {
        let actual_bytes = actual.iter().find(|(p, _)| p == path).map(|(_, b)| b);
        assert_eq!(
            actual_bytes,
            Some(expected_bytes),
            "{} diverged from the recorded skills.sh trace",
            universal_dir.join(path).display()
        );
    }
    assert_eq!(
        actual.len(),
        expected.len(),
        "install_via_cli wrote a file the trace does not name: {actual:?} vs {expected:?}"
    );

    // `link_claude_code`'s symlink must reach the same bytes.
    let claude_dir = home.join(".claude").join("skills").join("owner-repo-skill");
    let claude_actual = walk_files_relative(&claude_dir);
    assert_eq!(
        claude_actual, actual,
        "the .claude link's tree diverged from the universal root's trace-matched tree"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `install_over_corrupt_json_registry_document_fails_before_any_write_or_names_the_wiped_registry`
/// (R7): the same guard as the non-object case, this time for bytes that
/// don't even parse as JSON (`{`, a truncated object) - `serde_json`'s own
/// parse error, not the "not an object" branch, must still fail the install
/// before any write.
#[test]
fn install_over_corrupt_json_registry_document_fails_before_any_write_or_names_the_wiped_registry()
{
    let home = unique_temp_dir("install_corrupt_registry");
    std::fs::create_dir_all(home.join(".agents")).unwrap();
    let registry_path = home.join(".agents").join("skill-studio.json");
    std::fs::write(&registry_path, b"{").unwrap();
    let original_bytes = std::fs::read(&registry_path).unwrap();
    let rt = runtime_for(&home);
    let req = copy_request("lambda");

    let err = ops::install(&rt, &ctx(), &req).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Io);

    assert_eq!(
        std::fs::read(&registry_path).unwrap(),
        original_bytes,
        "a registry document that fails to read must not be rewritten"
    );
    assert!(
        !home.join(UNIVERSAL_ROOT_RELATIVE).join("lambda").exists(),
        "no destination folder may exist when the registry read fails before the first write"
    );
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert!(
        events.is_empty(),
        "the registry read happens before session.store.record, so no journal row exists either"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `install_with_claude_code_under_a_whole_folder_link_to_another_folder_refuses_before_any_write_or_names_the_false_link`:
/// `~/.claude/skills` is a whole-folder link to a folder that is not
/// `~/.agents/skills`. Claude Code reads only that other folder, so an
/// install into the universal root cannot reach it. The install must refuse
/// with a message that names the link, and write nothing - not report
/// Claude Code as linked.
#[test]
fn install_with_claude_code_under_a_whole_folder_link_to_another_folder_refuses_before_any_write_or_names_the_false_link(
) {
    let home = unique_temp_dir("install_claude_whole_folder_link_elsewhere");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join("dotfiles").join("claude-skills")).unwrap();
    std::os::unix::fs::symlink(
        "../dotfiles/claude-skills",
        home.join(".claude").join("skills"),
    )
    .unwrap();
    let rt = runtime_for(&home);

    let result = ops::install(&rt, &ctx(), &copy_request("iota"));

    let err = match result {
        Err(err) => err,
        Ok(outcome) => panic!(
            "install reported {outcome:?} although Claude Code reads another folder through \
             ~/.claude/skills and cannot see ~/.agents/skills/iota"
        ),
    };
    assert_eq!(err.code, skill_studio_core::ErrorCode::InvalidRequest);
    assert!(
        err.message.contains("link to another folder"),
        "the refusal must say the whole-folder link points elsewhere, got: {}",
        err.message
    );
    assert!(
        !home.join(UNIVERSAL_ROOT_RELATIVE).join("iota").exists(),
        "a refused install must not leave the skill folder behind"
    );
    assert!(
        !home
            .join("dotfiles")
            .join("claude-skills")
            .join("iota")
            .exists(),
        "a refused install must not write into the folder the link points at"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `install_with_claude_code_under_a_whole_folder_link_to_the_universal_root_reports_linked_or_names_the_refusal`:
/// the common layout `~/.claude/skills -> ../.agents/skills`, both before
/// the universal root exists (a dangling link on a fresh home) and after.
/// Claude Code already sees every skill through the link, so the install
/// adds no per-skill link and reports Claude Code as linked.
#[test]
fn install_with_claude_code_under_a_whole_folder_link_to_the_universal_root_reports_linked_or_names_the_refusal(
) {
    for (label, universal_root_exists) in [("dangling", false), ("live", true)] {
        let home = unique_temp_dir(&format!("install_claude_whole_folder_link_{label}"));
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        if universal_root_exists {
            std::fs::create_dir_all(home.join(UNIVERSAL_ROOT_RELATIVE)).unwrap();
        }
        std::os::unix::fs::symlink("../.agents/skills", home.join(".claude").join("skills"))
            .unwrap();
        let rt = runtime_for(&home);

        let outcome = ops::install(&rt, &ctx(), &copy_request("kappa")).unwrap_or_else(|e| {
            panic!("{label} whole-folder link into the universal root was refused: {e}")
        });

        let InstallOutcome::Installed {
            linked_harnesses, ..
        } = outcome
        else {
            panic!("{label}: expected Installed, got {outcome:?}");
        };
        assert_eq!(
            linked_harnesses,
            vec![AgentId::from(AgentId::CLAUDE_CODE)],
            "{label}: Claude Code sees the skill through the whole-folder link"
        );
        assert!(
            std::fs::symlink_metadata(home.join(".claude").join("skills"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "{label}: the whole-folder link must stay a link"
        );
        assert!(
            home.join(".claude")
                .join("skills")
                .join("kappa")
                .join("SKILL.md")
                .exists(),
            "{label}: the skill must be readable through ~/.claude/skills"
        );

        std::fs::remove_dir_all(&home).ok();
    }
}

// ---------------------------------------------------------------------------
// Install for a harness set (`skills` CLI 1.7.0 rules).
// ---------------------------------------------------------------------------

fn harness_set_request(
    skill: &str,
    harnesses: &[&'static str],
    mode: InstallLinkMode,
) -> InstallRequest {
    let mut req = copy_request(skill);
    req.harnesses = harnesses.iter().map(|h| AgentId::from(*h)).collect();
    req.link_mode = mode;
    req.save_as_preference = false;
    req
}

fn harness_results(outcome: InstallOutcome) -> (PathBuf, Vec<InstallHarnessResult>) {
    match outcome {
        InstallOutcome::Installed {
            deployment_path,
            harness_results,
            ..
        } => (deployment_path, harness_results),
        other @ InstallOutcome::NeedsTrust { .. } => panic!("expected Installed, got {other:?}"),
    }
}

fn is_real_folder(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}

fn home_copies(home: &std::path::Path) -> serde_json::Map<String, serde_json::Value> {
    let raw = std::fs::read_to_string(home.join(".agents").join("skill-studio.json")).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    doc["copies"].as_object().cloned().unwrap_or_default()
}

/// `install_for_claude_code_and_pi_links_both_to_one_shared_copy_with_relative_targets_or_names_the_bad_link`:
/// a global Link install for Claude Code and pi writes one real folder in
/// `~/.agents/skills` and a relative link in `~/.claude/skills` and in
/// `~/.pi/agent/skills`, the same `../` spelling the CLI writes. Fails when
/// a link is absolute, missing, or pi's link lands in `~/.pi/skills`.
#[test]
fn install_for_claude_code_and_pi_links_both_to_one_shared_copy_with_relative_targets_or_names_the_bad_link(
) {
    let home = unique_temp_dir("install_claude_and_pi_link");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let req = harness_set_request(
        "lambda",
        &[AgentId::CLAUDE_CODE, AgentId::PI],
        InstallLinkMode::Link,
    );

    let (deployment_path, results) = harness_results(ops::install(&rt, &ctx(), &req).unwrap());

    let shared = home.join(UNIVERSAL_ROOT_RELATIVE).join("lambda");
    assert_eq!(deployment_path, shared, "the shared copy is the deployment");
    assert!(
        is_real_folder(&shared),
        "the shared copy must be a real folder"
    );
    let claude_link = home.join(".claude/skills/lambda");
    let pi_link = home.join(".pi/agent/skills/lambda");
    assert_eq!(
        std::fs::read_link(&claude_link).unwrap(),
        PathBuf::from("../../.agents/skills/lambda"),
        "Claude Code's link must be relative"
    );
    assert_eq!(
        std::fs::read_link(&pi_link).unwrap(),
        PathBuf::from("../../../.agents/skills/lambda"),
        "pi's global link lives in ~/.pi/agent/skills and must be relative"
    );
    assert!(
        pi_link.join("SKILL.md").exists(),
        "pi must read the skill through its link"
    );
    assert_eq!(
        results,
        vec![
            InstallHarnessResult::Linked {
                harness: AgentId::from(AgentId::CLAUDE_CODE),
                path: claude_link,
            },
            InstallHarnessResult::Linked {
                harness: AgentId::from(AgentId::PI),
                path: pi_link,
            },
        ]
    );
    assert_eq!(
        home_copies(&home).len(),
        1,
        "only the shared copy is a real folder"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `install_for_pi_at_project_scope_without_a_pi_folder_skips_pi_and_says_why_or_names_the_folder_it_made`:
/// in a project with no `.pi` folder, a Link install for Claude Code and pi
/// links Claude Code (always made) and skips pi with a plain reason. Fails
/// when `.pi` is created or the skip is not reported.
#[test]
fn install_for_pi_at_project_scope_without_a_pi_folder_skips_pi_and_says_why_or_names_the_folder_it_made(
) {
    let home = unique_temp_dir("install_pi_project_skip");
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let mut scope = RuntimeScope::fixture(&home);
    scope.projects = skill_studio_core::scope::ProjectSelection::Explicit {
        paths: vec![project.clone()],
    };
    let rt = runtime_in(&scope, &home, Arc::new(RealFs::new()), None);
    let mut req = harness_set_request(
        "mu",
        &[AgentId::CLAUDE_CODE, AgentId::PI],
        InstallLinkMode::Link,
    );
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(project.clone()));

    let (_, results) = harness_results(ops::install(&rt, &ctx(), &req).unwrap());

    assert!(
        !project.join(".pi").exists(),
        "no .pi folder may be created"
    );
    assert_eq!(
        results,
        vec![
            InstallHarnessResult::Linked {
                harness: AgentId::from(AgentId::CLAUDE_CODE),
                path: project.join(".claude/skills/mu"),
            },
            InstallHarnessResult::Skipped {
                harness: AgentId::from(AgentId::PI),
                reason: "pi has no .pi folder in this project".to_string(),
            },
        ]
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `copy_mode_install_for_pi_at_project_scope_without_a_pi_folder_skips_pi_or_names_the_folder_it_made`:
/// the missing-folder skip does not depend on the mode. A Copy install for
/// Claude Code and pi in a project with no `.pi` folder copies for Claude
/// Code and skips pi. Fails when Copy mode creates `.pi/skills`.
#[test]
fn copy_mode_install_for_pi_at_project_scope_without_a_pi_folder_skips_pi_or_names_the_folder_it_made(
) {
    let home = unique_temp_dir("install_pi_project_copy_skip");
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let mut scope = RuntimeScope::fixture(&home);
    scope.projects = skill_studio_core::scope::ProjectSelection::Explicit {
        paths: vec![project.clone()],
    };
    let rt = runtime_in(&scope, &home, Arc::new(RealFs::new()), None);
    let mut req = harness_set_request(
        "mu",
        &[AgentId::CLAUDE_CODE, AgentId::PI],
        InstallLinkMode::Copy,
    );
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(project.clone()));

    let (_, results) = harness_results(ops::install(&rt, &ctx(), &req).unwrap());

    assert!(
        !project.join(".pi").exists(),
        "Copy mode may not create a .pi folder either"
    );
    assert_eq!(
        results,
        vec![
            InstallHarnessResult::Copied {
                harness: AgentId::from(AgentId::CLAUDE_CODE),
                path: project.join(".claude/skills/mu"),
                link_failed: false,
            },
            InstallHarnessResult::Skipped {
                harness: AgentId::from(AgentId::PI),
                reason: "pi has no .pi folder in this project".to_string(),
            },
        ]
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `install_under_a_whole_folder_claude_link_to_the_shared_folder_makes_no_self_link_or_names_the_link_it_wrote`:
/// with `~/.claude/skills -> ../.agents/skills`, a Link install for Claude
/// Code and pi reports Claude Code as reading the shared folder, leaves the
/// shared copy a real folder (never a link to itself), and still links pi.
#[test]
fn install_under_a_whole_folder_claude_link_to_the_shared_folder_makes_no_self_link_or_names_the_link_it_wrote(
) {
    let home = unique_temp_dir("install_whole_folder_no_self_link");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join(UNIVERSAL_ROOT_RELATIVE)).unwrap();
    std::os::unix::fs::symlink("../.agents/skills", home.join(".claude/skills")).unwrap();
    let rt = runtime_for(&home);
    let req = harness_set_request(
        "nu",
        &[AgentId::CLAUDE_CODE, AgentId::PI],
        InstallLinkMode::Link,
    );

    let (_, results) = harness_results(ops::install(&rt, &ctx(), &req).unwrap());

    let shared = home.join(UNIVERSAL_ROOT_RELATIVE).join("nu");
    assert!(
        is_real_folder(&shared),
        "the shared copy must stay a real folder"
    );
    assert_eq!(
        results[0],
        InstallHarnessResult::ReadsShared {
            harness: AgentId::from(AgentId::CLAUDE_CODE),
            path: shared,
        },
        "Claude Code reads the shared folder through the whole-folder link"
    );
    assert!(
        matches!(&results[1], InstallHarnessResult::Linked { harness, .. } if harness.as_str() == AgentId::PI),
        "pi still gets its own link: {:?}",
        results[1]
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `install_for_claude_code_alone_forces_a_copy_with_no_shared_folder_or_names_the_link_it_made`:
/// one chosen folder forces Copy even when Link is asked (the CLI's
/// `uniqueDirs.size <= 1` rule), so Claude Code alone gets a real folder in
/// `~/.claude/skills` and nothing is written to `~/.agents/skills`.
#[test]
fn install_for_claude_code_alone_forces_a_copy_with_no_shared_folder_or_names_the_link_it_made() {
    let home = unique_temp_dir("install_claude_alone_copy");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let req = harness_set_request("xi", &[AgentId::CLAUDE_CODE], InstallLinkMode::Link);

    let (deployment_path, results) = harness_results(ops::install(&rt, &ctx(), &req).unwrap());

    let claude_copy = home.join(".claude/skills/xi");
    assert_eq!(deployment_path, claude_copy);
    assert!(
        is_real_folder(&claude_copy),
        "Claude Code must get a real folder"
    );
    assert!(claude_copy.join("SKILL.md").exists());
    assert!(
        !home.join(UNIVERSAL_ROOT_RELATIVE).join("xi").exists(),
        "a forced copy writes no shared copy"
    );
    assert_eq!(
        results,
        vec![InstallHarnessResult::Copied {
            harness: AgentId::from(AgentId::CLAUDE_CODE),
            path: claude_copy,
            link_failed: false,
        }]
    );
    let copies = home_copies(&home);
    assert_eq!(copies.len(), 1);
    let entry = copies.values().next().unwrap();
    assert_eq!(entry["destination"], "per_harness");
    assert_eq!(entry["slot"], "claude-code");

    std::fs::remove_dir_all(&home).ok();
}

/// `install_in_copy_mode_for_claude_code_and_codex_writes_two_real_folders_or_names_the_link`:
/// Copy mode for Claude Code and Codex writes the shared copy (Codex reads
/// it) and a separate real folder in `~/.claude/skills`, and records both.
#[test]
fn install_in_copy_mode_for_claude_code_and_codex_writes_two_real_folders_or_names_the_link() {
    let home = unique_temp_dir("install_copy_mode_two_folders");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let req = harness_set_request(
        "omicron",
        &[AgentId::CLAUDE_CODE, AgentId::CODEX],
        InstallLinkMode::Copy,
    );

    let (_, results) = harness_results(ops::install(&rt, &ctx(), &req).unwrap());

    let shared = home.join(UNIVERSAL_ROOT_RELATIVE).join("omicron");
    let claude_copy = home.join(".claude/skills/omicron");
    assert!(
        is_real_folder(&shared),
        "Codex's shared copy must be a real folder"
    );
    assert!(
        is_real_folder(&claude_copy),
        "Claude Code's copy must be a real folder, not a link"
    );
    assert_eq!(
        results,
        vec![
            InstallHarnessResult::Copied {
                harness: AgentId::from(AgentId::CLAUDE_CODE),
                path: claude_copy,
                link_failed: false,
            },
            InstallHarnessResult::ReadsShared {
                harness: AgentId::from(AgentId::CODEX),
                path: shared,
            },
        ]
    );
    assert_eq!(
        home_copies(&home).len(),
        2,
        "both real folders are recorded"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `install_falls_back_to_a_copy_when_the_link_fails_and_reports_it_or_names_the_missing_folder`:
/// when the symlink call fails, the native method copies the folder into
/// the harness folder instead, the same fallback as the CLI, and reports
/// `link_failed`.
#[test]
fn install_falls_back_to_a_copy_when_the_link_fails_and_reports_it_or_names_the_missing_folder() {
    let home = unique_temp_dir("install_link_fails_copy");
    std::fs::create_dir_all(&home).unwrap();
    let failing = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    failing.fail_next_symlink();
    let rt = runtime_with(&home, failing, None);
    let req = harness_set_request(
        "rho",
        &[AgentId::CLAUDE_CODE, AgentId::CODEX],
        InstallLinkMode::Link,
    );

    let (_, results) = harness_results(ops::install(&rt, &ctx(), &req).unwrap());

    let claude_copy = home.join(".claude/skills/rho");
    assert!(
        is_real_folder(&claude_copy),
        "a failed link must leave a real folder"
    );
    assert_eq!(
        results[0],
        InstallHarnessResult::Copied {
            harness: AgentId::from(AgentId::CLAUDE_CODE),
            path: claude_copy,
            link_failed: true,
        }
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `copy_install_keeps_each_source_files_mode_or_names_the_file_that_lost_its_bits`:
/// Flow: a global Copy install carries `scripts/run.sh` (0o755) and
/// `SKILL.md` (0o640). Expectation: the deployed script is still 0o755 and
/// `SKILL.md` keeps 0o640. A failure here means the copy wrote bytes with the
/// process default mode, so an installed script is no longer executable.
#[cfg(unix)]
#[test]
fn copy_install_keeps_each_source_files_mode_or_names_the_file_that_lost_its_bits() {
    use std::os::unix::fs::PermissionsExt;

    let home = unique_temp_dir("install_copy_keeps_modes");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let mut req = copy_request("modes");
    req.files[0].mode = Some(0o640);
    req.files.push(InstallFile {
        relative_path: PathBuf::from("scripts/run.sh"),
        contents: b"#!/bin/sh\necho hi\n".to_vec(),
        mode: Some(0o755),
    });

    ops::install(&rt, &ctx(), &req).unwrap();

    let deployed = home.join(UNIVERSAL_ROOT_RELATIVE).join("modes");
    let mode_of = |relative: &str| {
        std::fs::metadata(deployed.join(relative))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode_of("scripts/run.sh"), 0o755, "the script lost its bits");
    assert_eq!(mode_of("SKILL.md"), 0o640, "a plain file lost its own mode");

    std::fs::remove_dir_all(&home).ok();
}

/// `undo_of_a_copy_install_restores_the_registry_and_preferences_or_names_the_key_left_behind`:
/// Flow: `skill-studio.json` already holds an unrelated `copies` entry and a
/// saved `preferred_method`/`preferred_harnesses`; a global Copy install with
/// `save_as_preference` changes both and adds its own `copies` entries; the
/// install is then undone. Expectation: the registry equals its state before
/// the install (the other copy and the earlier preferences come back
/// unchanged, none of the install's copies stay) and a scan finds no
/// deployment of the skill. A failure here means undo removed the folders
/// but left the install's registry writes behind.
#[test]
fn undo_of_a_copy_install_restores_the_registry_and_preferences_or_names_the_key_left_behind() {
    let home = unique_temp_dir("install_undo_registry");
    std::fs::create_dir_all(home.join(".agents")).unwrap();
    let registry_path = home.join(".agents").join("skill-studio.json");
    let before = serde_json::json!({
        "preferred_method": "skills-sh",
        "preferred_harnesses": ["codex"],
        "copies": { "other-id": { "deployment_id": "other-id", "name": "other" } },
        "added_folders": ["/somewhere"],
    });
    std::fs::write(&registry_path, serde_json::to_vec(&before).unwrap()).unwrap();
    let rt = runtime_for(&home);

    let req = copy_request("undone");
    let InstallOutcome::Installed { event_id, .. } = ops::install(&rt, &ctx(), &req).unwrap()
    else {
        panic!("expected Installed");
    };
    assert_ne!(
        read_registry(&registry_path),
        before,
        "the install must write"
    );

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id,
            force: false,
        },
    )
    .unwrap();

    assert_eq!(
        read_registry(&registry_path),
        before,
        "undo must put copies and preferences back"
    );
    let inventory =
        ops::scan(&rt, &ctx(), &skill_studio_core::dto::ScanRequest::default()).unwrap();
    assert!(
        !inventory.skills.iter().any(|s| s.name.0 == "undone"),
        "a scan must show no deployment after undo"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// The registry file without its `write_version` counter, which every write
/// bumps and undo cannot rewind.
fn read_registry(path: &std::path::Path) -> serde_json::Value {
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    doc.as_object_mut().unwrap().remove("write_version");
    doc
}

// ---------------------------------------------------------------------------
// `destination: PerHarness`: a real folder in each chosen harness's own
// skills folder, and nothing in the shared folder.
// ---------------------------------------------------------------------------

const PER_HARNESS_ALL: [&str; 6] = [
    AgentId::CLAUDE_CODE,
    AgentId::CODEX,
    AgentId::OPEN_CODE,
    AgentId::PI,
    AgentId::CURSOR,
    AgentId::GROK_BUILD,
];

fn per_harness_request(skill: &str, harnesses: &[&'static str]) -> InstallRequest {
    let mut req = harness_set_request(skill, harnesses, InstallLinkMode::Copy);
    req.destination = skill_studio_core::identity::SkillDestination::PerHarness;
    req
}

/// `per_harness_install_at_global_scope_copies_into_each_own_folder_and_not_the_shared_folder_or_names_the_missing_one`:
/// a global per-harness install for all six harnesses writes a real folder in
/// each harness's own skills folder, the same six the Destination rows show,
/// and none in `~/.agents/skills`. Fails when a harness's folder is missing
/// (the row promises it) or the shared folder gets a copy (Universal is off).
#[test]
fn per_harness_install_at_global_scope_copies_into_each_own_folder_and_not_the_shared_folder_or_names_the_missing_one(
) {
    let home = unique_temp_dir("install_per_harness_global");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let req = per_harness_request("tau", &PER_HARNESS_ALL);

    let (_, results) = harness_results(ops::install(&rt, &ctx(), &req).unwrap());

    let folders = [
        ".claude/skills",
        ".codex/skills",
        ".config/opencode/skills",
        ".pi/agent/skills",
        ".cursor/skills",
        ".grok/skills",
    ];
    for folder in folders {
        assert!(
            is_real_folder(&home.join(folder).join("tau")),
            "a real copy in {folder}"
        );
    }
    assert!(
        std::fs::symlink_metadata(home.join(UNIVERSAL_ROOT_RELATIVE).join("tau")).is_err(),
        "the shared folder gets no copy"
    );
    assert!(results
        .iter()
        .all(|r| matches!(r, InstallHarnessResult::Copied { .. })));
    assert_eq!(results.len(), PER_HARNESS_ALL.len());

    std::fs::remove_dir_all(&home).ok();
}

/// A runtime whose Codex home and `OpenCode` config root are set to the given
/// paths in place of `~/.codex` and `~/.config/opencode`.
fn runtime_with_roots(
    home: &std::path::Path,
    codex_home: Option<PathBuf>,
    opencode_root: Option<PathBuf>,
) -> Runtime {
    let mut scope = RuntimeScope::fixture(home);
    scope.codex_home = codex_home;
    scope.opencode_config_root = opencode_root;
    runtime_in(
        &scope,
        home,
        Arc::new(RealFs::new()),
        Some(Arc::new(FakeNpxSpawner::new(home.to_path_buf()))),
    )
}

fn scanned_paths(rt: &Runtime, skill: &str) -> Vec<PathBuf> {
    let inventory = ops::scan(rt, &ctx(), &skill_studio_core::dto::ScanRequest::default()).unwrap();
    inventory
        .skills
        .iter()
        .filter(|s| s.name.0 == skill)
        .flat_map(|s| s.deployments.iter().map(|d| d.path.clone()))
        .collect()
}

/// `per_harness_install_with_a_configured_codex_home_copies_under_it_and_scan_lists_it_or_names_the_default_path_used`:
/// with `codex_home` set to `home/custom-codex`, a per-harness Codex install
/// writes a real folder at `custom-codex/skills/tau`, none at
/// `~/.codex/skills/tau`, and a scan lists the copy. Fails when install
/// writes the default path, which Codex never reads.
#[test]
fn per_harness_install_with_a_configured_codex_home_copies_under_it_and_scan_lists_it_or_names_the_default_path_used(
) {
    let home = unique_temp_dir("install_per_harness_codex_home");
    std::fs::create_dir_all(&home).unwrap();
    let custom = home.join("custom-codex");
    let rt = runtime_with_roots(&home, Some(custom.clone()), None);

    ops::install(&rt, &ctx(), &per_harness_request("tau", &["codex"])).unwrap();

    let copy = custom.join("skills").join("tau");
    assert!(is_real_folder(&copy), "a real copy under the Codex home");
    assert!(
        std::fs::symlink_metadata(home.join(".codex/skills/tau")).is_err(),
        "no copy under the default ~/.codex"
    );
    assert!(
        scanned_paths(&rt, "tau").contains(&copy),
        "the scan must list the copy under the Codex home"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `per_harness_install_with_a_configured_opencode_root_inside_or_outside_home_copies_under_it_and_scan_lists_it_or_names_the_failure`:
/// with `opencode_config_root` set inside the home and again outside it, a
/// per-harness `OpenCode` install writes `<root>/skills/tau`, none under
/// `~/.config/opencode`, and a scan lists it. Fails when install writes the
/// default path or refuses a root outside the home.
#[test]
fn per_harness_install_with_a_configured_opencode_root_inside_or_outside_home_copies_under_it_and_scan_lists_it_or_names_the_failure(
) {
    // `contains` checks the OpenCode root lexically, so the outside root must
    // not sit behind a symlink such as macOS's /var -> /private/var.
    let outside = unique_temp_dir("install_per_harness_opencode_outside");
    std::fs::create_dir_all(&outside).unwrap();
    let outside = outside.canonicalize().unwrap();
    // Install makes `skills` under the root, not the root: a config root is
    // the user's own directory and exists before any install.
    std::fs::create_dir_all(outside.join("opencode")).unwrap();
    for (label, outside_home) in [("inside", false), ("outside", true)] {
        let home = unique_temp_dir(&format!("install_per_harness_opencode_{label}"));
        std::fs::create_dir_all(&home).unwrap();
        let custom = if outside_home {
            outside.join("opencode")
        } else {
            home.join("custom-opencode")
        };
        let rt = runtime_with_roots(&home, None, Some(custom.clone()));

        ops::install(&rt, &ctx(), &per_harness_request("tau", &["open-code"])).unwrap();

        let copy = custom.join("skills").join("tau");
        assert!(is_real_folder(&copy), "a real copy in the {label} root");
        assert!(
            std::fs::symlink_metadata(home.join(".config/opencode/skills/tau")).is_err(),
            "no copy under the default ~/.config/opencode"
        );
        assert!(
            scanned_paths(&rt, "tau").contains(&copy),
            "the scan must list the copy in the {label} root"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    std::fs::remove_dir_all(&outside).ok();
}

/// `per_harness_install_with_both_roots_configured_writes_where_split_writes_for_every_harness_or_names_the_harness_that_differs`:
/// with `codex_home` and `opencode_config_root` both set, each harness's
/// per-harness copy lands in the folder `split_target_root` names. Fails
/// when install and split resolve a harness to different folders, which
/// leaves a split copy and an install copy in two places.
#[test]
fn per_harness_install_with_both_roots_configured_writes_where_split_writes_for_every_harness_or_names_the_harness_that_differs(
) {
    let home = unique_temp_dir("install_per_harness_both_roots");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_with_roots(
        &home,
        Some(home.join("custom-codex")),
        Some(home.join("custom-opencode")),
    );

    let (_, results) = harness_results(
        ops::install(&rt, &ctx(), &per_harness_request("tau", &PER_HARNESS_ALL)).unwrap(),
    );

    assert_eq!(results.len(), PER_HARNESS_ALL.len());
    for result in results {
        let InstallHarnessResult::Copied { harness, path, .. } = result else {
            panic!("expected Copied, got {result:?}");
        };
        let expected =
            skill_studio_core::ops_split::split_target_root(&rt, &RootScope::Global, &harness)
                .unwrap()
                .join("tau");
        assert_eq!(path, expected, "install path for {harness}");
    }

    std::fs::remove_dir_all(&home).ok();
}

/// `undo_of_a_per_harness_install_under_configured_roots_removes_the_copies_or_names_the_one_left_behind`:
/// undoing a per-harness Codex and `OpenCode` install made under custom roots
/// removes both copies. Fails when undo looks in the default folders and
/// leaves a copy under a custom root.
#[test]
fn undo_of_a_per_harness_install_under_configured_roots_removes_the_copies_or_names_the_one_left_behind(
) {
    let home = unique_temp_dir("install_per_harness_undo_roots");
    std::fs::create_dir_all(&home).unwrap();
    let codex = home.join("custom-codex");
    let opencode = home.join("custom-opencode");
    let rt = runtime_with_roots(&home, Some(codex.clone()), Some(opencode.clone()));
    let InstallOutcome::Installed { event_id, .. } = ops::install(
        &rt,
        &ctx(),
        &per_harness_request("tau", &["codex", "open-code"]),
    )
    .unwrap() else {
        panic!("expected Installed");
    };
    assert!(is_real_folder(&codex.join("skills/tau")));
    assert!(is_real_folder(&opencode.join("skills/tau")));

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id,
            force: false,
        },
    )
    .unwrap();

    for copy in [codex.join("skills/tau"), opencode.join("skills/tau")] {
        assert!(
            std::fs::symlink_metadata(&copy).is_err(),
            "undo must remove {}",
            copy.display()
        );
    }

    std::fs::remove_dir_all(&home).ok();
}

/// `per_harness_install_at_project_scope_creates_pi_and_grok_folders_and_uses_each_project_folder_or_names_the_skip`:
/// in a project with no `.pi` or `.grok` folder, a per-harness install still
/// copies for pi and Grok Build (the Universal-mode install skips them), and
/// Codex, `OpenCode`, and Cursor get `.codex/skills`, `.opencode/skills`, and
/// `.cursor/skills`. Fails when a harness is skipped, so the row promised a
/// folder the backend did not write.
#[test]
fn per_harness_install_at_project_scope_creates_pi_and_grok_folders_and_uses_each_project_folder_or_names_the_skip(
) {
    let home = unique_temp_dir("install_per_harness_project");
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let mut scope = RuntimeScope::fixture(&home);
    scope.projects = skill_studio_core::scope::ProjectSelection::Explicit {
        paths: vec![project.clone()],
    };
    let rt = runtime_in(&scope, &home, Arc::new(RealFs::new()), None);
    let mut req = per_harness_request("tau", &PER_HARNESS_ALL);
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(project.clone()));

    let (_, results) = harness_results(ops::install(&rt, &ctx(), &req).unwrap());

    for folder in [
        ".claude/skills",
        ".codex/skills",
        ".opencode/skills",
        ".pi/skills",
        ".cursor/skills",
        ".grok/skills",
    ] {
        assert!(
            is_real_folder(&project.join(folder).join("tau")),
            "a real copy in {folder}"
        );
    }
    assert!(
        std::fs::symlink_metadata(project.join(UNIVERSAL_ROOT_RELATIVE).join("tau")).is_err(),
        "the shared folder gets no copy"
    );
    assert!(results
        .iter()
        .all(|r| matches!(r, InstallHarnessResult::Copied { .. })));

    std::fs::remove_dir_all(&home).ok();
}

/// `per_harness_install_with_a_cli_method_is_refused_or_writes_the_shared_folder_anyway`:
/// skills.sh and dotagents always write `.agents/skills`, so a per-harness
/// request for either is an invalid request and writes nothing. Fails when it
/// installs, since the user asked for no shared copy.
#[test]
fn per_harness_install_with_a_cli_method_is_refused_or_writes_the_shared_folder_anyway() {
    let home = unique_temp_dir("install_per_harness_cli_method");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let mut req = per_harness_request("tau", &[AgentId::CODEX]);
    req.method = InstallMethod::SkillsSh;
    req.source = Some("owner/repo".to_string());

    let err = ops::install(&rt, &ctx(), &req).unwrap_err();

    assert_eq!(
        err.code,
        skill_studio_core::error::ErrorCode::InvalidRequest
    );
    assert!(std::fs::read_dir(home.join(UNIVERSAL_ROOT_RELATIVE)).is_err());

    std::fs::remove_dir_all(&home).ok();
}

/// `per_harness_install_undo_removes_each_copy_or_names_the_orphan`: undoing a
/// per-harness install removes the Codex and Cursor copies. Fails when undo
/// leaves one behind.
#[test]
fn per_harness_install_undo_removes_each_copy_or_names_the_orphan() {
    let home = unique_temp_dir("install_per_harness_undo");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let req = per_harness_request("tau", &[AgentId::CODEX, AgentId::CURSOR]);
    let InstallOutcome::Installed { event_id, .. } = ops::install(&rt, &ctx(), &req).unwrap()
    else {
        panic!("expected Installed");
    };

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id,
            force: false,
        },
    )
    .unwrap();

    for folder in [".codex/skills", ".cursor/skills"] {
        assert!(
            std::fs::symlink_metadata(home.join(folder).join("tau")).is_err(),
            "undo must remove the copy in {folder}"
        );
    }

    std::fs::remove_dir_all(&home).ok();
}
