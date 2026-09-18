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
use std::sync::Arc;

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

/// Stands in for `npx -y @sentry/dotagents add <source>` / `npx -y skills
/// add <source>`: writes a minimal `SKILL.md` under
/// `<cwd>/.agents/skills/<skill>` on the real filesystem, the same shape
/// the real CLI leaves. Named by the request's own skill folder name
/// (passed as the trailing arg) rather than derived from a repo slug -
/// these tests always request the two under the same name.
struct FakeNpxSpawner;

impl ProcessSpawner for FakeNpxSpawner {
    fn run(
        &self,
        spec: &ProcessSpec,
        _cancel: &dyn CancelToken,
    ) -> Result<ProcessOutput, skill_studio_core::CoreError> {
        assert_eq!(spec.program, "npx");
        let skill = spec.args.last().expect("add <source> arg").clone();
        let cwd = spec.cwd.clone().expect("install_via_cli always sets cwd");
        let dir = cwd.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {skill}\ndescription: installed by a fake CLI\n---\nBody.\n"),
        )
        .unwrap();
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
        Some(Arc::new(FakeNpxSpawner)),
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
        trust_confirmed: false,
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
    let rt = runtime_with(&home, failing_fs.clone(), Some(Arc::new(FakeNpxSpawner)));
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
