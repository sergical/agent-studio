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
    AgentId, DeploymentId, LifecycleOwnerKind, RootKind, RootScope, SkillName,
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
const CLAUDE_ROOT_RELATIVE: &str = ".claude/skills";
/// A second, non-Claude harness root - round 3, N4: the real
/// `npx skills remove`/`npx -y @sentry/dotagents remove` only ever detects
/// and deletes the harnesses it knows about, so a link under a root it does
/// not touch is exactly the "harness the CLI does not detect" case
/// `remove_and_link`'s own link loop must still reach and remove.
const CODEX_ROOT_RELATIVE: &str = ".codex/skills";

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
            // The real CLI removes every detected agent's link when no
            // `--agent` is given (skills CLI v1.5.23 `dist/cli.mjs:6217-
            // 6263`), not just the tree - `remove_and_link`'s own link loop
            // (round 2, B1) must treat a link this already deleted as
            // already-removed rather than an error.
            let claude_link = cwd.join(CLAUDE_ROOT_RELATIVE).join(&name);
            std::fs::remove_file(&claude_link).ok();
            let lock_path = cwd.join(".agents").join(".skill-lock.json");
            if let Ok(bytes) = std::fs::read(&lock_path) {
                if let Ok(mut doc) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if let Some(skills) = doc.get_mut("skills").and_then(|v| v.as_object_mut()) {
                        skills.shift_remove(&name);
                    }
                    let _ = std::fs::write(&lock_path, serde_json::to_vec(&doc).unwrap());
                }
            }
            // The real `@sentry/dotagents remove` drops its own row from
            // `agents.lock`/`agents.toml` the same way `npx skills remove`
            // drops its `.skill-lock.json` row above - round 3, N5 needs
            // this so undo's provenance assertion sees a real "the CLI's own
            // lock entry is gone" state for `Dotagents`, not a stale row
            // this fake never touched. `mark_dotagents` only ever writes one
            // skill's row per test home, so dropping both files whole (never
            // called for a name that is not the one just marked) matches
            // this fixture's own scope, not a general TOML editor.
            let agents_lock_path = cwd.join(".agents").join("agents.lock");
            if std::fs::read_to_string(&agents_lock_path)
                .is_ok_and(|contents| contents.contains(&format!("[skills.{name}]")))
            {
                std::fs::remove_file(&agents_lock_path).ok();
                std::fs::remove_file(cwd.join(".agents").join("agents.toml")).ok();
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
    runtime_with_clock(home, fs, spawner, Arc::new(FakeClock::at(0)))
}

/// [`runtime_with`], with the clock a caller supplies instead of a fresh
/// `FakeClock` at the epoch - round 2, N4: the age-cap test needs to
/// advance a clock it still holds a handle to after the runtime is built.
fn runtime_with_clock(
    home: &std::path::Path,
    fs: Arc<dyn skill_studio_core::ports::ScopeFs>,
    spawner: Option<Arc<dyn ProcessSpawner>>,
    clock: Arc<FakeClock>,
) -> Runtime {
    let history_root = home.join(".history");
    let db_path = history_root.join("events.sqlite3");
    let scope = RuntimeScope::fixture(home);
    let ports = Ports {
        fs,
        clock,
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

/// `skill`'s universal deployment's own [`LifecycleOwnerKind`], via a fresh
/// `ops::scan` - round 3, N5: reads back how the classifier sees the
/// deployment right now, the same signal the desktop's provenance badge
/// would show.
fn resolve_owner_kind(rt: &Runtime, skill: &str) -> LifecycleOwnerKind {
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
        .owner_kind
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

/// [`setup_owner_kind`], plus a Claude Code per-skill link
/// (`.claude/skills/<skill>`) pointing at the universal deployment - round
/// 2, B1: the undo and crash loops need a real harness link in play so
/// `remove_and_link`'s link-removal step, and its `NotFound` tolerance, are
/// actually exercised for every owner kind, not skipped because
/// `find_all_links` found nothing. `Copy` gets its link the real way, by
/// asking `ops::install` for `claude-code`, which - like every link
/// `skill-studio-core` itself writes - is always absolute (`ScopeFs::symlink`
/// only ever takes a `confine`d, and so absolute, `ScopedPath`; see the
/// `set_claude_code_switch` comment in `ops.rs` on why there is no port
/// through which core could write a relative spelling even if it wanted to).
/// The other three kinds never go through `ops::install` (see
/// [`write_manual_universal_skill`]'s own doc), so their link is hand-made -
/// round 3, B1: written *relative*, the way the real CLI actually writes it
/// (`~/.claude/skills/<name> -> ../../.agents/skills/<name>`;
/// `skills/dist/cli.mjs:2268` calls `symlink(relativePath, linkPath)`), so
/// the undo path's relative-target resolution is exercised for real instead
/// of only ever seeing the absolute spelling `Copy`'s own link happens to
/// have.
fn setup_owner_kind_with_claude_link(
    rt: &Runtime,
    home: &std::path::Path,
    kind: LifecycleOwnerKind,
    skill: &str,
) -> DeploymentId {
    if kind == LifecycleOwnerKind::Copy {
        let mut req = copy_request(skill);
        req.harnesses = vec![AgentId::parse(AgentId::CLAUDE_CODE).unwrap()];
        let InstallOutcome::Installed { .. } = ops::install(rt, &ctx(), &req).unwrap() else {
            panic!("expected Installed");
        };
        return resolve_deployment_id(rt, skill);
    }
    let deployment_id = setup_owner_kind(rt, home, kind, skill);
    let claude_dir = home.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&claude_dir).unwrap();
    // The real CLI's own relative spelling - two `..` segments up from
    // `.claude/skills/` to `home`, then back down into
    // `.agents/skills/<skill>`.
    let relative_target = PathBuf::from("../..")
        .join(UNIVERSAL_ROOT_RELATIVE)
        .join(skill);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&relative_target, claude_dir.join(skill)).unwrap();
    deployment_id
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
        let deployment_id = setup_owner_kind_with_claude_link(&rt, &home, kind, &skill);
        let claude_link = home.join(CLAUDE_ROOT_RELATIVE).join(&skill);

        let outcome = ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();
        assert!(
            std::fs::symlink_metadata(&claude_link).is_err(),
            "{kind:?}: remove must take the Claude Code link down too"
        );
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

/// `undo_after_remove_restores_the_links_and_the_provenance_state_or_names_the_missing_path`
/// (round 2, N3; round 3, N5): the tree-hash guarantee the test above
/// checks, joined by two more - the Claude Code link `remove` took down
/// comes back pointing at the restored tree, and each owner kind's own
/// provenance ends up where `issue-3.9a-followup-a.md` documents it should.
/// For `Copy`/`Fork` (the two kinds with their own registry row) that row is
/// back too. `Dotagents`/`SkillsSh` have no registry row of `remove`'s own to
/// lose (see `CrashPoint`'s own doc) - undo does not, and is not meant to,
/// re-run the CLI, so the skill reads back as `Manual` once the CLI's own
/// lock entry is gone (item 10 of the follow-up doc), not as its original
/// owner kind.
#[test]
fn undo_after_remove_restores_the_links_and_the_provenance_state_or_names_the_missing_path() {
    for kind in MUTABLE_OWNER_KINDS {
        let home = unique_temp_dir(&format!("remove_undo_links_{kind:?}"));
        std::fs::create_dir_all(&home).unwrap();
        let rt = runtime_for(&home);
        let skill = format!("undo-links-{kind:?}").to_lowercase();
        let deployment_id = setup_owner_kind_with_claude_link(&rt, &home, kind, &skill);
        let claude_link = home.join(CLAUDE_ROOT_RELATIVE).join(&skill);
        let universal_path = home.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);

        let outcome = ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();
        ops::restore_event(
            &rt,
            &ctx(),
            &RestoreRequest {
                event_id: outcome.event_id,
                force: false,
            },
        )
        .unwrap();

        let raw_target = std::fs::read_link(&claude_link).unwrap_or_else(|e| {
            panic!("{kind:?}: the Claude Code link must be back after restore: {e}")
        });
        // Round 3, B1: the recorded target may have been relative (as a
        // hand-made non-`Copy` link is, matching the real CLI - see
        // `setup_owner_kind_with_claude_link`'s own doc), so the link's own
        // parent joins it into an absolute path before this compares it to
        // `universal_path`, the same resolution `restore_event` itself does
        // before recreating the link.
        let resolved_target = if raw_target.is_absolute() {
            raw_target
        } else {
            claude_link
                .parent()
                .expect("claude_link always has a parent")
                .join(&raw_target)
        };
        assert_eq!(
            resolved_target, universal_path,
            "{kind:?}: the restored link must resolve to the restored tree"
        );

        if matches!(kind, LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork) {
            let registry =
                std::fs::read_to_string(home.join(".agents").join("skill-studio.json")).unwrap();
            let registry: serde_json::Value = serde_json::from_str(&registry).unwrap();
            let map_key = if kind == LifecycleOwnerKind::Copy {
                "copies"
            } else {
                "forks"
            };
            let has_row = registry
                .get(map_key)
                .and_then(serde_json::Value::as_object)
                .is_some_and(|map| {
                    map.keys().any(|k| k == &skill)
                        || map
                            .values()
                            .any(|v| v.get("name").and_then(|n| n.as_str()) == Some(skill.as_str()))
                });
            assert!(
                has_row,
                "{kind:?}: the {map_key} registry row must be back after restore"
            );
        } else {
            // Round 3, N5: undo does not re-run the CLI, so the lock entry
            // it dropped (`.skill-lock.json` for `SkillsSh`, `agents.lock`
            // for `Dotagents` - see `FakeNpxSpawner`'s `remove` branch) stays
            // gone, and the restored tree reads as `Manual` until the next
            // real `npx ... add` - the documented state from
            // `issue-3.9a-followup-a.md` item 10.
            let owner_kind = resolve_owner_kind(&rt, &skill);
            assert_eq!(
                owner_kind,
                LifecycleOwnerKind::Manual,
                "{kind:?}: without its CLI-owned lock entry, the restored tree must read as Manual"
            );
        }
    }
}

/// Which step [`remove_crash_mid_rename_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder`]
/// (round 2, N2) injects a failure into - the tree step every kind has, or
/// the registry write / link removal that follow it for `Copy`/`Fork`.
/// `Dotagents`/`SkillsSh` skip `Registry`: their registry bookkeeping is the
/// CLI's own (no `drop_registry_entry` write of this op's own to fail). They
/// do get `Link` (round 3, N4): the real CLI already deletes the Claude Code
/// link itself before this op ever reaches its own link loop (see
/// [`FakeNpxSpawner`]'s `remove` branch and B1's `NotFound` tolerance in
/// `remove_and_link`), so their `Link` crash targets a second, non-Claude
/// harness link the fake CLI never touches instead.
#[derive(Debug, Clone, Copy)]
enum CrashPoint {
    Tree,
    Registry,
    Link,
}

/// `remove_crash_mid_rename_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder`
/// (the red check, extended in coordinator round 2 to all four owner
/// kinds, and to the registry-write and link-removal steps, N2): failing
/// any one write `remove` makes must never leave the deployment
/// half-moved - either it is still at the universal root (the before
/// state) or, for `Copy`/`Fork`, it already landed, complete, in
/// quarantine (the after state; `Dotagents`/`SkillsSh` have no "after"
/// state to land in for a `Tree` crash, since the CLI's own crash is
/// injected before it touches disk at all). A `Registry`/`Link` crash
/// happens after the tree step already landed, so the tree-then-links
/// order (module doc on `remove_and_link`) puts the link still standing
/// for a `Link` crash - proof the link step never runs before the tree
/// step, not after it silently skips.
#[test]
fn remove_crash_mid_rename_leaves_disk_in_the_before_or_after_state_or_names_the_stray_folder() {
    for kind in MUTABLE_OWNER_KINDS {
        let points: &[CrashPoint] = match kind {
            LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork => {
                &[CrashPoint::Tree, CrashPoint::Registry, CrashPoint::Link]
            }
            LifecycleOwnerKind::Dotagents | LifecycleOwnerKind::SkillsSh => {
                &[CrashPoint::Tree, CrashPoint::Link]
            }
            other => panic!("unsupported owner kind: {other:?}"),
        };
        for point in points {
            let home = unique_temp_dir(&format!("remove_crash_window_{kind:?}_{point:?}"));
            std::fs::create_dir_all(&home).unwrap();
            // One runtime for both the setup and the crashed remove -
            // `park_and_unpark.rs`'s own crash tests follow the same shape.
            // Two separate runtimes would each carry their own `FakeIds`
            // counter starting from zero, and the second call's event id
            // would collide with the first (both writing to the same
            // `events.sqlite3`).
            let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
            let spawner = Arc::new(FakeNpxSpawner::new(home.clone()));
            let rt = runtime_with(&home, failing_fs.clone(), Some(spawner.clone()));
            let skill = format!("crash-{kind:?}-{point:?}").to_lowercase();
            let deployment_id = setup_owner_kind_with_claude_link(&rt, &home, kind, &skill);
            let claude_link = home.join(CLAUDE_ROOT_RELATIVE).join(&skill);
            // Round 3, N4: `Dotagents`/`SkillsSh` also get a second link
            // under a harness root the fake CLI never touches, so their
            // `Link` crash point actually exercises `remove_and_link`'s own
            // link loop rather than crashing on a link the CLI already took
            // down itself.
            let codex_link = matches!(
                kind,
                LifecycleOwnerKind::Dotagents | LifecycleOwnerKind::SkillsSh
            )
            .then(|| {
                let codex_dir = home.join(CODEX_ROOT_RELATIVE);
                std::fs::create_dir_all(&codex_dir).unwrap();
                let target = home.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);
                let link = codex_dir.join(&skill);
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, &link).unwrap();
                link
            });

            match point {
                CrashPoint::Tree => match kind {
                    LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork => {
                        failing_fs.fail_next_rename();
                    }
                    LifecycleOwnerKind::Dotagents | LifecycleOwnerKind::SkillsSh => {
                        spawner.fail_next_call();
                    }
                    other => panic!("unsupported owner kind: {other:?}"),
                },
                CrashPoint::Registry => failing_fs.fail_next_write_atomic(),
                CrashPoint::Link => failing_fs.fail_next_remove_file(),
            }
            let err = ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap_err();
            assert_eq!(
                err.code,
                skill_studio_core::ErrorCode::Io,
                "{kind:?} {point:?}"
            );

            let original = home.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);
            let quarantine_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(QUARANTINE_DIR_NAME);
            let landed = std::fs::read_dir(&quarantine_dir)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false);
            let tree_state_ok = match (kind, point) {
                (_, CrashPoint::Tree) => original.join("SKILL.md").exists() != landed,
                // `Copy`/`Fork`'s tree step already succeeded here: the
                // deployment is already in quarantine, and the
                // universal-root copy is gone either way.
                (
                    LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork,
                    CrashPoint::Registry | CrashPoint::Link,
                ) => !original.join("SKILL.md").exists() && landed,
                // `Dotagents`/`SkillsSh` never quarantine - their tree step
                // is the CLI's own delete, which a `Link` crash (round 3,
                // N4) happens strictly after, so the universal-root copy is
                // simply gone, with nothing landed anywhere.
                (
                    LifecycleOwnerKind::Dotagents | LifecycleOwnerKind::SkillsSh,
                    CrashPoint::Link,
                ) => !original.join("SKILL.md").exists() && !landed,
                (kind, point) => panic!("unexpected combination: {kind:?} {point:?}"),
            };
            assert!(
                tree_state_ok,
                "{kind:?} {point:?}: the deployment must be exactly one of: still at the universal root, or fully in quarantine - never neither or both"
            );
            if landed {
                let entry = std::fs::read_dir(&quarantine_dir)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap();
                assert!(entry.path().join("SKILL.md").exists(), "{kind:?} {point:?}");
            }
            match (kind, point) {
                (
                    LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork,
                    CrashPoint::Tree | CrashPoint::Registry,
                ) => {
                    // The link step never ran (a `Tree` crash never reaches
                    // it; a `Registry` crash fails before it) - the link
                    // must still stand exactly as setup left it.
                    assert!(
                        std::fs::symlink_metadata(&claude_link).is_ok(),
                        "{kind:?} {point:?}: the link must still stand - the link step never ran"
                    );
                }
                (LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork, CrashPoint::Link) => {
                    // The tree-then-links order (module doc): the tree
                    // already moved, and the link removal itself is what
                    // crashed, so the link is left stray rather than the
                    // tree never moving.
                    assert!(
                        std::fs::symlink_metadata(&claude_link).is_ok(),
                        "{kind:?} {point:?}: a crashed link removal must leave the link, not the tree, stray"
                    );
                }
                (
                    LifecycleOwnerKind::Dotagents | LifecycleOwnerKind::SkillsSh,
                    CrashPoint::Tree,
                ) => {
                    // `spawner.fail_next_call()` crashes the CLI before it
                    // touches disk (see `CrashPoint::Tree`'s own arm above),
                    // so the Claude Code link the CLI would have deleted
                    // itself is still standing.
                    assert!(
                        std::fs::symlink_metadata(&claude_link).is_ok(),
                        "{kind:?} {point:?}: the CLI never ran, so the Claude Code link must still stand"
                    );
                }
                (
                    LifecycleOwnerKind::Dotagents | LifecycleOwnerKind::SkillsSh,
                    CrashPoint::Link,
                ) => {
                    // The real CLI already deleted the Claude Code link as
                    // part of its own `remove` (the Tree step, which ran
                    // before this crash), so only the non-Claude link this
                    // op's own link loop still owns is left stray.
                    assert!(
                        std::fs::symlink_metadata(&claude_link).is_err(),
                        "{kind:?} {point:?}: the real CLI must have already deleted the Claude Code link"
                    );
                }
                (kind, point) => panic!("unexpected combination: {kind:?} {point:?}"),
            }
            if let Some(codex_link) = &codex_link {
                match point {
                    CrashPoint::Tree => assert!(
                        std::fs::symlink_metadata(codex_link).is_ok(),
                        "{kind:?} {point:?}: the non-Claude link must still stand - the link step never ran"
                    ),
                    CrashPoint::Link => assert!(
                        std::fs::symlink_metadata(codex_link).is_ok(),
                        "{kind:?} {point:?}: a crashed link removal must leave the non-Claude link, not the tree, stray"
                    ),
                    CrashPoint::Registry => {}
                }
            }

            // Round 1, Q3: the row itself must record the crash, not just
            // leave the tree in a valid state - a `failed` `remove` row
            // with its `backup_dir` set, so a later prune (Q2) and a
            // manual retry both have something to find.
            let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
            let remove_row = events
                .iter()
                .find(|e| e.kind == "remove")
                .expect("the crashed remove must still have written its own row");
            assert_eq!(
                remove_row.status, "failed",
                "{kind:?} {point:?}: a crashed remove must mark the row failed"
            );
            assert!(
                remove_row.backup_dir.is_some(),
                "{kind:?} {point:?}: a failed remove must still have an archival backup_dir"
            );
        }
    }
}

/// `skills_sh_remove_succeeds_when_the_cli_already_deleted_the_harness_links_or_names_the_failed_row`
/// (round 2, B1): the real `npx skills remove`/`npx -y @sentry/dotagents
/// remove` (no `--agent` given) already deletes every per-agent link
/// itself before this op ever reaches its own link loop - so a link that is
/// already gone by the time `remove_and_link` gets to it must not fail the
/// row. Without B1's fix, `remove_and_link`'s unconditional `fs.remove_file`
/// hit `NotFound` on the already-deleted Claude Code link and the whole call
/// returned `Io`, even though the deployment, links, and lock entry were
/// all correctly gone.
#[test]
fn skills_sh_remove_succeeds_when_the_cli_already_deleted_the_harness_links_or_names_the_failed_row(
) {
    for kind in [LifecycleOwnerKind::SkillsSh, LifecycleOwnerKind::Dotagents] {
        let home = unique_temp_dir(&format!("remove_cli_already_deleted_link_{kind:?}"));
        std::fs::create_dir_all(&home).unwrap();
        let rt = runtime_for(&home);
        let skill = format!("preremoved-{kind:?}").to_lowercase();
        let deployment_id = setup_owner_kind(&rt, &home, kind, &skill);
        let claude_dir = home.join(CLAUDE_ROOT_RELATIVE);
        std::fs::create_dir_all(&claude_dir).unwrap();
        let target = home.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);
        let link_path = claude_dir.join(&skill);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link_path).unwrap();

        // `FakeNpxSpawner`'s own `remove` branch matches the real CLI: it
        // deletes the tree and the Claude Code link together, so by the
        // time `remove_and_link`'s link loop runs, `link_path` is already
        // gone.
        ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();

        assert!(
            std::fs::symlink_metadata(&link_path).is_err(),
            "{kind:?}: the link must be gone"
        );
        let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
        let remove_row = events
            .iter()
            .find(|e| e.kind == "remove")
            .expect("the remove row must exist");
        assert_eq!(
            remove_row.status, "done",
            "{kind:?}: a link the CLI already deleted must not fail the row"
        );
    }
}

/// `skills_sh_remove_fails_when_a_link_parent_is_unreadable_or_names_the_row_marked_done`
/// (round 3, B2): a `symlink_metadata` error that is not `NotFound` - here a
/// `PermissionDenied` on a link whose parent the fake CLI never touches -
/// must fail the row, not be swallowed the same way a genuinely absent link
/// is. Before B2, `remove_and_link`'s link loop skipped the link on *any*
/// `symlink_metadata` error, so this case reported `Done` with the link
/// still on disk.
#[test]
fn skills_sh_remove_fails_when_a_link_parent_is_unreadable_or_names_the_row_marked_done() {
    for kind in [LifecycleOwnerKind::SkillsSh, LifecycleOwnerKind::Dotagents] {
        let home = unique_temp_dir(&format!("remove_link_parent_unreadable_{kind:?}"));
        std::fs::create_dir_all(&home).unwrap();
        let failing_fs = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
        let rt = runtime_with(
            &home,
            failing_fs.clone(),
            Some(Arc::new(FakeNpxSpawner::new(home.clone()))),
        );
        let skill = format!("unreadable-link-{kind:?}").to_lowercase();
        let deployment_id = setup_owner_kind(&rt, &home, kind, &skill);
        // A non-Claude harness root, so `FakeNpxSpawner`'s own `remove`
        // branch (which only ever touches `CLAUDE_ROOT_RELATIVE`) never
        // deletes this link itself - `remove_and_link`'s own loop must be
        // the one to reach it.
        let codex_dir = home.join(CODEX_ROOT_RELATIVE);
        std::fs::create_dir_all(&codex_dir).unwrap();
        let target = home.join(UNIVERSAL_ROOT_RELATIVE).join(&skill);
        let codex_link = codex_dir.join(&skill);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &codex_link).unwrap();

        failing_fs.fail_symlink_metadata_for(codex_link.clone());
        let err = ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap_err();
        assert_eq!(
            err.code,
            skill_studio_core::ErrorCode::Io,
            "{kind:?}: a PermissionDenied on the link check must surface as Io"
        );
        assert_eq!(
            err.path.as_deref(),
            Some(codex_link.as_path()),
            "{kind:?}: the error must name the unreadable link"
        );

        assert!(
            std::fs::symlink_metadata(&codex_link).is_ok(),
            "{kind:?}: the link this could not even check must be left in place, not reported removed"
        );
        let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
        let remove_row = events
            .iter()
            .find(|e| e.kind == "remove")
            .expect("the remove row must exist");
        assert_eq!(
            remove_row.status, "failed",
            "{kind:?}: an unreadable link must mark the row failed, not done"
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

    // Round 2, N4: the prune itself is journaled, not a silent sweep - see
    // `prune_quarantine`'s own doc.
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    assert!(
        events.iter().any(|e| e.kind == "quarantine_prune"),
        "a prune that drops at least one entry must record its own quarantine_prune row: {events:?}"
    );
}

/// `quarantine_prune_drops_entries_older_than_the_age_cap_or_names_the_kept_entry`
/// (round 2, N4): an entry past `QUARANTINE_AGE_CAP` is pruned even while
/// the directory is well under `QUARANTINE_RETENTION_CAP`, so an idle
/// install does not carry a removed tree forever. `FakeIds`' own fake event
/// ids are not real ulids with a meaningful embedded timestamp (see
/// `crate::testing::FakeIds`), so the aged entry's name is hand-built from a
/// real `ulid::Ulid` timestamped at the Unix epoch instead of one this test
/// drives through `remove` itself.
#[test]
fn quarantine_prune_drops_entries_older_than_the_age_cap_or_names_the_kept_entry() {
    let home = unique_temp_dir("remove_quarantine_age_cap");
    std::fs::create_dir_all(&home).unwrap();
    let quarantine_dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(QUARANTINE_DIR_NAME);
    std::fs::create_dir_all(&quarantine_dir).unwrap();

    let old_ulid = ulid::Ulid::from_datetime(std::time::UNIX_EPOCH);
    let old_name = format!("old-{old_ulid}");
    let old_dir = quarantine_dir.join(&old_name);
    std::fs::create_dir_all(&old_dir).unwrap();
    std::fs::write(old_dir.join("SKILL.md"), b"---\nname: old\n---\n").unwrap();

    // Well under `QUARANTINE_RETENTION_CAP`, so only the age cap - not the
    // count cap - can be what prunes `old_name`.
    let clock = Arc::new(FakeClock::at(0));
    clock.advance(std::time::Duration::from_secs(60 * 24 * 60 * 60));
    let rt = runtime_with_clock(
        &home,
        Arc::new(RealFs::new()),
        Some(Arc::new(FakeNpxSpawner::new(home.clone()))),
        clock,
    );
    let deployment_id = install_and_resolve(&rt, "fresh");

    ops::remove(&rt, &ctx(), &RemoveRequest { deployment_id }).unwrap();

    let remaining: Vec<String> = std::fs::read_dir(&quarantine_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !remaining.contains(&old_name),
        "the entry older than the age cap must be pruned, not kept: {remaining:?}"
    );
}

/// One recorded (here, hand-built) `npx skills remove` call's shape: the
/// argv, the tree it deletes, the harness link it also deletes, and the
/// lock entry it drops. Mirrors `install.rs`'s own `CliTrace`, plus
/// `claude_code_link`/`lock_before`/`lock_entry_removed`, which that
/// fixture has no counterpart for since `add` never touches the lock file
/// itself (that's `npx skills add`'s job, upstream of what `FakeNpxSpawner`
/// stands in for) and never has a pre-existing link to remove.
#[derive(serde::Deserialize)]
struct RemoveCliTrace {
    program: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    files: Vec<CliTraceFile>,
    /// Round 2, N1: whether the fixture's before state includes a Claude
    /// Code per-skill link the CLI also removes - `find_all_links`
    /// (`ops.rs:4670`) was exercised by no test before this.
    claude_code_link: bool,
    /// Round 2, N1: the full `.agents/.skill-lock.json` document before the
    /// run, so the parity test can diff the whole file, not just the one
    /// entry's presence.
    lock_before: serde_json::Value,
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
/// then asserts `remove_via_cli`'s own argv/cwd match the trace exactly,
/// that the tree, the Claude Code link, and the lock entry are gone
/// afterward, and that the lock file's remaining bytes match `lock_before`
/// with only that entry removed - `docs/action-map/definition-of-done.md`
/// check 4's "lockfile entry matches ... byte for byte" (round 2, N1).
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
    let agents_dir = home.join(".agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    let lock_path = agents_dir.join(".skill-lock.json");
    std::fs::write(&lock_path, serde_json::to_vec(&trace.lock_before).unwrap()).unwrap();

    let claude_link = home.join(CLAUDE_ROOT_RELATIVE).join(skill);
    if trace.claude_code_link {
        let claude_dir = home.join(CLAUDE_ROOT_RELATIVE);
        std::fs::create_dir_all(&claude_dir).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&skill_dir, &claude_link).unwrap();
    }

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
    if trace.claude_code_link {
        assert!(
            std::fs::symlink_metadata(&claude_link).is_err(),
            "the CLI's remove call must take the Claude Code link down too"
        );
    }

    let mut expected_lock = trace.lock_before.clone();
    expected_lock
        .get_mut("skills")
        .and_then(|s| s.as_object_mut())
        .and_then(|m| m.shift_remove(skill));
    let expected_bytes = serde_json::to_vec(&expected_lock).unwrap();
    let lock_bytes = std::fs::read(&lock_path).unwrap();
    assert_eq!(
        lock_bytes, expected_bytes,
        "the lock file's remaining bytes must match lock_before with only {skill:?} removed"
    );
}
