// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so the
// same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk integration tests for `ops::remove`.
//!
//! Covers all four mutable owner kinds (`Copy`, `Fork`, `Dotagents`,
//! `SkillsSh`): `Copy`/`Fork` resolve through `ops::install`'s own registry
//! bookkeeping, and `Dotagents`/`SkillsSh` are classified by hand-writing the
//! same ledger files the real `npx skills`/`npx -y @sentry/dotagents` CLIs
//! leave behind (`.agents/.skill-lock.json`, `agents.lock`/`agents.toml`) -
//! `FakeNpxSpawner` itself only ever writes the skill's own tree, matching
//! the real CLI's actual scope.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use skill_studio_core::dto::{
    InstallFile, InstallMethod, InstallOutcome, InstallRequest, ListEventsRequest, RemoveRequest,
    RestoreRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{
    DeploymentId, LifecycleOwnerKind, RootKind, RootScope, SkillName,
};
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
/// `remove` deletes that same directory and drops the skill's own
/// `.skill-lock.json` entry, when one exists - both real `npx ... remove`
/// side effects this op's own post-call check (`remove_via_cli`) and the
/// CLI trace parity test below rely on.
struct FakeNpxSpawner {
    home: PathBuf,
    recorded: Mutex<Vec<(Vec<String>, Option<PathBuf>)>>,
    /// Set by [`FakeNpxSpawner::fail_next_call`]: the next `run` call
    /// returns a nonzero exit before touching disk, simulating an `npx`
    /// process crashing before it deletes anything - the CLI-based
    /// counterpart to `FailingFs::fail_next_rename` for `Copy`/`Fork`.
    fail_next: AtomicBool,
}

impl FakeNpxSpawner {
    fn new(home: PathBuf) -> Self {
        FakeNpxSpawner {
            home,
            recorded: Mutex::new(Vec::new()),
            fail_next: AtomicBool::new(false),
        }
    }

    fn fail_next_call(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
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
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Ok(ProcessOutput {
                status: Some(1),
                stdout: String::new(),
                stderr: "simulated npx crash".to_string(),
                timed_out: false,
            });
        }
        let cwd = spec.cwd.clone().unwrap_or_else(|| self.home.clone());
        if let Some(idx) = spec.args.iter().position(|a| a == "remove") {
            // The name is `remove`'s own next argument for both kinds:
            // `skills remove <name> --yes [--global]` and `-y
            // @sentry/dotagents [--project] remove <name>` - never the
            // argv's last element, which is a trailing flag for `SkillsSh`.
            let name = spec.args[idx + 1].clone();
            let dir = cwd.join(UNIVERSAL_ROOT_RELATIVE).join(&name);
            std::fs::remove_dir_all(&dir).ok();
            let lock_path = cwd.join(".agents").join(".skill-lock.json");
            if let Ok(bytes) = std::fs::read(&lock_path) {
                if let Ok(mut doc) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if let Some(skills) = doc.get_mut("skills").and_then(|v| v.as_object_mut()) {
                        skills.shift_remove(&name);
                    }
                    let _ = std::fs::write(&lock_path, serde_json::to_vec(&doc).unwrap());
                }
            }
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

/// Scans and finds `skill`'s universal-root deployment id, the shared tail
/// of every owner-kind setup below (`install_and_resolve`,
/// `mark_fork`/`mark_dotagents`/`mark_skills_sh`) - matching
/// `park_and_unpark.rs`'s own `universal_deployment_id`.
fn resolve_deployment_id(rt: &Runtime, skill: &str) -> DeploymentId {
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
        .unwrap_or_else(|| panic!("no universal deployment found for {skill}"))
        .id
        .clone()
}

/// Installs `skill` as a `Copy` deployment and returns its universal
/// deployment id, via the same `ops::install` -> `ops::scan` round trip
/// `park_and_unpark.rs`'s own `universal_deployment_id` uses.
fn install_and_resolve(rt: &Runtime, skill: &str) -> DeploymentId {
    let req = copy_request(skill);
    let InstallOutcome::Installed { .. } = ops::install(rt, &ctx(), &req).unwrap() else {
        panic!("expected Installed");
    };
    resolve_deployment_id(rt, skill)
}

/// Writes `skill`'s tree directly under the universal root, bypassing
/// `ops::install` entirely - `Dotagents`/`SkillsSh`/`Fork` setups build on
/// this instead of `install_and_resolve` because `ops::install`'s `Copy`
/// method itself writes a `copies` registry entry, which `classify_owner`
/// would then read back before ever reaching the ledger checks these owner
/// kinds depend on.
fn write_manual_universal_skill(home: &std::path::Path, skill: &str) {
    let dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(skill);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {skill}\ndescription: a manually placed skill\n---\nBody.\n"),
    )
    .unwrap();
}

/// Adds `skill` to `<home>/.agents/.skill-lock.json`, the ledger
/// `classify_owner` reads to classify a universal deployment as
/// `SkillsSh` (`lock_file::is_skill_installed`).
fn mark_skills_sh(home: &std::path::Path, skill: &str) {
    let agents_dir = home.join(".agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let lock_path = agents_dir.join(".skill-lock.json");
    let mut doc: serde_json::Value = std::fs::read(&lock_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| serde_json::json!({"version": 3, "skills": {}}));
    doc["skills"][skill] = serde_json::json!({
        "source": format!("owner/{skill}"),
        "sourceType": "github",
        "sourceUrl": format!("https://github.com/owner/{skill}"),
        "skillFolderHash": "deadbeef",
        "installedAt": "2024-01-01T00:00:00Z",
        "updatedAt": "2024-01-01T00:00:00Z",
    });
    std::fs::write(&lock_path, serde_json::to_vec(&doc).unwrap()).unwrap();
}

/// Adds `skill` to `<home>/.agents/agents.lock` and `agents.toml`, the
/// ledgers `classify_owner` reads to classify a universal deployment as
/// `Dotagents` (`ownership::read_dotagents_ledger`): an `agents.lock`
/// `[skills.<name>]` row alone yields `WildcardDotagents`, so both files
/// need the name for the real, non-wildcard `Dotagents` kind.
fn mark_dotagents(home: &std::path::Path, skill: &str) {
    let agents_dir = home.join(".agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(
        agents_dir.join("agents.lock"),
        format!("[skills.{skill}]\nsource = \"owner/{skill}\"\n"),
    )
    .unwrap();
    std::fs::write(
        agents_dir.join("agents.toml"),
        format!("[[skills]]\nname = \"{skill}\"\n"),
    )
    .unwrap();
}

/// Adds `skill` to `<home>/.agents/skill-studio.json`'s `forks` map, the
/// registry `classify_owner` reads to classify a universal deployment as
/// `Fork`. An empty `ForkRecord` (`{}`) matches any deployment id and
/// defaults its expected directory to `<home>/.agents/skills/<skill>` -
/// exactly where `write_manual_universal_skill` places it.
fn mark_fork(home: &std::path::Path, skill: &str) {
    let agents_dir = home.join(".agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let registry_path = agents_dir.join("skill-studio.json");
    let mut doc: serde_json::Value = std::fs::read(&registry_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    doc["forks"][skill] = serde_json::json!({});
    std::fs::write(&registry_path, serde_json::to_vec(&doc).unwrap()).unwrap();
}

/// Builds a `skill` deployment classified as `kind` and returns its
/// deployment id - the shared setup every four-kind loop in this file
/// drives, covering the `Copy`/`Fork`/`Dotagents`/`SkillsSh` owner kinds
/// `remove` treats as mutable (`LifecycleOwnerKind::is_mutable`).
fn setup_owner_kind(
    rt: &Runtime,
    home: &std::path::Path,
    kind: LifecycleOwnerKind,
    skill: &str,
) -> DeploymentId {
    match kind {
        LifecycleOwnerKind::Copy => install_and_resolve(rt, skill),
        LifecycleOwnerKind::Fork => {
            write_manual_universal_skill(home, skill);
            mark_fork(home, skill);
            resolve_deployment_id(rt, skill)
        }
        LifecycleOwnerKind::Dotagents => {
            write_manual_universal_skill(home, skill);
            mark_dotagents(home, skill);
            resolve_deployment_id(rt, skill)
        }
        LifecycleOwnerKind::SkillsSh => {
            write_manual_universal_skill(home, skill);
            mark_skills_sh(home, skill);
            resolve_deployment_id(rt, skill)
        }
        other => panic!("unsupported owner kind for remove test setup: {other:?}"),
    }
}

/// Every owner kind `remove` treats as mutable - the loop body for the
/// undo and crash tests below.
const MUTABLE_OWNER_KINDS: [LifecycleOwnerKind; 4] = [
    LifecycleOwnerKind::Copy,
    LifecycleOwnerKind::Fork,
    LifecycleOwnerKind::Dotagents,
    LifecycleOwnerKind::SkillsSh,
];

/// `remove_writes_a_journal_row_before_the_first_write_or_names_the_missing_step`:
/// a `Copy` removal leaves exactly one `done` `remove` event and takes the
/// deployment off the universal root. Kept `Copy`-only - the journaling
/// order this asserts does not vary by owner kind (see the undo and crash
/// tests below for the four-kind loop).
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

/// `copy_remove_moves_the_tree_into_quarantine_with_the_same_tree_hash_or_names_the_diverging_file`:
/// `Copy`'s removal never deletes the tree - it lands, intact, in
/// `.skill-studio-quarantine`, and the registry's `copies` entry for it is
/// gone.
#[test]
fn copy_remove_moves_the_tree_into_quarantine_with_the_same_tree_hash_or_names_the_diverging_file()
{
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

/// `remove_undo_restores_the_copy_tree_with_the_same_tree_hash_or_names_the_diverging_file`
/// (round 1, Q1): `restore_event` on a `Copy` removal's own event id must
/// bring the tree back to the universal root, byte-for-byte - it fails
/// without Q1's fix, whose `remove` recorded `inverse: None` for every
/// branch, so `restore_event` returned `Unsupported` instead of moving
/// anything back.
#[test]
fn remove_undo_restores_the_copy_tree_with_the_same_tree_hash_or_names_the_diverging_file() {
    let home = unique_temp_dir("remove_undo_copy");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let deployment_id = install_and_resolve(&rt, "epsilon");

    let outcome = ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();
    let restored = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id,
            force: false,
        },
    )
    .unwrap();

    let restored_path = home.join(UNIVERSAL_ROOT_RELATIVE).join("epsilon");
    assert!(
        restored.restored_paths.contains(&restored_path),
        "restore must name the deployment path among what it put back: {:?}",
        restored.restored_paths
    );
    let tree_hash_after = skill_studio_core::tree_hash::tree_hash(&RealFs::new(), &restored_path)
        .expect("the tree must be back at the universal root, byte-for-byte");
    assert_eq!(
        outcome.tree_hash_before, tree_hash_after,
        "the undone tree must match the original hash exactly"
    );
}

/// `undo_after_remove_brings_the_tree_back_with_the_same_tree_hash_or_names_the_diverging_file`
/// (the issue's own literal name, coordinator round 2): the same undo
/// guarantee `remove_undo_restores_the_copy_tree_...` checks for `Copy`
/// alone, looped over all four owner kinds `remove` treats as mutable -
/// `Fork`, `Dotagents`, and `SkillsSh` share `remove`'s own
/// `backup_paths`/`restore_backup_inverse` call, which does not branch on
/// owner kind, so the same restore path must work for all of them.
#[test]
fn undo_after_remove_brings_the_tree_back_with_the_same_tree_hash_or_names_the_diverging_file() {
    for kind in MUTABLE_OWNER_KINDS {
        let home = unique_temp_dir(&format!("remove_undo_four_kinds_{kind:?}"));
        std::fs::create_dir_all(&home).unwrap();
        let rt = runtime_for(&home);
        let skill = format!("undo-{kind:?}").to_lowercase();
        let deployment_id = setup_owner_kind(&rt, &home, kind, &skill);

        let outcome = ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();
        let restored = ops::restore_event(
            &rt,
            &ctx(),
            &RestoreRequest {
                event_id: outcome.event_id,
                force: false,
            },
        )
        .unwrap();

        let restored_path = home.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);
        assert!(
            restored.restored_paths.contains(&restored_path),
            "{kind:?}: restore must name the deployment path among what it put back: {:?}",
            restored.restored_paths
        );
        let tree_hash_after = skill_studio_core::tree_hash::tree_hash(
            &RealFs::new(),
            &restored_path,
        )
        .unwrap_or_else(|e| {
            panic!("{kind:?}: the tree must be back at the universal root, byte-for-byte: {e}")
        });
        assert_eq!(
            outcome.tree_hash_before, tree_hash_after,
            "{kind:?}: the undone tree must match the original hash exactly"
        );
    }
}

/// `remove_crash_mid_rename_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder`
/// (the red check, extended in coordinator round 2 to all four owner
/// kinds): failing the single write `Copy`/`Fork`'s removal makes (a
/// rename) or `Dotagents`/`SkillsSh`'s removal makes (an `npx` call) must
/// never leave the deployment half-moved - either it is still at the
/// universal root (the before state) or, for `Copy`/`Fork`, it already
/// landed, complete, in quarantine (the after state; `Dotagents`/`SkillsSh`
/// have no "after" state to land in, since the CLI's own crash is injected
/// before it touches disk at all).
#[test]
fn remove_crash_mid_rename_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder() {
    for kind in MUTABLE_OWNER_KINDS {
        let home = unique_temp_dir(&format!("remove_crash_window_{kind:?}"));
        std::fs::create_dir_all(&home).unwrap();
        // One runtime for both the setup and the crashed remove -
        // `park_and_unpark.rs`'s own crash tests follow the same shape. Two
        // separate runtimes would each carry their own `FakeIds` counter
        // starting from zero, and the second call's event id would collide
        // with the first (both writing to the same `events.sqlite3`).
        let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
        let spawner = Arc::new(FakeNpxSpawner::new(home.clone()));
        let rt = runtime_with(&home, failing_fs.clone(), Some(spawner.clone()));
        let skill = format!("crash-{kind:?}").to_lowercase();
        let deployment_id = setup_owner_kind(&rt, &home, kind, &skill);

        match kind {
            LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork => failing_fs.fail_next_rename(),
            LifecycleOwnerKind::Dotagents | LifecycleOwnerKind::SkillsSh => {
                spawner.fail_next_call();
            }
            other => panic!("unsupported owner kind: {other:?}"),
        }
        let err = ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap_err();
        assert_eq!(err.code, skill_studio_core::ErrorCode::Io, "{kind:?}");

        let original = home.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);
        let quarantine_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(QUARANTINE_DIR_NAME);
        let landed = std::fs::read_dir(&quarantine_dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
        assert!(
            original.join("SKILL.md").exists() != landed,
            "{kind:?}: the deployment must be exactly one of: still at the universal root, or fully in quarantine - never neither or both"
        );
        if landed {
            let entry = std::fs::read_dir(&quarantine_dir)
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            assert!(entry.path().join("SKILL.md").exists(), "{kind:?}");
        }

        // Round 1, Q3: the row itself must record the crash, not just leave
        // the tree in a valid state - a `failed` `remove` row with its
        // `backup_dir` set, so a later prune (Q2) and a manual retry both
        // have something to find.
        let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
        let remove_row = events
            .iter()
            .find(|e| e.kind == "remove")
            .expect("the crashed remove must still have written its own row");
        assert_eq!(
            remove_row.status, "failed",
            "{kind:?}: a crashed remove must mark the row failed"
        );
        assert!(
            remove_row.backup_dir.is_some(),
            "{kind:?}: a failed remove must still have an archival backup_dir"
        );
    }
}

/// `quarantine_prune_keeps_the_entry_of_a_failed_remove_or_names_the_lost_entry`
/// (round 1, Q2): a `remove` whose tree already landed in quarantine but
/// whose own row finishes `Failed` (registry write-back fails after the
/// rename succeeds) must not have its own quarantine entry pruned by a
/// later removal that pushes the directory over the cap - without Q2's
/// skip-if-referenced-by-an-open-remove check, `prune_quarantine` sorted
/// every entry purely by age and could delete the very backup a retry or an
/// undo of the failed row still needs.
#[test]
fn quarantine_prune_keeps_the_entry_of_a_failed_remove_or_names_the_lost_entry() {
    let home = unique_temp_dir("remove_quarantine_keeps_failed");
    std::fs::create_dir_all(&home).unwrap();
    let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(
        &home,
        failing_fs.clone(),
        Some(Arc::new(FakeNpxSpawner::new(home.clone()))),
    );
    let deployment_id = install_and_resolve(&rt, "zeta");

    // The rename into quarantine succeeds; the registry write-back right
    // after it does not, so the row finishes `Failed` with its tree already
    // quarantined.
    failing_fs.fail_next_write_atomic();
    let err = ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::Io);

    let quarantine_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(QUARANTINE_DIR_NAME);
    let failed_entry = std::fs::read_dir(&quarantine_dir)
        .unwrap()
        .next()
        .expect("the failed remove's tree must have landed in quarantine")
        .unwrap()
        .file_name()
        .to_string_lossy()
        .into_owned();

    // Push the directory over the cap with fresh, uncontested removals -
    // enough that a naive oldest-first prune would reach the failed entry.
    let cap = skill_studio_core::doctor::QUARANTINE_RETENTION_CAP;
    for i in 0..=cap {
        let deployment_id = install_and_resolve(&rt, &format!("filler-{i}"));
        ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();
    }

    let remaining: Vec<String> = std::fs::read_dir(&quarantine_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        remaining.contains(&failed_entry),
        "the failed remove's own quarantine entry must survive later prunes: {remaining:?}"
    );
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

/// One recorded (here, hand-built) `npx skills remove` call's shape: the
/// argv, the tree it deletes, and the lock entry it drops. Mirrors
/// `install.rs`'s own `CliTrace`, plus `lock_entry_removed`, which that
/// fixture has no counterpart for since `add` never touches the lock file
/// itself (that's `npx skills add`'s job, upstream of what `FakeNpxSpawner`
/// stands in for).
#[derive(serde::Deserialize)]
struct RemoveCliTrace {
    program: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    files: Vec<CliTraceFile>,
    lock_entry_removed: String,
}

#[derive(serde::Deserialize)]
struct CliTraceFile {
    relative_path: PathBuf,
    content: String,
}

/// `cli_remove_matches_the_npx_skills_remove_trace_byte_for_byte_apart_from_timestamps_or_names_the_diverging_file`
/// (coordinator round 2, Q5 - not deferrable per the shared brief's "do not
/// skip the test"): the hand-built counterpart to `install.rs`'s own CLI
/// trace parity test, for `remove` instead of `add`. Recording a real `npx
/// skills remove` run needs a real `npx`, out of reach in this worktree -
/// unit 5.4 owns swapping this fixture for a recorded one
/// (`issue-3.9a-followup-a.md`). Seeds the fixture's pre-existing tree and a
/// matching `.skill-lock.json` entry directly on disk (not through
/// `FakeNpxSpawner`, which only ever writes what a real `add` call would),
/// then asserts `remove_via_cli`'s own argv/cwd match the trace exactly and
/// that both the tree and the lock entry are gone afterward.
#[test]
fn cli_remove_matches_the_npx_skills_remove_trace_byte_for_byte_apart_from_timestamps_or_names_the_diverging_file(
) {
    let fixture_bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cli_traces/skills_sh_remove_global_claude_code.trace.json"
    ))
    .unwrap();
    let trace: RemoveCliTrace = serde_json::from_slice(&fixture_bytes).unwrap();
    assert_eq!(
        trace.program, "npx",
        "the fixture's own program must be npx"
    );
    let skill = &trace.lock_entry_removed;

    let home = unique_temp_dir("remove_cli_trace_parity");
    std::fs::create_dir_all(&home).unwrap();
    let skill_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(skill);
    std::fs::create_dir_all(&skill_dir).unwrap();
    for file in &trace.files {
        let path = skill_dir.join(&file.relative_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &file.content).unwrap();
    }
    mark_skills_sh(&home, skill);

    let spawner = Arc::new(FakeNpxSpawner::new(home.clone()));
    let rt = runtime_with(&home, Arc::new(RealFs::new()), Some(spawner.clone()));
    let deployment_id = resolve_deployment_id(&rt, skill);

    ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();

    let recorded = spawner.recorded.lock().unwrap();
    assert_eq!(
        recorded.len(),
        1,
        "remove_via_cli must call npx exactly once"
    );
    let (args, cwd) = &recorded[0];
    assert_eq!(
        args, &trace.args,
        "remove_via_cli's argv drifted from the recorded skills.sh trace"
    );
    assert_eq!(
        cwd, &trace.cwd,
        "remove_via_cli's cwd drifted from the recorded skills.sh trace"
    );

    assert!(
        !skill_dir.exists(),
        "the CLI's remove call must take the deployment off disk"
    );

    let lock_bytes = std::fs::read(home.join(".agents").join(".skill-lock.json")).unwrap();
    let lock: serde_json::Value = serde_json::from_slice(&lock_bytes).unwrap();
    assert!(
        lock.get("skills").and_then(|s| s.get(skill)).is_none(),
        "the CLI's remove call must drop the lock entry named {skill:?}"
    );
}
