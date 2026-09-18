// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so the
// same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk integration tests for `ops::update`.
//!
//! Follows `install.rs`'s pattern: `skill-studio-host`'s real adapters,
//! since `Copy`'s stage/swap and `Dotagents`/`SkillsSh`'s CLI invocation
//! both write real bytes a fake filesystem can't stand in for.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use skill_studio_core::dto::{
    InstallFile, InstallMethod, ListEventsRequest, RestoreRequest, UpdateOutcome, UpdateRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{RootScope, SkillName};
use skill_studio_core::ops;
use skill_studio_core::ports::{
    CancelToken, MutationSession, Ports, ProcessOutput, ProcessSpawner, ProcessSpec, Runtime,
};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FailingFs, FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";

/// Stands in for `npx skills update <name> ...` / `npx -y @sentry/dotagents
/// add <source> --name <name> ...`: overwrites `<cwd or home>/.agents/
/// skills/<skill>/SKILL.md` on the real filesystem with fresh content, the
/// same in-place rewrite the real CLI leaves. Named by parsing the
/// `--name`/second-positional flag out of argv - `update_cli_args_and_cwd`
/// puts the name last for `SkillsSh` (`skills update <name>`) and after
/// `--name` for `Dotagents`.
///
/// Records every call's argv and cwd (`recorded`), so the parity test can
/// assert the exact shape `update_cli_args_and_cwd` built without
/// duplicating its own logic to predict it. This is a hand-built fixture
/// trace, not one recorded from a real `npx` run (unit 5.4 owns recording
/// one); see the crate-level report for that follow-up.
struct FakeNpxUpdateSpawner {
    home: PathBuf,
    revision: &'static str,
    recorded: Mutex<Vec<(Vec<String>, Option<PathBuf>)>>,
}

impl FakeNpxUpdateSpawner {
    fn new(home: PathBuf, revision: &'static str) -> Self {
        FakeNpxUpdateSpawner {
            home,
            revision,
            recorded: Mutex::new(Vec::new()),
        }
    }
}

impl ProcessSpawner for FakeNpxUpdateSpawner {
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
        let skill = if spec.args.first().map(String::as_str) == Some("skills") {
            spec.args.get(2).expect("skills update <name>").clone()
        } else {
            let i = spec
                .args
                .iter()
                .position(|a| a == "--name")
                .expect("--name flag");
            spec.args.get(i + 1).expect("a value after --name").clone()
        };
        let cwd = spec.cwd.clone().unwrap_or_else(|| self.home.clone());
        let dir = cwd.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {skill}\ndescription: updated by a fake CLI\n---\nBody at {}.\n",
                self.revision
            ),
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

fn runtime_for(home: &std::path::Path, revision: &'static str) -> Runtime {
    runtime_with(
        home,
        Arc::new(RealFs::new()),
        Some(Arc::new(FakeNpxUpdateSpawner::new(
            home.to_path_buf(),
            revision,
        ))),
    )
}

/// Writes a pre-existing skill directly to disk (standing in for an earlier
/// `ops::install` call, which this test file does not itself exercise), so
/// `ops::update` has an existing deployment to refresh.
fn seed_installed_skill(home: &std::path::Path, skill: &str, revision: &str) {
    let dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(skill);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {skill}\ndescription: seeded\n---\nBody at {revision}.\n"),
    )
    .unwrap();
}

fn cli_request(skill: &str, method: InstallMethod) -> UpdateRequest {
    UpdateRequest {
        skill: SkillName(skill.to_string()),
        method,
        scope: RootScope::Global,
        files: Vec::new(),
        source: Some(skill.to_string()),
        ref_pin: None,
    }
}

fn copy_request(skill: &str, revision: &str) -> UpdateRequest {
    UpdateRequest {
        skill: SkillName(skill.to_string()),
        method: InstallMethod::Copy,
        scope: RootScope::Global,
        files: vec![InstallFile {
            relative_path: PathBuf::from("SKILL.md"),
            contents: format!(
                "---\nname: {skill}\ndescription: a copied skill\n---\nBody at {revision}.\n"
            )
            .into_bytes(),
        }],
        source: None,
        ref_pin: None,
    }
}

/// `update_writes_a_journal_row_and_quarantines_the_old_tree_before_the_swap_or_names_the_missing_step`:
/// a `Copy` update leaves exactly one `update` event, `done`, the fresh
/// bytes at the destination, and the previous tree moved (not deleted) into
/// `.skill-studio-update-quarantine` - proof the swap quarantined the old
/// folder rather than clobbering it in place. Fails if `update_copy` were to
/// call `fsops::stage`/`swap` with the old folder deleted first instead of
/// swapped, or if the journal row were dropped.
#[test]
fn update_writes_a_journal_row_and_quarantines_the_old_tree_before_the_swap_or_names_the_missing_step(
) {
    let home = unique_temp_dir("update_journal_and_quarantine");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "alpha", "v1");
    let rt = runtime_for(&home, "v2");

    let req = copy_request("alpha", "v2");
    let outcome = ops::update(&rt, &ctx(), &req).unwrap();
    assert_eq!(
        outcome.deployment_path,
        home.join(UNIVERSAL_ROOT_RELATIVE).join("alpha")
    );

    let bytes = std::fs::read_to_string(outcome.deployment_path.join("SKILL.md")).unwrap();
    assert!(
        bytes.contains("Body at v2"),
        "the fresh bytes must land: {bytes}"
    );

    let quarantine = home
        .join(UNIVERSAL_ROOT_RELATIVE)
        .join(".skill-studio-update-quarantine");
    let entries: Vec<_> = std::fs::read_dir(&quarantine)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(entries.len(), 1, "exactly one quarantined old tree");
    let quarantined_bytes = std::fs::read_to_string(entries[0].path().join("SKILL.md")).unwrap();
    assert!(
        quarantined_bytes.contains("Body at v1"),
        "the quarantined folder must be the previous tree, not the new one: {quarantined_bytes}"
    );

    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert_eq!(events.len(), 1, "exactly one event recorded");
    assert_eq!(events[0].kind, "update");
    assert_eq!(events[0].status, "done");

    std::fs::remove_dir_all(&home).ok();
}

/// `undo_after_an_update_restores_the_previous_tree_with_the_same_tree_hash`:
/// the `update` event's backup-and-inverse round-trips through
/// `ops::restore_event` back to the pre-update tree, with the same
/// `TreeHash` the update reported as `tree_hash_before`. Fails if `update`
/// were to skip `backup_paths` before its first write (nothing to restore
/// from), or record `EventKind::Install` instead of `Update` (the SQL
/// history reader would then reject it as the wrong shape).
#[test]
fn undo_after_an_update_restores_the_previous_tree_with_the_same_tree_hash() {
    let home = unique_temp_dir("update_undo_restores_tree_hash");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "beta", "v1");
    let rt = runtime_for(&home, "v2");
    let destination = home.join(UNIVERSAL_ROOT_RELATIVE).join("beta");
    let tree_hash_before =
        skill_studio_core::tree_hash::tree_hash(rt.ports.fs.as_ref(), &destination).unwrap();

    let req = copy_request("beta", "v2");
    let outcome = ops::update(&rt, &ctx(), &req).unwrap();
    assert_eq!(outcome.tree_hash_before, tree_hash_before);
    assert_ne!(
        outcome.tree_hash_after, tree_hash_before,
        "the update must actually have changed the tree"
    );

    let restore = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id,
            force: true,
        },
    )
    .unwrap();
    assert_eq!(restore.restored_paths, vec![destination.clone()]);

    let tree_hash_after_undo =
        skill_studio_core::tree_hash::tree_hash(rt.ports.fs.as_ref(), &destination).unwrap();
    assert_eq!(
        tree_hash_after_undo, tree_hash_before,
        "undo must bring the tree back to the exact pre-update TreeHash"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `update_crash_after_each_step_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder`
/// (the red check): failing the landing rename inside `swap` must never
/// leave a half-swapped deployment - either the destination still shows the
/// old tree (before state) or the new one, complete (after state), never a
/// mix, and any stray stage folder left behind is exactly what the next
/// `MutationSession::begin`'s reconcile removes.
#[test]
fn update_crash_after_each_step_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder()
{
    let home = unique_temp_dir("update_crash_window");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "gamma", "v1");
    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(
        &home,
        failing_fs.clone(),
        Some(Arc::new(FakeNpxUpdateSpawner::new(home.clone(), "v2"))),
    );
    let req = copy_request("gamma", "v2");

    // Same counting rule `install.rs`'s own crash test documents: 1
    // (manifest), 2 (plan), 3 (record_stage), 4 (record_swap), 5 (the
    // rename/exchange `swap` itself runs) - the first `fsops_rename` this
    // update makes is `stage`'s own plan-manifest write, since `update`'s
    // `backup_paths` copy of the existing tree runs through `RealFs`'s
    // regular `fs::rename`-free `copy_recursive`, not `fsops_rename`.
    failing_fs.fail_nth_fsops_rename(5);
    let err = ops::update(&rt, &ctx(), &req).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Io);

    let destination = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    assert!(
        destination.exists(),
        "the destination must never disappear entirely"
    );
    let bytes = std::fs::read_to_string(destination.join("SKILL.md")).unwrap();
    assert!(
        bytes.contains("Body at v1") || bytes.contains("Body at v2"),
        "the destination must show a complete tree, old or new, never a mix: {bytes}"
    );

    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert_eq!(events.len(), 1, "the crashed update left exactly one row");
    assert_eq!(
        events[0].status, "failed",
        "a crash mid-swap must mark the row failed, not leave it pending"
    );

    // Recovery: the next mutation session reconciles the interrupted plan,
    // sweeping any stray `.skill-studio-stage-*` folder, and a retry (with
    // the filesystem working again) completes the update a crash mid-swap
    // could not.
    let session = MutationSession::begin(&rt, &ctx()).unwrap();
    session.finish(&rt, &ctx());
    let retry = ops::update(&rt, &ctx(), &copy_request("gamma", "v2")).unwrap();
    let bytes = std::fs::read_to_string(retry.deployment_path.join("SKILL.md")).unwrap();
    assert!(
        bytes.contains("Body at v2"),
        "the retry must land the fresh bytes: {bytes}"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `cli_update_matches_the_npx_skills_update_trace_byte_for_byte_apart_from_timestamps_or_names_the_diverging_file`
/// (the CLI parity test, per `definition-of-done.md` check 4): replays a
/// hand-built fixture trace of `npx skills update <name> --global` and `npx
/// -y @sentry/dotagents add <source> --name <name>` against `ops::update`
/// and asserts the argv `update_cli_args_and_cwd` built and the resulting
/// tree both match the trace exactly. Hand-built, not recorded from a real
/// `npx` run - unit 5.4 owns recording one; replacing this fixture with a
/// recorded trace is a follow-up.
#[test]
fn cli_update_matches_the_npx_skills_update_trace_byte_for_byte_apart_from_timestamps_or_names_the_diverging_file(
) {
    for (label, method, expected_args) in [
        (
            "skills_sh",
            InstallMethod::SkillsSh,
            vec!["skills", "update", "delta", "--global"],
        ),
        (
            "dotagents",
            InstallMethod::Dotagents,
            vec!["-y", "@sentry/dotagents", "add", "delta", "--name", "delta"],
        ),
    ] {
        let home = unique_temp_dir(&format!("update_cli_parity_{label}"));
        std::fs::create_dir_all(&home).unwrap();
        seed_installed_skill(&home, "delta", "v1");
        let spawner = Arc::new(FakeNpxUpdateSpawner::new(home.clone(), "v2"));
        let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(spawner.clone()));

        let req = cli_request("delta", method);
        let outcome: UpdateOutcome = ops::update(&rt, &ctx(), &req).unwrap();

        let recorded = spawner.recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1, "{label}: exactly one npx call");
        let expected_args: Vec<String> = expected_args.into_iter().map(String::from).collect();
        assert_eq!(
            recorded[0].0, expected_args,
            "{label}: argv must match the fixture trace"
        );

        let bytes = std::fs::read_to_string(outcome.deployment_path.join("SKILL.md")).unwrap();
        assert!(
            bytes.contains("Body at v2"),
            "{label}: the fixture trace's own write must land verbatim: {bytes}"
        );

        std::fs::remove_dir_all(&home).ok();
    }
}

/// `update_over_a_missing_deployment_fails_before_any_write_or_names_the_created_folder`:
/// `update` refuses when nothing is installed at the destination yet,
/// writing no journal row and creating nothing - `install`, not `update`, is
/// the path that puts a first deployment on disk.
#[test]
fn update_over_a_missing_deployment_fails_before_any_write_or_names_the_created_folder() {
    let home = unique_temp_dir("update_missing_deployment");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home, "v2");

    let err = ops::update(&rt, &ctx(), &copy_request("epsilon", "v2")).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::InvalidRequest);
    assert!(!home.join(UNIVERSAL_ROOT_RELATIVE).join("epsilon").exists());
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert!(events.is_empty());

    std::fs::remove_dir_all(&home).ok();
}

/// `update_all_runs_each_skill_as_its_own_journal_entry_or_names_the_missing_row`:
/// a batch of three updates leaves three `update` events, one per skill, and
/// calls `on_outcome` once per skill in finishing order - the shape the
/// desktop's "update all" (3.6b) drives off the main thread.
#[test]
fn update_all_runs_each_skill_as_its_own_journal_entry_or_names_the_missing_row() {
    let home = unique_temp_dir("update_all_journal_per_skill");
    std::fs::create_dir_all(&home).unwrap();
    for name in ["one", "two", "three"] {
        seed_installed_skill(&home, name, "v1");
    }
    let rt = runtime_for(&home, "v2");

    let requests: Vec<UpdateRequest> = ["one", "two", "three"]
        .iter()
        .map(|name| copy_request(name, "v2"))
        .collect();
    let mut seen = Vec::new();
    let result = ops::update_all(&rt, &ctx(), &requests, |skill, outcome| {
        seen.push((skill.0.clone(), outcome.is_ok()));
    });
    assert_eq!(seen.len(), 3, "on_outcome must fire once per skill");
    assert!(seen.iter().all(|(_, ok)| *ok));
    assert_eq!(result.items.len(), 3);
    assert!(result.errors.is_empty());

    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert_eq!(events.len(), 3, "one journal row per skill");

    std::fs::remove_dir_all(&home).ok();
}
