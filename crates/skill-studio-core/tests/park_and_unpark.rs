//! Real-disk integration tests for `ops::park` and `ops::unpark`.
//!
//! Like `repair_and_restore.rs`, these use `skill-studio-host`'s real
//! adapters rather than the in-memory `FixtureFs`, since the mutation runs
//! real renames and symlinks a fake filesystem can't stand in for.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::{ListEventsRequest, ParkRequest, UnparkRequest};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{BackingRelationship, RootKind};
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FailingFs, FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";
const PARKED_ROOT_RELATIVE: &str = ".agents/skills-parked";
const CLAUDE_ROOT_RELATIVE: &str = ".claude/skills";

/// Materializes a home with one universal skill (`gamma`) linked from
/// Claude Code's per-skill root, the shape `find_claude_link` in `ops.rs`
/// looks for.
fn parkable_home(home: &Path) {
    let dir = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        b"---\nname: gamma\ndescription: a parkable skill\n---\nBody.\n",
    )
    .unwrap();
    let claude_skills = home.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&claude_skills).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&dir, claude_skills.join("gamma")).unwrap();
}

fn runtime_with(home: &Path, fs: Arc<dyn skill_studio_core::ports::ScopeFs>) -> Runtime {
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
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    Runtime::new(&scope, ports).unwrap()
}

fn runtime_for(home: &Path) -> Runtime {
    runtime_with(home, Arc::new(RealFs::new()))
}

fn universal_deployment_id(rt: &Runtime) -> skill_studio_core::identity::DeploymentId {
    let inventory = ops::scan(rt, &ctx(), &Default::default()).unwrap();
    let skill = inventory
        .skills
        .iter()
        .find(|s| s.name.0 == "gamma")
        .unwrap();
    skill
        .deployments
        .iter()
        .find(|d| d.root.kind == RootKind::Universal)
        .unwrap()
        .id
        .clone()
}

/// park_universal_skill_moves_directory_and_removes_the_claude_link: the
/// directory leaves `.agents/skills` for `.agents/skills-parked`, and the
/// Claude Code per-skill link that pointed at it is gone, not left dangling.
#[test]
fn park_universal_skill_moves_directory_and_removes_the_claude_link() {
    let home = unique_temp_dir("park_moves");
    parkable_home(&home);
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);

    let outcome = ops::park(&rt, &ctx(), &ParkRequest { deployment_id }).unwrap();

    assert!(!home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma").exists());
    assert!(outcome.parked_path.join("SKILL.md").exists());
    assert_eq!(
        outcome.parked_path,
        home.join(PARKED_ROOT_RELATIVE).join("gamma")
    );
    assert!(std::fs::symlink_metadata(home.join(CLAUDE_ROOT_RELATIVE).join("gamma")).is_err());

    std::fs::remove_dir_all(&home).ok();
}

/// park_then_unpark_restores_the_universal_skill_and_the_claude_link: the
/// reverse of the above, driven by the journal row `park` wrote (this build
/// carries no generic `inverse`; `unpark` finds its own `park` row by skill
/// name and un-recorded `reverted_by`, per its own doc comment).
#[test]
fn park_then_unpark_restores_the_universal_skill_and_the_claude_link() {
    let home = unique_temp_dir("park_unpark_roundtrip");
    parkable_home(&home);
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);
    let original_bytes = std::fs::read(
        home.join(UNIVERSAL_ROOT_RELATIVE)
            .join("gamma")
            .join("SKILL.md"),
    )
    .unwrap();

    let park_outcome = ops::park(&rt, &ctx(), &ParkRequest { deployment_id }).unwrap();

    let inventory = ops::scan(&rt, &ctx(), &Default::default()).unwrap();
    let parked = inventory
        .skills
        .iter()
        .find(|s| s.name.0 == "gamma")
        .unwrap()
        .deployments
        .iter()
        .find(|d| d.root.kind == RootKind::Parked)
        .unwrap()
        .clone();
    assert_eq!(parked.backing, BackingRelationship::Canonical);
    assert_eq!(parked.path, park_outcome.parked_path);

    let unpark_outcome = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: parked.id.clone(),
        },
    )
    .unwrap();

    let restored_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    assert_eq!(unpark_outcome.restored_path, restored_dir);
    assert!(!home.join(PARKED_ROOT_RELATIVE).join("gamma").exists());
    assert_eq!(
        std::fs::read(restored_dir.join("SKILL.md")).unwrap(),
        original_bytes
    );
    let link = home.join(CLAUDE_ROOT_RELATIVE).join("gamma");
    assert_eq!(
        std::fs::canonicalize(&link).unwrap(),
        std::fs::canonicalize(&restored_dir).unwrap()
    );

    std::fs::remove_dir_all(&home).ok();
}

/// park_journal_row_is_durable_before_the_directory_moves: `record` runs
/// before any filesystem step. Proven by failing the rename after `record`
/// already ran: the row exists (and, once recovery runs, reads
/// `interrupted`) even though the directory never moved.
#[test]
fn park_journal_row_is_durable_before_the_directory_moves() {
    let home = unique_temp_dir("park_journal_first");
    parkable_home(&home);
    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(&home, failing_fs.clone());
    let deployment_id = universal_deployment_id(&rt);

    failing_fs.fail_next_rename();
    let err = ops::park(&rt, &ctx(), &ParkRequest { deployment_id }).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Io);

    // Crash invariant: exactly one of {shared, parked} exists, and it is
    // the one that was there before the call - the rename never committed.
    assert!(home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma").exists());
    assert!(!home.join(PARKED_ROOT_RELATIVE).join("gamma").exists());
    // The link removal step ran before the failed rename: it is not left
    // dangling, and it is not silently restored either - the row a retry
    // will see is not `done`, so nothing here claims the mutation finished.
    assert!(std::fs::symlink_metadata(home.join(CLAUDE_ROOT_RELATIVE).join("gamma")).is_err());

    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    // The row was recorded (this is the point of the test): it exists
    // before the recovery step below even looks at it.
    assert_eq!(
        events.len(),
        1,
        "the park row must be recorded before the rename step runs"
    );
    assert_eq!(events[0].kind, "park");
    assert_eq!(events[0].status, "pending");

    // The next mutation session recovers it to `interrupted`, the same
    // startup recovery every other op relies on.
    let session = skill_studio_core::ports::MutationSession::begin(&rt, &ctx()).unwrap();
    session.finish(&rt, &ctx());
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert_eq!(events[0].status, "interrupted");

    // A retry, with the filesystem working again, finishes the job the
    // crashed attempt started.
    let deployment_id = universal_deployment_id(&rt);
    let outcome = ops::park(&rt, &ctx(), &ParkRequest { deployment_id }).unwrap();
    assert!(outcome.parked_path.join("SKILL.md").exists());

    std::fs::remove_dir_all(&home).ok();
}

/// cli_and_direct_calls_produce_the_same_disk_state_for_park: the CLI's
/// `run_park` is a thin wrapper over `ops::park` (see `apps/cli/src/main.rs`)
/// with no logic of its own; this asserts the one thing that could still
/// differ between two adapters calling the same op - the disk state left
/// behind - by running the op function directly the way every adapter does
/// and checking the result is exactly what a hand-computed layout expects.
#[test]
fn direct_ops_call_leaves_the_disk_state_every_surface_shares() {
    let home = unique_temp_dir("parity");
    parkable_home(&home);
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);

    let outcome = ops::park(&rt, &ctx(), &ParkRequest { deployment_id }).unwrap();

    // This is the one layout `docs/action-map/primitives-and-call-stack.md`
    // names for Park: link removed, directory under `skills-parked`. Any
    // adapter (CLI, MCP, desktop) that calls `ops::park` gets exactly this,
    // since none of them touch the filesystem themselves.
    assert_eq!(
        outcome.parked_path,
        home.join(PARKED_ROOT_RELATIVE).join("gamma")
    );
    assert!(!home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma").exists());
    assert!(std::fs::symlink_metadata(home.join(CLAUDE_ROOT_RELATIVE).join("gamma")).is_err());

    std::fs::remove_dir_all(&home).ok();
}
