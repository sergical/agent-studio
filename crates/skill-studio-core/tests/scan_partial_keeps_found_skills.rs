// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk integration test for `ops::scan`'s partial-root behaviour
//! (`docs/action-map/reads-and-snapshot.md`, "Desired state"; unit 3.3's
//! crash test). Like `park_and_unpark.rs`, this uses `skill-studio-host`'s
//! real adapter, wrapped in [`FailingFs`] to fail exactly one root's
//! `read_dir`.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::{Completeness, ScanRequest};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FailingFs, FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

const CLAUDE_ROOT_RELATIVE: &str = ".claude/skills";
const CODEX_ROOT_RELATIVE: &str = ".codex/skills";

fn write_skill(root: &Path, name: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: a scan-partial fixture skill\n---\nBody.\n"),
    )
    .unwrap();
}

/// A home with one skill under Claude Code's global root and one under
/// Codex's, so a failure reading one root still leaves the other's skill
/// reachable.
fn two_root_home(home: &Path) {
    write_skill(&home.join(CLAUDE_ROOT_RELATIVE), "claude-only");
    write_skill(&home.join(CODEX_ROOT_RELATIVE), "codex-only");
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

/// Given a home with a skill under each of two harness roots, when
/// `ops::scan` cannot read one root (`FailingFs` fails that root's
/// `read_dir` once), the scan still returns `Ok`, marks
/// `Completeness::Partial` with an observation naming the unreadable root,
/// and keeps the skill found under every other root it could read - it
/// does not fall back to an empty list just because one root failed.
#[test]
fn a_scan_error_sets_scan_partial_and_scan_observations_without_dropping_a_single_installed_skill()
{
    let home = unique_temp_dir("scan-partial-root");
    std::fs::create_dir_all(&home).expect("create home");
    let home = home.canonicalize().expect("canonicalize home");
    two_root_home(&home);

    let failing_root = home.join(CLAUDE_ROOT_RELATIVE).canonicalize().unwrap();
    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    failing_fs.fail_read_dir_for(failing_root.clone());

    let rt = runtime_with(&home, failing_fs);
    let inventory = ops::scan(&rt, &ctx(), &ScanRequest::default()).expect("scan must not error");

    assert_eq!(
        inventory.completeness,
        Completeness::Partial,
        "an unreadable root must mark the inventory partial, not fail the whole scan"
    );
    assert!(
        inventory
            .observations
            .iter()
            .any(|o| o.message.contains("could not read root")),
        "expected an observation naming the unreadable root, got {:#?}",
        inventory.observations
    );
    assert_eq!(
        inventory.unread_roots,
        vec![failing_root.clone()],
        "unread_roots must name the root a caller should scope a carried-over merge to"
    );

    let names: Vec<&str> = inventory.skills.iter().map(|s| s.name.0.as_str()).collect();
    assert!(
        names.contains(&"codex-only"),
        "a skill under a root that read successfully must still be in the list: {names:#?}"
    );
    assert!(
        !names.contains(&"claude-only"),
        "the skill under the unreadable root cannot be found this run: {names:#?}"
    );

    std::fs::remove_dir_all(&home).ok();
}
