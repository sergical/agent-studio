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
    InstallFile, InstallLinkMode, InstallMethod, InstallRequest, ListEventsRequest, RestoreRequest,
    ScanRequest, UpdateOutcome, UpdateRequest,
};
use skill_studio_core::error::ErrorCode;
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{AgentId, LifecycleOwnerKind, RootScope, SkillName};
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
    /// Skill folders a `dotagents install` adds beside the declared ones,
    /// as it does for an entry another machine declared.
    installs_new: Vec<&'static str>,
}

impl FakeNpxUpdateSpawner {
    fn new(home: PathBuf, revision: &'static str) -> Self {
        FakeNpxUpdateSpawner {
            home,
            revision,
            recorded: Mutex::new(Vec::new()),
            links_into: Vec::new(),
            copies_into: Vec::new(),
            installs_new: Vec::new(),
        }
    }

    fn installing_new(mut self, skills: &[&'static str]) -> Self {
        self.installs_new = skills.to_vec();
        self
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
                .chain(self.installs_new.iter().map(|s| (*s).to_string()))
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
    let db_path = home.join(".history").join("events.sqlite3");
    runtime_with_history(
        home,
        fs,
        spawner,
        Arc::new(SqliteHistoryOpener::new(db_path)),
    )
}

fn runtime_with_history(
    home: &std::path::Path,
    fs: Arc<dyn skill_studio_core::ports::ScopeFs>,
    spawner: Option<Arc<dyn ProcessSpawner>>,
    history: Arc<dyn skill_studio_core::ports::HistoryOpener>,
) -> Runtime {
    let scope = RuntimeScope::fixture(home);
    let ports = Ports {
        fs,
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history,
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

/// Flow: install a Copy skill, update it to new bytes, then undo the update.
/// Expectation: the scan still classifies the restored folder as a `Copy`
/// deployment, because the registry's `content_hash` for it went back with
/// the bytes. Failure here means undo left the hash of the new bytes in the
/// registry, so the restored folder reads as unowned and loses its update
/// path.
#[test]
fn undo_of_a_copy_update_keeps_the_skill_owned_as_a_copy_or_names_the_stale_content_hash() {
    let home = unique_temp_dir("update_undo_copy_owner");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home, "v2");
    let files = |revision: &str| {
        vec![InstallFile {
            relative_path: PathBuf::from("SKILL.md"),
            contents: format!(
                "---\nname: eta\ndescription: a copied skill\n---\nBody at {revision}.\n"
            )
            .into_bytes(),
            mode: None,
        }]
    };
    ops::install(
        &rt,
        &ctx(),
        &InstallRequest {
            skill: SkillName("eta".to_string()),
            method: InstallMethod::Copy,
            scope: RootScope::Global,
            harnesses: vec![AgentId::from("universal")],
            destination: skill_studio_core::identity::SkillDestination::Universal,
            files: files("v1"),
            source: None,
            trust_identity: None,
            trust_confirmed: false,
            save_as_preference: false,
            link_mode: InstallLinkMode::Link,
        },
    )
    .unwrap();
    let owner_kinds = |rt: &Runtime| {
        ops::scan(rt, &ctx(), &ScanRequest::default())
            .unwrap()
            .skills
            .iter()
            .filter(|s| s.name.0 == "eta")
            .flat_map(|s| s.deployments.iter().map(|d| d.owner_kind))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        owner_kinds(&rt),
        vec![LifecycleOwnerKind::Copy],
        "setup: a fresh Copy install is owned as a Copy"
    );

    let mut req = copy_request("eta", "v2");
    req.files = files("v2");
    let outcome = ops::update(&rt, &ctx(), &req).unwrap();
    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id,
            force: false,
        },
    )
    .unwrap();

    assert_eq!(
        owner_kinds(&rt),
        vec![LifecycleOwnerKind::Copy],
        "after undoing the update the folder must still be owned as a Copy"
    );

    std::fs::remove_dir_all(&home).ok();
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

fn install_copy(rt: &Runtime, skill: &str) {
    ops::install(
        rt,
        &ctx(),
        &InstallRequest {
            skill: SkillName(skill.to_string()),
            method: InstallMethod::Copy,
            scope: RootScope::Global,
            harnesses: vec![AgentId::from("universal")],
            destination: skill_studio_core::identity::SkillDestination::Universal,
            files: copy_request(skill, "v1").files,
            source: None,
            trust_identity: None,
            trust_confirmed: false,
            save_as_preference: false,
            link_mode: InstallLinkMode::Link,
        },
    )
    .unwrap();
}

fn read_registry(home: &std::path::Path) -> serde_json::Value {
    let bytes = std::fs::read(home.join(".agents").join("skill-studio.json")).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn write_registry(home: &std::path::Path, doc: &serde_json::Value) {
    std::fs::write(
        home.join(".agents").join("skill-studio.json"),
        serde_json::to_vec(doc).unwrap(),
    )
    .unwrap();
}

fn copy_row_key(registry: &serde_json::Value, skill: &str) -> String {
    registry["copies"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, row)| row["name"] == skill)
        .unwrap_or_else(|| panic!("no copies row for {skill}: {registry}"))
        .0
        .clone()
}

/// Flow: Copy-update skill `theta`, then Copy-install skill `iota` and save
/// a preference, then undo the update of `theta`. Expectation: `theta`'s
/// `copies` row has its old `content_hash` back, and `iota`'s row and the
/// preference stay. Failure here means undo restored the whole registry
/// file from the update's backup and erased every later registry change.
#[test]
fn undo_of_a_copy_update_keeps_registry_rows_added_after_it_or_names_the_erased_key() {
    let home = unique_temp_dir("update_undo_keeps_later_registry_rows");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home, "v2");
    install_copy(&rt, "theta");
    let theta_key = copy_row_key(&read_registry(&home), "theta");
    let hash_before = read_registry(&home)["copies"][&theta_key]["content_hash"].clone();

    let outcome = ops::update(&rt, &ctx(), &copy_request("theta", "v2")).unwrap();
    assert_ne!(
        read_registry(&home)["copies"][&theta_key]["content_hash"],
        hash_before,
        "setup: the update must change the row's hash"
    );
    install_copy(&rt, "iota");
    let mut doc = read_registry(&home);
    doc["preferred_method"] = serde_json::json!("copy");
    write_registry(&home, &doc);

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id,
            force: false,
        },
    )
    .unwrap();

    let after = read_registry(&home);
    assert_eq!(after["copies"][&theta_key]["content_hash"], hash_before);
    copy_row_key(&after, "iota");
    assert_eq!(after["preferred_method"], "copy");

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: Copy-update `kappa`, then something else rewrites `kappa`'s
/// `copies` row, then undo the update. Expectation: undo refuses with
/// `DriftConflict` and leaves the row as it is; with `force` it restores the
/// old row. Failure here means undo overwrote a row it did not write.
#[test]
fn undo_of_a_copy_update_refuses_when_its_registry_row_changed_since_or_names_the_clobbered_row() {
    let home = unique_temp_dir("update_undo_registry_drift");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home, "v2");
    install_copy(&rt, "kappa");
    let key = copy_row_key(&read_registry(&home), "kappa");
    let hash_before = read_registry(&home)["copies"][&key]["content_hash"].clone();
    let outcome = ops::update(&rt, &ctx(), &copy_request("kappa", "v2")).unwrap();
    let mut doc = read_registry(&home);
    doc["copies"][&key]["content_hash"] = serde_json::json!("changed-by-someone-else");
    write_registry(&home, &doc);

    let err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id.clone(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::DriftConflict);
    assert_eq!(
        read_registry(&home)["copies"][&key]["content_hash"],
        "changed-by-someone-else"
    );

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id,
            force: true,
        },
    )
    .unwrap();
    assert_eq!(
        read_registry(&home)["copies"][&key]["content_hash"],
        hash_before
    );

    std::fs::remove_dir_all(&home).ok();
}

/// The `SQLite` history, except `patch_inverse` does nothing: the state a
/// crash between an op's write and its inverse patch leaves behind.
struct PatchlessHistory(SqliteHistoryOpener);

struct PatchlessStore(Box<dyn skill_studio_core::ports::HistoryStore>);

impl skill_studio_core::ports::HistoryOpener for PatchlessHistory {
    fn open(
        &self,
        scope: &skill_studio_core::scope::NormalizedScope,
        access: skill_studio_core::ports::HistoryAccess,
    ) -> Result<Option<Box<dyn skill_studio_core::ports::HistoryStore>>, skill_studio_core::CoreError>
    {
        Ok(self.0.open(scope, access)?.map(|store| {
            Box::new(PatchlessStore(store)) as Box<dyn skill_studio_core::ports::HistoryStore>
        }))
    }
}

impl skill_studio_core::ports::HistoryStore for PatchlessStore {
    fn list(
        &self,
        filter: &skill_studio_core::events::EventFilter,
    ) -> Result<Vec<skill_studio_core::events::EventRecord>, skill_studio_core::CoreError> {
        self.0.list(filter)
    }
    fn get(
        &self,
        id: &skill_studio_core::identity::EventId,
    ) -> Result<Option<skill_studio_core::events::EventRecord>, skill_studio_core::CoreError> {
        self.0.get(id)
    }
    fn backup_paths(
        &mut self,
        guard: &skill_studio_core::ports::ExclusiveGuard,
        id: &skill_studio_core::identity::EventId,
        paths: &[PathBuf],
    ) -> Result<skill_studio_core::events::BackupManifest, skill_studio_core::CoreError> {
        self.0.backup_paths(guard, id, paths)
    }
    fn record(
        &mut self,
        guard: &skill_studio_core::ports::ExclusiveGuard,
        id: &skill_studio_core::identity::EventId,
        draft: &skill_studio_core::events::EventDraft,
    ) -> Result<(), skill_studio_core::CoreError> {
        self.0.record(guard, id, draft)
    }
    fn finish(
        &mut self,
        guard: &skill_studio_core::ports::ExclusiveGuard,
        id: &skill_studio_core::identity::EventId,
        status: skill_studio_core::events::EventStatus,
        post_fingerprint: Option<skill_studio_core::identity::Fingerprint>,
    ) -> Result<(), skill_studio_core::CoreError> {
        self.0.finish(guard, id, status, post_fingerprint)
    }
    fn claim_revert(
        &mut self,
        guard: &skill_studio_core::ports::ExclusiveGuard,
        target: &skill_studio_core::identity::EventId,
        by: &skill_studio_core::identity::EventId,
    ) -> Result<bool, skill_studio_core::CoreError> {
        self.0.claim_revert(guard, target, by)
    }
    fn release_revert(
        &mut self,
        guard: &skill_studio_core::ports::ExclusiveGuard,
        target: &skill_studio_core::identity::EventId,
        restore: &skill_studio_core::identity::EventId,
    ) -> Result<bool, skill_studio_core::CoreError> {
        self.0.release_revert(guard, target, restore)
    }
    fn pending(
        &self,
    ) -> Result<Vec<skill_studio_core::events::EventRecord>, skill_studio_core::CoreError> {
        self.0.pending()
    }
    fn read_manifest(
        &self,
        backup_dir: &str,
    ) -> Result<skill_studio_core::events::BackupManifest, skill_studio_core::CoreError> {
        self.0.read_manifest(backup_dir)
    }
    fn read_backup_bytes(
        &self,
        backup_dir: &str,
        relative: &str,
    ) -> Result<Vec<u8>, skill_studio_core::CoreError> {
        self.0.read_backup_bytes(backup_dir, relative)
    }
    fn read_backup_files(
        &self,
        backup_dir: &str,
        relative: &str,
    ) -> Result<Vec<skill_studio_core::fsops::StageFile>, skill_studio_core::CoreError> {
        self.0.read_backup_files(backup_dir, relative)
    }
    fn patch_payload(
        &mut self,
        guard: &skill_studio_core::ports::ExclusiveGuard,
        id: &skill_studio_core::identity::EventId,
        patch: serde_json::Value,
    ) -> Result<(), skill_studio_core::CoreError> {
        self.0.patch_payload(guard, id, patch)
    }
}

/// Flow: Copy-update `lambda` through a history that drops every inverse
/// patch (a crash right after the write), then undo the update.
/// Expectation: `lambda`'s `copies` row gets its old `content_hash` back
/// with the old bytes. Failure here means the undo entry only exists once
/// the post-write patch lands, so a crash in that window leaves undo
/// restoring the old folder under the new hash, which reads as unowned.
#[test]
fn undo_of_a_copy_update_restores_the_registry_row_without_the_post_write_patch() {
    let home = unique_temp_dir("update_undo_registry_without_patch");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_with_history(
        &home,
        Arc::new(RealFs::new()),
        Some(Arc::new(FakeNpxUpdateSpawner::new(home.clone(), "v2"))),
        Arc::new(PatchlessHistory(SqliteHistoryOpener::new(
            home.join(".history").join("events.sqlite3"),
        ))),
    );
    install_copy(&rt, "lambda");
    let key = copy_row_key(&read_registry(&home), "lambda");
    let hash_before = read_registry(&home)["copies"][&key]["content_hash"].clone();
    let outcome = ops::update(&rt, &ctx(), &copy_request("lambda", "v2")).unwrap();
    assert_ne!(
        read_registry(&home)["copies"][&key]["content_hash"],
        hash_before,
        "setup: the update must change the row's hash"
    );

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id,
            force: false,
        },
    )
    .unwrap();

    assert_eq!(
        read_registry(&home)["copies"][&key]["content_hash"],
        hash_before
    );

    std::fs::remove_dir_all(&home).ok();
}

const SIBLINGS_TOML: &str = "version = 1\n\n[[skills]]\nname = \"alpha\"\nsource = \"o/r\"\nref = \"aaa\"\n\n[[skills]]\nname = \"beta\"\nsource = \"o/r\"\n";

/// Two declared skills, `alpha` pinned and `beta` not, both at v1 with the
/// dotagents files beside them. The fake install rewrites both folders.
fn siblings_home(name: &str) -> PathBuf {
    let home = unique_temp_dir(name);
    std::fs::create_dir_all(&home).unwrap();
    seed_installed_skill(&home, "alpha", "v1");
    seed_installed_skill(&home, "beta", "v1");
    seed_dotagents_files(&home, SIBLINGS_TOML);
    home
}

fn pinned_alpha_update(rt: &Runtime) -> UpdateOutcome {
    let mut req = cli_request("alpha", InstallMethod::Dotagents);
    req.ref_pin = Some("bbb".to_string());
    ops::update(rt, &ctx(), &req).unwrap()
}

fn skill_body(home: &std::path::Path, skill: &str) -> String {
    std::fs::read_to_string(
        home.join(UNIVERSAL_ROOT_RELATIVE)
            .join(skill)
            .join("SKILL.md"),
    )
    .unwrap()
}

fn undo(
    rt: &Runtime,
    event_id: skill_studio_core::identity::EventId,
    force: bool,
) -> Result<skill_studio_core::dto::RestoreOutcome, skill_studio_core::CoreError> {
    ops::restore_event(rt, &ctx(), &RestoreRequest { event_id, force })
}

/// Flow: `alpha` and unpinned `beta` sit at v1; updating `alpha` runs an
/// install that rewrites both folders, `agents.toml` and `agents.lock`. One
/// undo must put `beta`, `alpha`, the toml and the lock back to their v1
/// bytes. Fails when only `alpha` returns and `beta` stays at v2, a change
/// the update's backup never covered.
#[test]
fn undoing_a_dotagents_update_restores_the_sibling_folders_the_install_rewrote() {
    let home = siblings_home("update_siblings_undo");
    let rt = runtime_with(
        &home,
        Arc::new(RealFs::new()),
        Some(Arc::new(FakeNpxUpdateSpawner::new(home.clone(), "v2"))),
    );
    let outcome = pinned_alpha_update(&rt);
    assert!(skill_body(&home, "beta").contains("Body at v2"));

    undo(&rt, outcome.event_id, false).unwrap();

    for skill in ["alpha", "beta"] {
        assert!(
            skill_body(&home, skill).contains("Body at v1"),
            "{skill} not restored"
        );
    }
    assert_eq!(
        std::fs::read_to_string(home.join(".agents/agents.toml")).unwrap(),
        SIBLINGS_TOML
    );
    assert_eq!(
        std::fs::read_to_string(home.join(".agents/agents.lock")).unwrap(),
        LOCK_BEFORE
    );

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: after the update, the user edits `beta`. Undo must refuse and name
/// `beta`, leaving everything as it is. With force it restores v1 and keeps
/// the edit in the restore's own backup. Fails when undo overwrites the edit
/// without force, or when force leaves no copy of the edit.
#[test]
fn undoing_a_dotagents_update_refuses_over_an_edited_sibling_and_force_keeps_the_edit() {
    let home = siblings_home("update_siblings_edit");
    let rt = runtime_with(
        &home,
        Arc::new(RealFs::new()),
        Some(Arc::new(FakeNpxUpdateSpawner::new(home.clone(), "v2"))),
    );
    let outcome = pinned_alpha_update(&rt);
    let beta_md = home.join(UNIVERSAL_ROOT_RELATIVE).join("beta/SKILL.md");
    std::fs::write(&beta_md, "my edit to beta\n").unwrap();

    let err = undo(&rt, outcome.event_id.clone(), false).unwrap_err();

    assert_eq!(err.code, ErrorCode::DriftConflict);
    assert!(
        err.path.as_deref().is_some_and(|p| p.ends_with("beta")),
        "the refusal must name beta, got {:?}",
        err.path
    );
    assert_eq!(
        std::fs::read_to_string(&beta_md).unwrap(),
        "my edit to beta\n"
    );
    assert!(skill_body(&home, "alpha").contains("Body at v2"));

    let restored = undo(&rt, outcome.event_id, true).unwrap();

    assert!(skill_body(&home, "beta").contains("Body at v1"));
    assert!(skill_body(&home, "alpha").contains("Body at v1"));
    let restore_backup_has_edit = std::fs::read_dir(home.join(".history"))
        .unwrap()
        .flatten()
        .any(|entry| {
            walk_files(&entry.path())
                .iter()
                .any(|f| std::fs::read_to_string(f).is_ok_and(|t| t == "my edit to beta\n"))
        });
    assert!(
        restore_backup_has_edit,
        "force must back up the edit under restore {:?}",
        restored.restore_event_id
    );

    std::fs::remove_dir_all(&home).ok();
}

fn walk_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk_files(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// Flow: the install also creates `gamma`, a folder that did not exist
/// before. Undo must remove it along with restoring the declared skills.
/// Fails when `gamma` stays behind as a skill the update never announced.
#[test]
fn undoing_a_dotagents_update_removes_a_folder_the_install_created() {
    let home = siblings_home("update_siblings_new");
    let rt = runtime_with(
        &home,
        Arc::new(RealFs::new()),
        Some(Arc::new(
            FakeNpxUpdateSpawner::new(home.clone(), "v2").installing_new(&["gamma"]),
        )),
    );
    let outcome = pinned_alpha_update(&rt);
    assert!(home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma").is_dir());

    undo(&rt, outcome.event_id, false).unwrap();

    assert!(
        std::fs::symlink_metadata(home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma")).is_err(),
        "gamma must be gone after undo"
    );
    assert!(skill_body(&home, "beta").contains("Body at v1"));

    std::fs::remove_dir_all(&home).ok();
}

fn history_holds_text(home: &std::path::Path, text: &str) -> bool {
    std::fs::read_dir(home.join(".history"))
        .unwrap()
        .flatten()
        .any(|entry| {
            walk_files(&entry.path())
                .iter()
                .any(|f| std::fs::read_to_string(f).is_ok_and(|t| t == text))
        })
}

fn siblings_runtime(name: &str) -> (PathBuf, Runtime) {
    let home = siblings_home(name);
    let rt = runtime_with(
        &home,
        Arc::new(RealFs::new()),
        Some(Arc::new(FakeNpxUpdateSpawner::new(home.clone(), "v2"))),
    );
    (home, rt)
}

/// Flow: update, undo it, then edit `beta` and add a row to `agents.toml`
/// before undoing the undo. Without force the second undo must refuse for
/// drift and leave both edits; with force it proceeds, backs both edits up,
/// and puts the update's v2 state back. Fails when the undo's own inverse
/// does not vouch for the extra paths, so the second undo overwrites the
/// edits silently.
#[test]
fn undoing_an_update_undo_refuses_over_edits_made_since_and_force_backs_them_up() {
    let (home, rt) = siblings_runtime("update_undo_undo_drift");
    let outcome = pinned_alpha_update(&rt);
    let first_undo = undo(&rt, outcome.event_id, false).unwrap();
    let beta_md = home.join(UNIVERSAL_ROOT_RELATIVE).join("beta/SKILL.md");
    let toml = home.join(".agents/agents.toml");
    std::fs::write(&beta_md, "my edit to beta\n").unwrap();
    let edited_toml = format!("{SIBLINGS_TOML}\n[[skills]]\nname = \"mine\"\nsource = \"o/r\"\n");
    std::fs::write(&toml, &edited_toml).unwrap();

    let err = undo(&rt, first_undo.restore_event_id.clone(), false).unwrap_err();

    assert_eq!(err.code, ErrorCode::DriftConflict);
    assert_eq!(
        std::fs::read_to_string(&beta_md).unwrap(),
        "my edit to beta\n"
    );
    assert_eq!(std::fs::read_to_string(&toml).unwrap(), edited_toml);

    undo(&rt, first_undo.restore_event_id, true).unwrap();

    assert!(skill_body(&home, "beta").contains("Body at v2"));
    assert!(history_holds_text(&home, "my edit to beta\n"));
    assert!(history_holds_text(&home, &edited_toml));

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: the first dotagents update creates `agents.lock`. Undo must remove
/// it. When a symlink replaced it before a forced undo, the symlink goes
/// and the file it pointed at survives. Fails when undo follows the link
/// and deletes its target.
#[test]
fn undoing_a_first_time_lock_removes_it_and_a_replacing_symlink_never_loses_its_target() {
    for replaced_by_link in [false, true] {
        let home = unique_temp_dir("update_first_lock");
        std::fs::create_dir_all(home.join(".agents")).unwrap();
        seed_installed_skill(&home, "alpha", "v1");
        seed_installed_skill(&home, "beta", "v1");
        std::fs::write(home.join(".agents/agents.toml"), SIBLINGS_TOML).unwrap();
        let rt = runtime_with(
            &home,
            Arc::new(RealFs::new()),
            Some(Arc::new(FakeNpxUpdateSpawner::new(home.clone(), "v2"))),
        );
        let outcome = pinned_alpha_update(&rt);
        let lock = home.join(".agents/agents.lock");
        assert!(lock.is_file());
        let outside = home.join("outside.txt");
        if replaced_by_link {
            std::fs::write(&outside, "keep me\n").unwrap();
            std::fs::remove_file(&lock).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&outside, &lock).unwrap();
        }

        undo(&rt, outcome.event_id, replaced_by_link).unwrap();

        assert!(
            lock.symlink_metadata().is_err(),
            "lock must be gone (link: {replaced_by_link})"
        );
        if replaced_by_link {
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me\n");
        }

        std::fs::remove_dir_all(&home).ok();
    }
}

/// Flow: the install creates `gamma`; undo removes it; a real folder with a
/// user file now sits at `gamma`; a forced undo of that undo writes `gamma`
/// back. The user's file must stay recoverable from the restore's backup,
/// and a plain undo of that restore must put the folder back. Fails when the
/// forced write quarantines the folder with no backup, or when the second
/// undo writes the folder back and then deletes it as a copy to remove.
#[test]
fn forced_undo_over_a_real_folder_where_a_copy_goes_back_keeps_that_folder_in_the_backup() {
    let home = siblings_home("update_write_back_backup");
    let rt = runtime_with(
        &home,
        Arc::new(RealFs::new()),
        Some(Arc::new(
            FakeNpxUpdateSpawner::new(home.clone(), "v2").installing_new(&["gamma"]),
        )),
    );
    let outcome = pinned_alpha_update(&rt);
    let first_undo = undo(&rt, outcome.event_id, false).unwrap();
    let gamma = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    std::fs::create_dir_all(&gamma).unwrap();
    std::fs::write(gamma.join("notes.txt"), "mine\n").unwrap();

    let forced = undo(&rt, first_undo.restore_event_id, true).unwrap();

    assert!(skill_body(&home, "gamma").contains("Body at v2"));
    assert!(history_holds_text(&home, "mine\n"));

    undo(&rt, forced.restore_event_id, false).unwrap();

    assert_eq!(
        std::fs::read_to_string(gamma.join("notes.txt")).unwrap(),
        "mine\n"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// Runs the fake CLI, then makes `unreadable` unreadable, so the row's
/// side-effect patch cannot fingerprint it.
#[cfg(unix)]
struct UnreadableAfterInstall {
    inner: FakeNpxUpdateSpawner,
    unreadable: PathBuf,
}

#[cfg(unix)]
impl ProcessSpawner for UnreadableAfterInstall {
    fn run(
        &self,
        spec: &ProcessSpec,
        cancel: &dyn CancelToken,
    ) -> Result<ProcessOutput, skill_studio_core::CoreError> {
        use std::os::unix::fs::PermissionsExt;
        let output = self.inner.run(spec, cancel)?;
        std::fs::set_permissions(&self.unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
        Ok(output)
    }
}

#[cfg(unix)]
fn make_readable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
}

/// Flow: `beta` cannot be read when the update records its fingerprint, so
/// the row has no real post-state for it. After it is readable again, undo
/// must still refuse without force and name `beta`. Fails when the unreadable
/// path is dropped from the row, so undo overwrites `beta` unchecked.
#[cfg(unix)]
#[test]
fn undoing_an_update_that_could_not_read_a_sibling_refuses_without_force() {
    let home = siblings_home("update_unreadable_sibling");
    let beta_md = home.join(UNIVERSAL_ROOT_RELATIVE).join("beta/SKILL.md");
    let rt = runtime_with(
        &home,
        Arc::new(RealFs::new()),
        Some(Arc::new(UnreadableAfterInstall {
            inner: FakeNpxUpdateSpawner::new(home.clone(), "v2"),
            unreadable: beta_md.clone(),
        })),
    );
    let outcome = pinned_alpha_update(&rt);
    make_readable(&beta_md);

    let err = undo(&rt, outcome.event_id.clone(), false).unwrap_err();

    assert_eq!(err.code, ErrorCode::DriftConflict);
    assert!(
        err.path.as_deref().is_some_and(|p| p.ends_with("beta")),
        "the refusal must name beta, got {:?}",
        err.path
    );

    undo(&rt, outcome.event_id, true).unwrap();
    assert!(skill_body(&home, "beta").contains("Body at v1"));

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: a folder the install created cannot be read, so the row's whole
/// side-effect patch fails. The user then edits `beta`, and a forced undo
/// must still keep that edit in the restore's backup. Fails when the restore
/// backs up only the paths a successful patch listed.
#[cfg(unix)]
#[test]
fn undoing_an_update_whose_side_effect_patch_failed_still_backs_up_the_sibling_it_overwrites() {
    let home = siblings_home("update_failed_patch_backup");
    let rt = runtime_with(
        &home,
        Arc::new(RealFs::new()),
        Some(Arc::new(UnreadableAfterInstall {
            inner: FakeNpxUpdateSpawner::new(home.clone(), "v2").installing_new(&["gamma"]),
            unreadable: home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma/SKILL.md"),
        })),
    );
    let outcome = pinned_alpha_update(&rt);
    let beta_md = home.join(UNIVERSAL_ROOT_RELATIVE).join("beta/SKILL.md");
    std::fs::write(&beta_md, "my edit to beta\n").unwrap();

    undo(&rt, outcome.event_id, true).unwrap();

    assert!(skill_body(&home, "beta").contains("Body at v1"));
    assert!(history_holds_text(&home, "my edit to beta\n"));

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: the skills root cannot be listed, so the update cannot record which
/// folders exist before it. It must fail before any backup, leaving `alpha`
/// at v1. Fails when the update goes on and backs up or rewrites anything.
#[cfg(unix)]
#[test]
fn update_that_cannot_list_the_skills_root_fails_before_any_backup() {
    use std::os::unix::fs::PermissionsExt;
    let (home, rt) = siblings_runtime("update_unlistable_root");
    let root = home.join(UNIVERSAL_ROOT_RELATIVE);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o300)).unwrap();
    let mut req = cli_request("alpha", InstallMethod::Dotagents);
    req.ref_pin = Some("bbb".to_string());

    let result = ops::update(&rt, &ctx(), &req);

    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
    assert!(skill_body(&home, "alpha").contains("Body at v1"));
    assert!(
        walk_files(&home.join(".history")).iter().all(|f| f
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("events.sqlite3"))),
        "no backup may exist before the listing succeeds"
    );

    std::fs::remove_dir_all(&home).ok();
}
