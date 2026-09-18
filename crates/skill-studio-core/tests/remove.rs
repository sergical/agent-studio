// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so the
// same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk integration tests for `ops::remove`.
//!
//! Reduced breadth, per this unit's follow-up notes: every case here removes
//! a `Copy` deployment. `SkillsSh`/`Dotagents` classification needs a real
//! `~/.agents/.skill-lock.json` / dotagents ledger entry on disk that
//! `FakeNpxSpawner` does not write, and `Fork` is not exercised either -
//! their own journal rows and crash windows are named as a follow-up, not
//! built here.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use skill_studio_core::dto::{
    InstallFile, InstallMethod, InstallOutcome, InstallRequest, ListEventsRequest, RemoveRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{RootKind, RootScope, SkillName};
use skill_studio_core::ops;
use skill_studio_core::ports::{
    CancelToken, Ports, ProcessOutput, ProcessSpawner, ProcessSpec, Runtime,
};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FailingFs, FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";
const QUARANTINE_DIR_NAME: &str = ".skill-studio-quarantine";

/// Stands in for `npx skills add|remove <name> ...` / `npx -y
/// @sentry/dotagents add|remove <name> ...`: `add` writes a minimal
/// `SKILL.md` the same shape `tests/install.rs`'s own fake spawner does;
/// `remove` deletes that same directory - real `npx ... remove` behavior
/// this op's own post-call check (`remove_via_cli`) relies on.
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
        let cwd = spec.cwd.clone().unwrap_or_else(|| self.home.clone());
        if spec.args.iter().any(|a| a == "remove") {
            let name = spec.args.last().expect("remove <name>").clone();
            let dir = cwd.join(UNIVERSAL_ROOT_RELATIVE).join(&name);
            std::fs::remove_dir_all(&dir).ok();
        } else {
            let skill = spec
                .args
                .iter()
                .position(|a| a == "--skill" || a == "--name")
                .and_then(|i| spec.args.get(i + 1))
                .expect("--skill or --name flag with a value")
                .clone();
            let dir = cwd.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {skill}\ndescription: installed by a fake CLI\n---\nBody.\n"),
            )
            .unwrap();
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
        harnesses: Vec::new(),
        files: vec![InstallFile {
            relative_path: PathBuf::from("SKILL.md"),
            contents: format!("---\nname: {skill}\ndescription: a copied skill\n---\nBody.\n")
                .into_bytes(),
        }],
        source: None,
        trust_identity: None,
        trust_confirmed: false,
        save_as_preference: false,
    }
}

/// Installs `skill` as a `Copy` deployment and returns its universal
/// deployment id, via the same `ops::install` -> `ops::scan` round trip
/// `park_and_unpark.rs`'s own `universal_deployment_id` uses.
///
/// Every case in this file removes a `Copy` deployment (see the module doc
/// for why `SkillsSh`/`Dotagents`/`Fork` aren't exercised here), so this
/// helper does not take a method.
fn install_and_resolve(rt: &Runtime, skill: &str) -> skill_studio_core::identity::DeploymentId {
    let req = copy_request(skill);
    let InstallOutcome::Installed { .. } = ops::install(rt, &ctx(), &req).unwrap() else {
        panic!("expected Installed");
    };
    let inventory = ops::scan(rt, &ctx(), &skill_studio_core::dto::ScanRequest::default()).unwrap();
    inventory
        .skills
        .iter()
        .find(|s| s.name.0 == skill)
        .and_then(|s| {
            s.deployments
                .iter()
                .find(|d| d.root.kind == RootKind::Universal)
        })
        .unwrap()
        .id
        .clone()
}

/// `remove_writes_a_journal_row_before_the_first_write_or_names_the_missing_step`:
/// a `Copy` removal leaves exactly one `done` `remove` event and takes the
/// deployment off the universal root.
///
/// `SkillsSh`/`Dotagents` are not exercised here: classifying a deployment
/// as either owner kind requires a real `~/.agents/.skill-lock.json` /
/// dotagents ledger entry on disk (`crate::ownership`'s classifier), which
/// `FakeNpxSpawner` does not write - named as a follow-up, not built here.
#[test]
fn remove_writes_a_journal_row_before_the_first_write_or_names_the_missing_step() {
    let home = unique_temp_dir("remove_journal_copy");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let skill = "alpha-copy";
    let deployment_id = install_and_resolve(&rt, skill);

    let outcome = ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();
    assert!(
        !outcome.tree_hash_before.is_empty(),
        "tree_hash_before must be the removed folder's real hash"
    );
    assert!(
        !home.join(UNIVERSAL_ROOT_RELATIVE).join(skill).exists(),
        "the deployment must be gone from the universal root"
    );

    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert_eq!(events.len(), 2, "an install event plus a remove event");
    assert_eq!(events[0].kind, "remove", "newest first");
    assert_eq!(events[0].status, "done");
}

/// `copy_remove_moves_the_tree_into_quarantine_with_the_same_tree_hash`:
/// `Copy`'s removal never deletes the tree - it lands, intact, in
/// `.skill-studio-quarantine`, and the registry's `copies` entry for it is
/// gone.
#[test]
fn copy_remove_moves_the_tree_into_quarantine_with_the_same_tree_hash() {
    let home = unique_temp_dir("remove_quarantine");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let deployment_id = install_and_resolve(&rt, "beta");

    let outcome = ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();
    let quarantine_path = outcome
        .quarantine_path
        .expect("Copy removal always names a quarantine path");
    assert!(quarantine_path.join("SKILL.md").exists());
    let tree_hash_after =
        skill_studio_core::tree_hash::tree_hash(&RealFs::new(), &quarantine_path).unwrap();
    assert_eq!(
        outcome.tree_hash_before, tree_hash_after,
        "quarantine must hold the exact same tree, not a rewritten copy"
    );

    let registry = std::fs::read_to_string(home.join(".agents").join("skill-studio.json")).unwrap();
    let registry: serde_json::Value = serde_json::from_str(&registry).unwrap();
    let copies = registry.get("copies").and_then(|v| v.as_object());
    assert!(
        copies.is_none_or(|c| !c
            .values()
            .any(|v| v.get("name").and_then(|n| n.as_str()) == Some("beta"))),
        "the removed copy's registry entry must be dropped"
    );
}

/// `remove_crash_mid_rename_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder`
/// (the red check): failing the single rename `Copy`'s removal makes must
/// never leave the deployment half-moved - either it is still at the
/// universal root (the before state) or it already landed, complete, in
/// quarantine (the after state).
#[test]
fn remove_crash_mid_rename_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder() {
    let home = unique_temp_dir("remove_crash_window");
    std::fs::create_dir_all(&home).unwrap();
    // One runtime for both the setup install and the crashed remove -
    // `park_and_unpark.rs`'s own crash tests follow the same shape. Two
    // separate runtimes would each carry their own `FakeIds` counter
    // starting from zero, and the second install's event id would collide
    // with the first (both writing to the same `events.sqlite3`).
    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(
        &home,
        failing_fs.clone(),
        Some(Arc::new(FakeNpxSpawner::new(home.clone()))),
    );
    let deployment_id = install_and_resolve(&rt, "gamma");

    failing_fs.fail_next_rename();
    let err = ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Io);

    let original = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    let quarantine_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(QUARANTINE_DIR_NAME);
    let landed = std::fs::read_dir(&quarantine_dir)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    assert!(
        original.join("SKILL.md").exists() != landed,
        "the deployment must be exactly one of: still at the universal root, or fully in quarantine - never neither or both"
    );
    if landed {
        let entry = std::fs::read_dir(&quarantine_dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert!(entry.path().join("SKILL.md").exists());
    }
}

/// `quarantine_stays_within_the_retention_cap_and_prunes_the_oldest_entries_or_names_the_stray_entry`:
/// a `Copy` removal that pushes the quarantine dir over
/// `QUARANTINE_RETENTION_CAP` prunes back down to the cap, oldest entries
/// first.
#[test]
fn quarantine_stays_within_the_retention_cap_and_prunes_the_oldest_entries_or_names_the_stray_entry(
) {
    let home = unique_temp_dir("remove_quarantine_cap");
    std::fs::create_dir_all(&home).unwrap();
    let quarantine_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(QUARANTINE_DIR_NAME);
    std::fs::create_dir_all(&quarantine_dir).unwrap();
    // Pre-seed the cap's worth of old entries, named so lexical order is
    // also arrival order - the same convention `remove` itself uses
    // (`<skill>-<event-id>`, and `EventId`'s ulid text sorts
    // chronologically).
    let cap = skill_studio_core::doctor::QUARANTINE_RETENTION_CAP;
    for i in 0..cap {
        let dir = quarantine_dir.join(format!("old-{i:04}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), b"---\nname: old\n---\n").unwrap();
    }
    let rt = runtime_for(&home);
    let deployment_id = install_and_resolve(&rt, "delta");

    ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();

    let remaining: Vec<String> = std::fs::read_dir(&quarantine_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        remaining.len(),
        cap,
        "quarantine must stay at the cap after a removal pushes it over, not grow unbounded: {remaining:?}"
    );
    assert!(
        !remaining.contains(&"old-0000".to_string()),
        "the oldest pre-existing entry must be the one pruned: {remaining:?}"
    );
}
