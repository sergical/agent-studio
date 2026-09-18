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
    InstallFile, InstallMethod, InstallOutcome, InstallRequest, ListEventsRequest,
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
/// falls back to `home` for the process cwd, since `install_via_cli` only
/// ever sets a cwd for a `Dotagents` project-scope install - skills.sh's own
/// builder never sets the process cwd at all (`--global`/`--cwd` carry the
/// target instead).
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
struct FakeNpxSpawner {
    home: PathBuf,
    recorded: Mutex<Vec<(Vec<String>, Option<PathBuf>)>>,
}

impl FakeNpxSpawner {
    fn new(home: PathBuf) -> Self {
        FakeNpxSpawner {
            home,
            recorded: Mutex::new(Vec::new()),
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
        let dir = cwd.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {skill}\ndescription: installed by a fake CLI\n---\nBody.\n"),
        )
        .unwrap();
        let has_claude_code_agent = spec
            .args
            .windows(2)
            .any(|w| w[0] == "--agent" && w[1] == "claude-code");
        if has_claude_code_agent {
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
    let history_root = home.join(".history");
    let db_path = history_root.join("events.sqlite3");
    let scope = RuntimeScope::fixture(home);
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
    };
    Runtime::new(&scope, ports).unwrap()
}

fn runtime_for(home: &std::path::Path) -> Runtime {
    runtime_with(
        home,
        Arc::new(RealFs::new()),
        Some(Arc::new(FakeNpxSpawner::new(home.to_path_buf()))),
    )
}

fn copy_request(skill: &str) -> InstallRequest {
    InstallRequest {
        skill: SkillName(skill.to_string()),
        method: InstallMethod::Copy,
        scope: RootScope::Global,
        harnesses: vec![AgentId::from(AgentId::CLAUDE_CODE)],
        files: vec![InstallFile {
            relative_path: PathBuf::from("SKILL.md"),
            contents: format!("---\nname: {skill}\ndescription: a copied skill\n---\nBody.\n")
                .into_bytes(),
        }],
        source: None,
        trust_identity: None,
        trust_confirmed: false,
        save_as_preference: true,
    }
}

fn cli_request(skill: &str, method: InstallMethod) -> InstallRequest {
    InstallRequest {
        skill: SkillName(skill.to_string()),
        method,
        scope: RootScope::Global,
        harnesses: vec![AgentId::from(AgentId::CLAUDE_CODE)],
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
    assert_eq!(after.harnesses, vec![AgentId::from(AgentId::CLAUDE_CODE)]);

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
/// the skill's own bytes.
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
        deployment.join("SKILL.md").exists(),
        "the CLI's own write already landed before the registry write failed"
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
    };
    let rt = Runtime::new(&scope, ports).unwrap();

    let mut req = copy_request("iota");
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(project.clone()));
    let outcome = ops::install(&rt, &ctx(), &req).unwrap();
    assert!(matches!(outcome, InstallOutcome::Installed { .. }));

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
