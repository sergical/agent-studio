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
/// install`: overwrites `<cwd or home>/.agents/skills/<skill>/SKILL.md` on
/// the real filesystem with fresh content, the same in-place rewrite the
/// real CLI leaves. `skills update` names its skill as the third argv
/// entry; `dotagents install` names none and refreshes every folder in the
/// skills root, then rewrites `agents.lock`.
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
    /// Harness skills directories (relative to `home`) where the fake CLI
    /// links the updated skill, as `npx skills update` does for every
    /// harness it knows. A link already there is left alone.
    links_into: Vec<&'static str>,
    /// Same, but the fake CLI writes a real folder instead of a link.
    copies_into: Vec<&'static str>,
}

impl FakeNpxUpdateSpawner {
    fn new(home: PathBuf, revision: &'static str) -> Self {
        FakeNpxUpdateSpawner {
            home,
            revision,
            recorded: Mutex::new(Vec::new()),
            links_into: Vec::new(),
            copies_into: Vec::new(),
        }
    }

    fn linking_into(mut self, dirs: &[&'static str]) -> Self {
        self.links_into = dirs.to_vec();
        self
    }

    fn copying_into(mut self, dirs: &[&'static str]) -> Self {
        self.copies_into = dirs.to_vec();
        self
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
        let cwd = spec.cwd.clone().unwrap_or_else(|| self.home.clone());
        let skills = if spec.args.iter().any(|a| a == "install") {
            // `dotagents install` refreshes every declared entry: rewrite
            // each folder already in the skills root, and the lock file.
            let root = cwd.join(UNIVERSAL_ROOT_RELATIVE);
            let lock_dir = if spec.cwd.is_some() {
                cwd.clone()
            } else {
                cwd.join(".agents")
            };
            std::fs::write(lock_dir.join("agents.lock"), "# rewritten by install\n").unwrap();
            std::fs::read_dir(root)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        } else {
            vec![spec.args.get(2).expect("skills update <name>").clone()]
        };
        for skill in skills {
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
            for dir in &self.links_into {
                let link = cwd.join(dir).join(&skill);
                if link.symlink_metadata().is_err() {
                    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(dir_of(&cwd, &skill), &link).unwrap();
                }
            }
            for dir in &self.copies_into {
                let folder = cwd.join(dir).join(&skill);
                std::fs::create_dir_all(&folder).unwrap();
                std::fs::write(folder.join("SKILL.md"), "real folder written by the CLI").unwrap();
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

fn dir_of(cwd: &std::path::Path, skill: &str) -> PathBuf {
    cwd.join(UNIVERSAL_ROOT_RELATIVE).join(skill)
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

        telemetry: std::sync::Arc::new(skill_studio_core::ports::NoopTelemetry),
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

/// Same as [`seed_installed_skill`], but two files - the crash test needs a
/// tree wide enough that `stage`'s own copy is more than a single rename,
/// so a mid-swap failure has an actual multi-file "before" tree to diverge
/// from a multi-file "after" tree.
fn seed_installed_skill_two_files(home: &std::path::Path, skill: &str, revision: &str) {
    let dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(skill);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {skill}\ndescription: seeded\n---\nBody at {revision}.\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("reference.md"),
        format!("Reference at {revision}.\n"),
    )
    .unwrap();
}

fn copy_request_two_files(skill: &str, revision: &str) -> UpdateRequest {
    UpdateRequest {
        skill: SkillName(skill.to_string()),
        method: InstallMethod::Copy,
        scope: RootScope::Global,
        files: vec![
            InstallFile {
                relative_path: PathBuf::from("SKILL.md"),
                contents: format!(
                    "---\nname: {skill}\ndescription: a copied skill\n---\nBody at {revision}.\n"
                )
                .into_bytes(),
                mode: None,
            },
            InstallFile {
                relative_path: PathBuf::from("reference.md"),
                contents: format!("Reference at {revision}.\n").into_bytes(),
                mode: None,
            },
        ],
        source: None,
        ref_pin: None,
    }
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
            mode: None,
        }],
        source: None,
        ref_pin: None,
    }
}

/// An `agents.toml` with comments the update must not lose, declaring
/// `delta` (pinned) and `other`.
const DECLARED_TOML: &str = "# skills I declared by hand\nversion = 1\n\n[[skills]]\nname = \"delta\" # pinned on purpose\nsource = \"o/r\"\nref = \"aaa\"\n\n[[skills]]\nname = \"other\"\nsource = \"o/r\"\n";
const LOCK_BEFORE: &str = "# lock before the update\n";

/// Writes `<home>/.agents/agents.toml` and its `agents.lock` beside it.
fn seed_dotagents_files(home: &std::path::Path, toml: &str) {
    let dir = home.join(".agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("agents.toml"), toml).unwrap();
    std::fs::write(dir.join("agents.lock"), LOCK_BEFORE).unwrap();
}

/// `update_writes_a_journal_row_and_quarantines_the_old_tree_before_the_swap_or_names_the_missing_step`:
/// a `Copy` update leaves exactly one `update` event, `done`, the fresh
/// bytes at the destination, and the previous tree moved (not deleted) into
/// the shared `.skill-studio-quarantine` folder (`doctor::QUARANTINE_DIR_NAME`,
/// U3: the same one the doctor prune and check sweep, not an
/// update-specific name a prune pass would never see) - proof the swap
/// quarantined the old folder rather than clobbering it in place. Fails if
/// `update_copy` were to call `fsops::stage`/`swap` with the old folder
/// deleted first instead of swapped, or if the journal row were dropped.
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
        .join(".skill-studio-quarantine");
    let entries: Vec<_> = std::fs::read_dir(&quarantine)
        .unwrap_or_else(|e| panic!("expected a quarantine directory at {quarantine:?}: {e}"))
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

/// `undo_after_an_update_restores_the_previous_tree_with_the_same_tree_hash_or_names_the_diverging_file`:
/// the `update` event's backup-and-inverse round-trips through
/// `ops::restore_event` - without `force` - back to the pre-update tree,
/// with the same `TreeHash` the update reported as `tree_hash_before`.
/// Fails if `update` were to skip `backup_paths` before its first write
/// (nothing to restore from), record `EventKind::Install` instead of
/// `Update` (the SQL history reader would then reject it as the wrong
/// shape), or finish the row with `post_fingerprint: None` (U1: that would
/// tell `restore_event` the path was "absent" after the update, so even an
/// undrifted restore would return `DriftConflict` instead of restoring).
#[test]
fn undo_after_an_update_restores_the_previous_tree_with_the_same_tree_hash_or_names_the_diverging_file(
) {
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
            force: false,
        },
    )
    .unwrap_or_else(|e| panic!("undo without force must succeed on an undrifted tree: {e}"));
    assert_eq!(restore.restored_paths, vec![destination.clone()]);

    let tree_hash_after_undo =
        skill_studio_core::tree_hash::tree_hash(rt.ports.fs.as_ref(), &destination).unwrap();
    assert_eq!(
        tree_hash_after_undo, tree_hash_before,
        "undo must bring the tree back to the exact pre-update TreeHash"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `undo_after_an_update_refuses_when_the_tree_changed_since_or_names_the_drift`:
/// a caller that edits a file after `update` lands, then tries an
/// unforced restore, must get `DriftConflict` naming the path - `update`'s
/// row records the post-write fingerprint, so `restore_event`'s live-vs-
/// recorded comparison has something real to catch the edit against.
#[test]
fn undo_after_an_update_refuses_when_the_tree_changed_since_or_names_the_drift() {
    let home = unique_temp_dir("update_undo_refuses_on_drift");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "zeta", "v1");
    let rt = runtime_for(&home, "v2");
    let destination = home.join(UNIVERSAL_ROOT_RELATIVE).join("zeta");

    let outcome = ops::update(&rt, &ctx(), &copy_request("zeta", "v2")).unwrap();

    std::fs::write(destination.join("SKILL.md"), "drifted after the update\n").unwrap();

    let err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id,
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::DriftConflict);
    assert_eq!(err.path.as_deref(), Some(destination.as_path()));

    std::fs::remove_dir_all(&home).ok();
}

/// `update_crash_after_each_step_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder`
/// (the red check, U2): unlike a fresh install, `update`'s `swap` runs over
/// an already-existing destination, so it takes `fsops::swap`'s
/// exchange-then-quarantine-move path, not a bare rename - `FailingFs`'s
/// `fsops_exchange` never advances the `fsops_rename` counter
/// (`testing.rs`), so failing only the 5th `fsops_rename` call (the old,
/// install-copied comment this replaces) never actually hits the exchange
/// itself. This loops `fail_nth_fsops_rename(1..=5)` - the journal's own
/// manifest/plan/`record_stage`/`record_swap` writes, plus the post-exchange
/// quarantine-move rename - and adds a `fail_next_fsops_exchange` case for
/// the one step that counter cannot reach: the exchange the module doc
/// calls out as the actual crash-critical commit point. Every case, on a
/// two-file tree, must leave the destination showing exactly the before
/// tree hash (nothing committed yet) or the after tree hash (the exchange
/// already landed), never a hash that matches neither - which a half-copied
/// `stage` or a half-exchanged `final_name` would produce.
#[test]
fn update_crash_after_each_step_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder()
{
    // Golden run, unfailing: the exact before/after `TreeHash`es every
    // failing attempt below is allowed to land on.
    let golden_home = unique_temp_dir("update_crash_window_golden");
    std::fs::create_dir_all(&golden_home).unwrap();
    seed_installed_skill_two_files(&golden_home, "gamma", "v1");
    let golden_rt = runtime_for(&golden_home, "v2");
    let golden_destination = golden_home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    let hash_before =
        skill_studio_core::tree_hash::tree_hash(golden_rt.ports.fs.as_ref(), &golden_destination)
            .unwrap();
    let golden_outcome =
        ops::update(&golden_rt, &ctx(), &copy_request_two_files("gamma", "v2")).unwrap();
    let hash_after = golden_outcome.tree_hash_after;
    assert_ne!(
        hash_before, hash_after,
        "the update must actually change the tree"
    );
    std::fs::remove_dir_all(&golden_home).ok();

    type FailureCase = (&'static str, fn(&FailingFs));
    let cases: Vec<FailureCase> = vec![
        ("rename-1", |fs: &FailingFs| fs.fail_nth_fsops_rename(1)),
        ("rename-2", |fs: &FailingFs| fs.fail_nth_fsops_rename(2)),
        ("rename-3", |fs: &FailingFs| fs.fail_nth_fsops_rename(3)),
        ("rename-4", |fs: &FailingFs| fs.fail_nth_fsops_rename(4)),
        ("rename-5", |fs: &FailingFs| fs.fail_nth_fsops_rename(5)),
        ("rename-6", |fs: &FailingFs| fs.fail_nth_fsops_rename(6)),
        ("exchange", |fs: &FailingFs| fs.fail_next_fsops_exchange()),
    ];
    for (label, apply_failure) in cases {
        let home = unique_temp_dir(&format!("update_crash_window_{label}"));
        std::fs::create_dir_all(&home).unwrap();
        seed_installed_skill_two_files(&home, "gamma", "v1");
        let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
        let rt = runtime_with(
            &home,
            failing_fs.clone(),
            Some(Arc::new(FakeNpxUpdateSpawner::new(home.clone(), "v2"))),
        );
        apply_failure(failing_fs.as_ref());

        let result = ops::update(&rt, &ctx(), &copy_request_two_files("gamma", "v2"));
        let e = result.expect_err(&format!("{label}: injected failure did not fire"));

        let destination = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
        assert!(
            destination.exists(),
            "{label}: the destination must never disappear entirely"
        );
        let hash_now =
            skill_studio_core::tree_hash::tree_hash(rt.ports.fs.as_ref(), &destination).unwrap();
        assert!(
            hash_now == hash_before || hash_now == hash_after,
            "{label}: destination tree hash {hash_now} matches neither the before ({hash_before}) \
             nor the after ({hash_after}) state - a crash at this step left a half-swapped tree"
        );

        let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
        assert_eq!(
            events.len(),
            1,
            "{label}: a crashed update left exactly one row, got error {e}"
        );
        assert_eq!(
            events[0].status, "failed",
            "{label}: a crash must mark the row failed, not leave it pending"
        );

        // Recovery: the next mutation session reconciles the interrupted
        // plan, sweeping any stray `.skill-studio-stage-*` folder, and a
        // retry (with the filesystem working again) completes the update
        // the crash could not.
        let session = MutationSession::begin(&rt, &ctx()).unwrap();
        session.finish(&rt, &ctx());
        let retry = ops::update(&rt, &ctx(), &copy_request_two_files("gamma", "v2")).unwrap();
        let bytes = std::fs::read_to_string(retry.deployment_path.join("SKILL.md")).unwrap();
        assert!(
            bytes.contains("Body at v2"),
            "{label}: the retry must land the fresh bytes: {bytes}"
        );

        std::fs::remove_dir_all(&home).ok();
    }
}

/// `cli_update_spawns_the_npx_skills_update_argv_and_lands_the_new_revision_or_names_the_diverging_arg`
/// (the CLI parity test, per `definition-of-done.md` check 4): replays a
/// hand-built (not a checked-in fixture file, and not recorded from a real
/// `npx` run - unit 5.4 owns recording one) trace of `npx skills update
/// <name> --global` and `npx -y @sentry/dotagents add <source> --name
/// <name>` against `ops::update`. What this actually proves: the argv
/// `update_cli_args_and_cwd` built matches the trace's own argv exactly, and
/// the file the fake CLI wrote lands at the destination with the expected
/// revision marker in it - not a full result-tree byte diff against a
/// recorded trace, which check 4 in full would need a real `npx` capture
/// for (5.4's job).
#[test]
fn cli_update_spawns_the_npx_skills_update_argv_and_lands_the_new_revision_or_names_the_diverging_arg(
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
            vec!["-y", "@sentry/dotagents", "install"],
        ),
    ] {
        let home = unique_temp_dir(&format!("update_cli_parity_{label}"));
        std::fs::create_dir_all(&home).unwrap();
        seed_installed_skill(&home, "delta", "v1");
        seed_dotagents_files(&home, DECLARED_TOML);
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

/// `update_with_an_unreadable_registry_fails_before_the_first_write_or_names_the_stray_tree`
/// (round 1, U4): a `Copy` update over a corrupt `.agents/skill-studio.json`
/// (not a JSON object, so `read_registry_document` refuses it) must fail
/// before `update_copy`'s swap ever runs - the old tree still on disk, no
/// journal row - not after the swap has already landed the new tree with a
/// stray, unrecorded copy in the registry.
#[test]
fn update_with_an_unreadable_registry_fails_before_the_first_write_or_names_the_stray_tree() {
    let home = unique_temp_dir("update_unreadable_registry");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "theta", "v1");
    let registry_dir = home.join(".agents");
    std::fs::create_dir_all(&registry_dir).unwrap();
    std::fs::write(registry_dir.join("skill-studio.json"), b"[]").unwrap();
    let rt = runtime_for(&home, "v2");

    let err = ops::update(&rt, &ctx(), &copy_request("theta", "v2")).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Io);

    let destination = home.join(UNIVERSAL_ROOT_RELATIVE).join("theta");
    let bytes = std::fs::read_to_string(destination.join("SKILL.md")).unwrap();
    assert!(
        bytes.contains("Body at v1"),
        "an unreadable registry must fail before update_copy's swap lands the new tree: {bytes}"
    );

    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert!(
        events.is_empty(),
        "no journal row when the registry read fails before backup_paths"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `update_without_a_source_records_no_journal_row_or_names_the_stray_row`
/// (round 1, U5): a `Dotagents` update with no `source` must fail before
/// `backup_paths` records anything - `validate_cli_request` runs ahead of
/// the journal row, so this leaves no `update` event at all. Without this
/// ordering, the stray row `backup_paths` would have already recorded
/// stays `pending` forever (nothing ever calls `finish` on it), not
/// `failed`.
#[test]
fn update_without_a_source_records_no_journal_row_or_names_the_stray_row() {
    let home = unique_temp_dir("update_missing_source");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "iota", "v1");
    let rt = runtime_for(&home, "v2");

    let mut req = cli_request("iota", InstallMethod::Dotagents);
    req.source = None;
    let err = ops::update(&rt, &ctx(), &req).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::InvalidRequest);

    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert!(
        events.is_empty(),
        "a missing source must leave no journal row, not a stray pending one: {events:?}"
    );

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

/// `dotagents_update_with_a_new_ref_runs_install_edits_only_the_ref_and_undo_restores_toml_lock_and_folder`:
/// a pinned dotagents update spawns `dotagents install` (not `add`, which
/// fails on repos whose marketplace lists `"source": "./"`), rewrites only
/// the `ref` of the named `[[skills]]` entry with its comments intact, and
/// one undo puts `agents.toml`, `agents.lock` and the skill folder back
/// together. Fails if the argv falls back to `add`, if the edit reformats
/// or drops comments, or if undo restores the folder but leaves the ref and
/// lock at the new commit.
#[test]
fn dotagents_update_with_a_new_ref_runs_install_edits_only_the_ref_and_undo_restores_toml_lock_and_folder(
) {
    let home = unique_temp_dir("update_dotagents_pinned");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "delta", "v1");
    seed_dotagents_files(&home, DECLARED_TOML);
    let spawner = Arc::new(FakeNpxUpdateSpawner::new(home.clone(), "v2"));
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(spawner.clone()));

    let mut req = cli_request("delta", InstallMethod::Dotagents);
    req.ref_pin = Some("bbb".to_string());
    let outcome = ops::update(&rt, &ctx(), &req).unwrap();

    let recorded = spawner.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, vec!["-y", "@sentry/dotagents", "install"]);
    drop(recorded);

    let toml_path = home.join(".agents/agents.toml");
    let edited = std::fs::read_to_string(&toml_path).unwrap();
    assert_eq!(
        edited,
        DECLARED_TOML.replace("ref = \"aaa\"", "ref = \"bbb\""),
        "only the pinned entry's ref may change; comments and the other entry stay"
    );
    assert_ne!(
        std::fs::read_to_string(home.join(".agents/agents.lock")).unwrap(),
        LOCK_BEFORE,
        "the fake install must have rewritten the lock, or this test proves nothing about undo"
    );

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id,
            force: false,
        },
    )
    .unwrap_or_else(|e| panic!("undo of a dotagents update must succeed: {e}"));

    assert_eq!(std::fs::read_to_string(&toml_path).unwrap(), DECLARED_TOML);
    assert_eq!(
        std::fs::read_to_string(home.join(".agents/agents.lock")).unwrap(),
        LOCK_BEFORE
    );
    let folder =
        std::fs::read_to_string(home.join(UNIVERSAL_ROOT_RELATIVE).join("delta/SKILL.md")).unwrap();
    assert!(
        folder.contains("Body at v1"),
        "folder not restored: {folder}"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `dotagents_update_of_a_name_missing_from_agents_toml_is_refused_before_any_journal_row_or_process`:
/// `dotagents install` only refreshes declared entries, so an update for a
/// name with no `[[skills]]` row would run and change nothing. It must fail
/// with `InvalidRequest` before `backup_paths`, leave no journal row, spawn
/// nothing and leave `agents.toml` byte-for-byte alone.
#[test]
fn dotagents_update_of_a_name_missing_from_agents_toml_is_refused_before_any_journal_row_or_process(
) {
    let home = unique_temp_dir("update_dotagents_undeclared");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "stranger", "v1");
    seed_dotagents_files(&home, DECLARED_TOML);
    let spawner = Arc::new(FakeNpxUpdateSpawner::new(home.clone(), "v2"));
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(spawner.clone()));

    let mut req = cli_request("stranger", InstallMethod::Dotagents);
    req.ref_pin = Some("bbb".to_string());
    let err = ops::update(&rt, &ctx(), &req).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::InvalidRequest);

    assert!(spawner.recorded.lock().unwrap().is_empty());
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert!(events.is_empty(), "no journal row expected: {events:?}");
    assert_eq!(
        std::fs::read_to_string(home.join(".agents/agents.toml")).unwrap(),
        DECLARED_TOML
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `dotagents_update_writes_through_a_symlinked_agents_toml_and_keeps_the_link`:
/// a dotfiles repo often links `~/.agents/agents.toml`; the ref edit must
/// land in the linked file, not replace the link with a regular file.
#[cfg(unix)]
#[test]
fn dotagents_update_writes_through_a_symlinked_agents_toml_and_keeps_the_link() {
    let home = unique_temp_dir("update_dotagents_symlinked_toml");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "delta", "v1");
    seed_dotagents_files(&home, DECLARED_TOML);
    let real = home.join("dotfiles-agents.toml");
    let link = home.join(".agents/agents.toml");
    std::fs::rename(&link, &real).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let rt = runtime_for(&home, "v2");

    let mut req = cli_request("delta", InstallMethod::Dotagents);
    req.ref_pin = Some("bbb".to_string());
    ops::update(&rt, &ctx(), &req).unwrap();

    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the update replaced the symlinked agents.toml with a regular file"
    );
    assert!(std::fs::read_to_string(&real)
        .unwrap()
        .contains("ref = \"bbb\""));

    std::fs::remove_dir_all(&home).ok();
}

/// Fails every call the way a real `npx` does: npm chatter, then the
/// tool's own error line, exit status 1.
struct FailingNpxSpawner;

impl ProcessSpawner for FailingNpxSpawner {
    fn run(
        &self,
        _spec: &ProcessSpec,
        _cancel: &dyn CancelToken,
    ) -> Result<ProcessOutput, skill_studio_core::CoreError> {
        Ok(ProcessOutput {
            status: Some(1),
            stdout: String::new(),
            stderr:
                "npm notice New version available\nerror: could not fetch o/r\nnpm notice done\n"
                    .to_string(),
            timed_out: false,
        })
    }
}

/// `a_failed_cli_update_says_which_command_failed_with_the_tool_line_and_no_npm_chatter`:
/// the message users see must read `dotagents install failed: ...` /
/// `skills update failed: ...` with the tool's own error line, not `npx
/// exited with Some(1): ...` followed by `npm notice` noise.
#[test]
fn a_failed_cli_update_says_which_command_failed_with_the_tool_line_and_no_npm_chatter() {
    for (method, expected) in [
        (
            InstallMethod::Dotagents,
            "dotagents install failed: error: could not fetch o/r",
        ),
        (
            InstallMethod::SkillsSh,
            "skills update failed: error: could not fetch o/r",
        ),
    ] {
        let home = unique_temp_dir("update_cli_failure_text");
        std::fs::create_dir_all(&home).unwrap();
        seed_installed_skill(&home, "delta", "v1");
        seed_dotagents_files(&home, DECLARED_TOML);
        let rt = runtime_with(
            &home,
            Arc::new(RealFs::new()),
            Some(Arc::new(FailingNpxSpawner)),
        );

        let err = ops::update(&rt, &ctx(), &cli_request("delta", method)).unwrap_err();
        assert_eq!(err.message, expected);

        std::fs::remove_dir_all(&home).ok();
    }
}

/// Flow: a dotagents update pins `delta` to a new ref, and `dotagents
/// install` fails. Expectation: `agents.toml` holds its original bytes,
/// including the old `ref`. Failure here means a failed update leaves the
/// declaration pinned to a ref that never installed, so the next
/// `dotagents install` fetches it.
#[test]
fn a_failed_dotagents_update_leaves_agents_toml_on_its_old_ref_or_names_the_pin_left_behind() {
    let home = unique_temp_dir("update_dotagents_failed_pin");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "delta", "v1");
    seed_dotagents_files(&home, DECLARED_TOML);
    let rt = runtime_with(
        &home,
        Arc::new(RealFs::new()),
        Some(Arc::new(FailingNpxSpawner)),
    );
    let mut req = cli_request("delta", InstallMethod::Dotagents);
    req.ref_pin = Some("bbb".to_string());

    ops::update(&rt, &ctx(), &req).unwrap_err();

    assert_eq!(
        std::fs::read_to_string(home.join(".agents/agents.toml")).unwrap(),
        DECLARED_TOML,
        "the failed install left agents.toml pinned to the new ref"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: a global skills.sh update over a skill only the Universal folder
/// holds, where the CLI links it into the Claude Code and Codex folders too.
/// Expectation: after the update neither harness folder holds the skill, and
/// the Universal folder carries the new revision.
/// A failure here means an update turned harnesses on that the user never
/// enabled, so the skill starts loading in tools that did not have it.
#[test]
fn update_removes_links_the_cli_added_for_harnesses_without_the_skill_or_names_the_leftover_link() {
    let home = unique_temp_dir("update_removes_added_links");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "zeta", "v1");
    let spawner = FakeNpxUpdateSpawner::new(home.clone(), "v2")
        .linking_into(&[".claude/skills", ".codex/skills"]);
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(Arc::new(spawner)));

    let outcome = ops::update(&rt, &ctx(), &cli_request("zeta", InstallMethod::SkillsSh)).unwrap();

    for dir in [".claude/skills", ".codex/skills"] {
        let leftover = home.join(dir).join("zeta");
        assert!(
            leftover.symlink_metadata().is_err(),
            "the CLI's link at {} must be removed",
            leftover.display()
        );
    }
    let bytes = std::fs::read_to_string(outcome.deployment_path.join("SKILL.md")).unwrap();
    assert!(bytes.contains("Body at v2"), "{bytes}");

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: the same update where the pi folder already had a link to the skill
/// before the update, and the CLI also adds a Codex link.
/// Expectation: the pi link survives; only the new Codex link goes.
/// A failure here means the update switched off a harness the user had on.
#[test]
fn update_keeps_a_harness_link_that_existed_before_or_names_the_removed_link() {
    let home = unique_temp_dir("update_keeps_existing_link");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "eta", "v1");
    let pi_link = home.join(".pi/agent/skills/eta");
    std::fs::create_dir_all(pi_link.parent().unwrap()).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir_of(&home, "eta"), &pi_link).unwrap();
    let spawner = FakeNpxUpdateSpawner::new(home.clone(), "v2")
        .linking_into(&[".pi/agent/skills", ".codex/skills"]);
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(Arc::new(spawner)));

    ops::update(&rt, &ctx(), &cli_request("eta", InstallMethod::SkillsSh)).unwrap();

    assert!(
        pi_link.symlink_metadata().unwrap().file_type().is_symlink(),
        "the pi link that existed before the update must stay"
    );
    assert!(home.join(".codex/skills/eta").symlink_metadata().is_err());

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: the CLI writes a real folder (not a link) into a harness folder
/// that did not hold the skill.
/// Expectation: the update still succeeds and the folder stays, since it is
/// not a link Skill Studio may delete.
/// A failure here means the update deleted a folder it did not create as a
/// link, or failed the whole update over it.
#[test]
fn update_leaves_a_real_folder_the_cli_wrote_in_place_or_names_the_deleted_folder() {
    let home = unique_temp_dir("update_keeps_real_folder");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "theta", "v1");
    let spawner = FakeNpxUpdateSpawner::new(home.clone(), "v2").copying_into(&[".codex/skills"]);
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(Arc::new(spawner)));

    ops::update(&rt, &ctx(), &cli_request("theta", InstallMethod::SkillsSh)).unwrap();

    assert!(home.join(".codex/skills/theta/SKILL.md").is_file());

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: a project-scope dotagents update where dotagents keeps
/// `agents.toml` and `agents.lock` in the project root (`resolveScope`),
/// with a pinned ref, then undo.
/// Expectation: `<project>/agents.toml` gets the new ref, the CLI runs
/// `--project install` with the project as cwd, and undo restores both root
/// files.
/// A failure here means the update looks for `<project>/.agents/agents.toml`
/// and refuses a project skill dotagents itself manages, or undo leaves the
/// root lock rewritten.
#[test]
fn project_dotagents_update_edits_the_project_root_agents_toml_runs_project_install_and_undo_restores_both_files(
) {
    let home = unique_temp_dir("update_dotagents_project_root");
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    seed_installed_skill(&project, "delta", "v1");
    std::fs::write(project.join("agents.toml"), DECLARED_TOML).unwrap();
    std::fs::write(project.join("agents.lock"), LOCK_BEFORE).unwrap();

    let spawner = Arc::new(FakeNpxUpdateSpawner::new(home.clone(), "v2"));
    let mut scope = RuntimeScope::fixture(&home);
    scope.projects = skill_studio_core::scope::ProjectSelection::Explicit {
        paths: vec![project.clone()],
    };
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(
            home.join(".history").join("events.sqlite3"),
        )),
        sink: Arc::new(RecordingSink::default()),
        spawner: Some(spawner.clone()),
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
        telemetry: std::sync::Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    let rt = Runtime::new(&scope, ports).unwrap();

    let mut req = cli_request("delta", InstallMethod::Dotagents);
    req.scope = RootScope::Project(skill_studio_core::identity::ProjectRef(project.clone()));
    req.ref_pin = Some("bbb".to_string());
    let outcome = ops::update(&rt, &ctx(), &req).unwrap();

    let recorded = spawner.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].0,
        vec!["-y", "@sentry/dotagents", "--project", "install"]
    );
    assert_eq!(recorded[0].1.as_deref(), Some(project.as_path()));
    drop(recorded);

    assert_eq!(
        std::fs::read_to_string(project.join("agents.toml")).unwrap(),
        DECLARED_TOML.replace("ref = \"aaa\"", "ref = \"bbb\"")
    );
    assert_ne!(
        std::fs::read_to_string(project.join("agents.lock")).unwrap(),
        LOCK_BEFORE,
        "the fake install must have rewritten the lock, or this test proves nothing about undo"
    );

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id,
            force: false,
        },
    )
    .unwrap_or_else(|e| panic!("undo of a project dotagents update must succeed: {e}"));

    assert_eq!(
        std::fs::read_to_string(project.join("agents.toml")).unwrap(),
        DECLARED_TOML
    );
    assert_eq!(
        std::fs::read_to_string(project.join("agents.lock")).unwrap(),
        LOCK_BEFORE
    );

    std::fs::remove_dir_all(&home).ok();
}

/// `copy_update_keeps_each_source_files_mode_or_names_the_file_that_lost_its_bits`:
/// Flow: a Copy update swaps in a tree whose `scripts/run.sh` is 0o755 and
/// whose `SKILL.md` is 0o640. Expectation: the deployed script is still 0o755
/// and `SKILL.md` keeps 0o640. A failure here means the update wrote bytes
/// with the process default mode, so an updated script is no longer
/// executable.
#[cfg(unix)]
#[test]
fn copy_update_keeps_each_source_files_mode_or_names_the_file_that_lost_its_bits() {
    use std::os::unix::fs::PermissionsExt;

    let home = unique_temp_dir("update_copy_keeps_modes");
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "alpha", "v1");
    let rt = runtime_for(&home, "v2");
    let mut req = copy_request("alpha", "v2");
    req.files[0].mode = Some(0o640);
    req.files.push(InstallFile {
        relative_path: PathBuf::from("scripts/run.sh"),
        contents: b"#!/bin/sh\necho hi\n".to_vec(),
        mode: Some(0o755),
    });

    let outcome = ops::update(&rt, &ctx(), &req).unwrap();

    let mode_of = |relative: &str| {
        std::fs::metadata(outcome.deployment_path.join(relative))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode_of("scripts/run.sh"), 0o755, "the script lost its bits");
    assert_eq!(mode_of("SKILL.md"), 0o640, "a plain file lost its own mode");

    std::fs::remove_dir_all(&home).ok();
}
