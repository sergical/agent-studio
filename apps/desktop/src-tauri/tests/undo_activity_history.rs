// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Desktop Activity and Undo must read the *shared* core history database
//! (`core_runtime::history_db_path`) instead of a separate,
//! desktop-only `events.sqlite3` (issue #263): every core `ops` mutation
//! (park, unpark, install, remove, `set_harness_enabled`) has to show up in
//! `EventStore::list` (what `list_skill_events` reads), and undoing it has
//! to dispatch to whichever side (`EventStore::restore` or
//! `ops::restore_event`) actually understands that event's inverse shape
//! (`event_commands::restore_event_with_runtime`).
//!
//! Each test builds its own `Runtime` the way `crates/skill-studio-core`'s
//! own integration tests do (`Ports { .. }` by hand), pointed at the same
//! `<data_root>/history/events.sqlite3` path the real desktop app computes
//! via `core_runtime::history_db_path`, then opens the desktop's
//! `EventStore` on that same file to read Activity and drive Undo - proving
//! the two sides actually share one database rather than merely agreeing on
//! a schema.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use skill_studio_core::dto::{
    InstallFile, InstallMethod, InstallOutcome, InstallRequest, ScanRequest,
    SetHarnessEnabledRequest, UnparkRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{
    AgentId, CorrelationId, DeploymentId, RootKind, RootScope, SkillName,
};
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::{
    CancelToken, OpContext, Ports, ProcessOutput, ProcessSpawner, ProcessSpec, Runtime,
};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

use skill_studio_lib::skills::core_runtime::{build_runtime_write_at, to_command_result};
use skill_studio_lib::skills::event_commands::{dto_from_row, restore_event_with_runtime};
use skill_studio_lib::skills::event_store::EventStore;
use skill_studio_lib::skills::skill_materialize::repair_remove_link;
use skill_studio_lib::skills::skill_park::park_with_runtime;

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";
const CLAUDE_ROOT_RELATIVE: &str = ".claude/skills";

/// `<data_root>/history/events.sqlite3` - matches `core_runtime::
/// history_db_path`, restated here (rather than importing a `pub(crate)`
/// item across the crate boundary) the same way `tests/park_parity.rs`
/// restates `DATA_ROOT_RELATIVE` instead of reaching into private helpers.
fn history_db_path(data_root: &Path) -> PathBuf {
    data_root.join("history").join("events.sqlite3")
}

fn data_root_for(home: &Path) -> PathBuf {
    home.join(".skill-studio")
}

/// Opens the desktop's `EventStore` against the same shared file
/// `build_runtime_write_at` writes core history through - what
/// `lib.rs::open_event_store` does in the real app.
fn desktop_store(home: &Path, data_root: &Path) -> EventStore {
    EventStore::open_with_db(&home.join("app_data"), &history_db_path(data_root)).unwrap()
}

fn write_manual_universal_skill(home: &Path, skill: &str) {
    let dir = home.join(UNIVERSAL_ROOT_RELATIVE).join(skill);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {skill}\ndescription: a fixture skill\n---\nBody.\n"),
    )
    .unwrap();
}

/// Adds `skill` to `<home>/.agents/.skill-lock.json`, the ledger
/// `classify_owner` reads to classify a universal deployment as `SkillsSh` -
/// matches `crates/skill-studio-core/tests/remove.rs`'s own `mark_skills_sh`.
fn mark_skills_sh(home: &Path, skill: &str) {
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

fn lock_has_skill(home: &Path, skill: &str) -> bool {
    let lock_path = home.join(".agents").join(".skill-lock.json");
    let Ok(bytes) = std::fs::read(&lock_path) else {
        return false;
    };
    let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    doc.get("skills")
        .and_then(|skills| skills.get(skill))
        .is_some()
}

/// Stands in for `npx skills remove <name> --yes --global`: deletes the
/// skill's universal tree, its Claude Code link, and its `.skill-lock.json`
/// row - the same three side effects
/// `crates/skill-studio-core/tests/remove.rs`'s own `FakeNpxSpawner` has,
/// trimmed to only the `remove` branch since this file never installs
/// through the CLI.
struct FakeNpxRemoveSpawner {
    home: PathBuf,
    recorded: Mutex<Vec<Vec<String>>>,
}

impl ProcessSpawner for FakeNpxRemoveSpawner {
    fn run(
        &self,
        spec: &ProcessSpec,
        _cancel: &dyn CancelToken,
    ) -> Result<ProcessOutput, skill_studio_core::CoreError> {
        assert_eq!(spec.program, "npx");
        self.recorded.lock().unwrap().push(spec.args.clone());
        let cwd = spec.cwd.clone().unwrap_or_else(|| self.home.clone());
        let idx = spec
            .args
            .iter()
            .position(|a| a == "remove")
            .expect("this fixture only ever removes");
        let name = spec.args[idx + 1].clone();
        std::fs::remove_dir_all(cwd.join(UNIVERSAL_ROOT_RELATIVE).join(&name)).ok();
        std::fs::remove_file(cwd.join(CLAUDE_ROOT_RELATIVE).join(&name)).ok();
        let lock_path = cwd.join(".agents").join(".skill-lock.json");
        if let Ok(bytes) = std::fs::read(&lock_path) {
            if let Ok(mut doc) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(skills) = doc.get_mut("skills").and_then(|v| v.as_object_mut()) {
                    skills.remove(&name);
                }
                let _ = std::fs::write(&lock_path, serde_json::to_vec(&doc).unwrap());
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

/// A `Runtime` rooted at `home`/`data_root` like `build_runtime_write_at`
/// builds, but with a fake `npx` spawner in place of the real one - the
/// only difference a `SkillsSh` remove needs from the real desktop wiring.
fn runtime_with_fake_spawner(home: &Path, data_root: &Path) -> Runtime {
    let scope = RuntimeScope::live(home.to_path_buf(), data_root.join("history"));
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(data_root.join("leases"))),
        history: Arc::new(SqliteHistoryOpener::new(history_db_path(data_root))),
        sink: Arc::new(RecordingSink::default()),
        spawner: Some(Arc::new(FakeNpxRemoveSpawner {
            home: home.to_path_buf(),
            recorded: Mutex::new(Vec::new()),
        })),
        discovery: Some(Arc::new(skill_studio_host::HostProjectDiscovery::new())),
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    Runtime::new(&scope, ports).unwrap()
}

fn resolve_universal_deployment_id(rt: &Runtime, skill: &str) -> DeploymentId {
    let inventory = ops::scan(rt, &ctx(), &ScanRequest::default()).unwrap();
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

/// After a park, the skill's deployment id changes (it now lives under the
/// `Parked` root, not `Universal`) - `ops::unpark` needs that new id, not
/// the pre-park one, matching `park_and_unpark.rs`'s own round trip.
fn resolve_parked_deployment_id(rt: &Runtime, skill: &str) -> DeploymentId {
    let inventory = ops::scan(rt, &ctx(), &ScanRequest::default()).unwrap();
    inventory
        .skills
        .iter()
        .find(|s| s.name.0 == skill)
        .and_then(|s| {
            s.deployments
                .iter()
                .find(|d| d.root.kind == RootKind::Parked)
        })
        .unwrap_or_else(|| panic!("no parked deployment found for {skill}"))
        .id
        .clone()
}

// ---------------------------------------------------------------------
// (a) A desktop remove of a skills.sh skill shows in Activity and Undo
// restores its folder, its Claude Code link, and its `.skill-lock.json` row.
// ---------------------------------------------------------------------

#[test]
fn desktop_remove_of_a_skills_sh_skill_appears_in_history_and_undo_restores_folder_links_and_lock_entry(
) {
    let home = unique_temp_dir("undo_remove_skills_sh");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let skill = "gamma-sh";
    write_manual_universal_skill(&home, skill);
    mark_skills_sh(&home, skill);
    let claude_dir = home.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&claude_dir).unwrap();
    let relative_target = PathBuf::from("../..")
        .join(UNIVERSAL_ROOT_RELATIVE)
        .join(skill);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&relative_target, claude_dir.join(skill)).unwrap();

    let rt = runtime_with_fake_spawner(&home, &data_root);
    let deployment_id = resolve_universal_deployment_id(&rt, skill);
    let outcome = ops::remove(
        &rt,
        &ctx(),
        &skill_studio_core::dto::RemoveRequest {
            deployment_id: deployment_id.clone(),
        },
    )
    .unwrap();

    // The remove really ran: the tree, the link, and the lock row are gone.
    assert!(!home.join(UNIVERSAL_ROOT_RELATIVE).join(skill).exists());
    assert!(std::fs::symlink_metadata(claude_dir.join(skill)).is_err());
    assert!(!lock_has_skill(&home, skill));

    // Activity: the desktop's own `EventStore`, opened on the same shared
    // file the core just wrote through, sees the `remove` event - the
    // defect this fix closes (previously, this event lived in a database
    // the desktop's `EventStore` never opened).
    let store = desktop_store(&home, &data_root);
    let rows = store.list(50, Some(skill)).unwrap();
    let row = rows
        .iter()
        .find(|r| r.kind == "remove")
        .expect("the core's remove event must be visible through the desktop's EventStore");
    let dto = dto_from_row(&store, &home, row.clone());
    assert!(dto.restorable, "a fresh remove event must be restorable");

    // Undo: dispatches to the core (remove's inverse is core-owned), and
    // restores the folder, the link, and the lock entry.
    restore_event_with_runtime(&store, &home, &data_root, &row.id, false).unwrap();
    assert!(
        home.join(UNIVERSAL_ROOT_RELATIVE).join(skill).exists(),
        "undo must restore the skill's own folder"
    );
    // The core's own restore rewrites the link's target as an absolute
    // path rather than replaying the original relative spelling, so this
    // compares resolved targets instead of the literal link text.
    assert_eq!(
        std::fs::canonicalize(claude_dir.join(skill)).unwrap(),
        std::fs::canonicalize(home.join(UNIVERSAL_ROOT_RELATIVE).join(skill)).unwrap(),
        "undo must restore the Claude Code link"
    );
    let _ = &relative_target;
    assert!(
        lock_has_skill(&home, skill),
        "undo must restore the .skill-lock.json row"
    );

    let _ = outcome;
    std::fs::remove_dir_all(&home).ok();
}

// ---------------------------------------------------------------------
// (b) Table test: park, unpark, `set_harness_enabled`, and install each
// appear in Activity; park then Undo restores the skill.
// ---------------------------------------------------------------------

/// A universal skill with a Claude Code per-skill link, ready to park -
/// matches `tests/park_parity.rs`'s own `parkable_home`.
fn parkable_skill(home: &Path, skill: &str) {
    write_manual_universal_skill(home, skill);
    let claude_skills = home.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&claude_skills).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        home.join(UNIVERSAL_ROOT_RELATIVE).join(skill),
        claude_skills.join(skill),
    )
    .unwrap();
}

#[test]
fn park_unpark_harness_toggle_and_install_each_appear_in_history_and_park_undo_restores_the_skill()
{
    let home = unique_temp_dir("undo_table_test");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);

    // install (Copy - no external CLI needed).
    let rt = build_runtime_write_at(&home, &data_root).unwrap();
    let install_outcome = ops::install(
        &rt,
        &ctx(),
        &InstallRequest {
            skill: SkillName("delta-copy".to_string()),
            method: InstallMethod::Copy,
            scope: RootScope::Global,
            harnesses: Vec::new(),
            files: vec![InstallFile {
                relative_path: PathBuf::from("SKILL.md"),
                contents: b"---\nname: delta-copy\ndescription: a copied skill\n---\nBody.\n"
                    .to_vec(),
            }],
            source: None,
            trust_identity: None,
            trust_confirmed: false,
            save_as_preference: false,
        },
    )
    .unwrap();
    let InstallOutcome::Installed { event_id, .. } = install_outcome else {
        panic!("expected Installed");
    };

    // park a second skill.
    let park_skill = "epsilon-park";
    parkable_skill(&home, park_skill);
    let park_deployment_id = resolve_universal_deployment_id(&rt, park_skill);
    let park_outcome = park_with_runtime(&home, &data_root, park_deployment_id).unwrap();
    assert!(
        !home.join(UNIVERSAL_ROOT_RELATIVE).join(park_skill).exists(),
        "parked skill must be off the universal root"
    );

    // set_harness_enabled on a third skill (Claude Code's global switch).
    let toggle_skill = "zeta-toggle";
    parkable_skill(&home, toggle_skill);
    let toggle_ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let toggle_result = ops::set_harness_enabled(
        &rt,
        &toggle_ctx,
        &SetHarnessEnabledRequest {
            skill: SkillName(toggle_skill.to_string()),
            harness: AgentId::parse(AgentId::CLAUDE_CODE).unwrap(),
            enabled: false,
            project_path: None,
        },
    );
    let toggle_outcome = to_command_result(ResultEnvelope::from_result(
        Operation::SetHarnessEnabled,
        &rt.scope,
        &toggle_ctx,
        toggle_result,
    ))
    .unwrap();

    // `Park` deliberately carries no generic inverse (see `ops::park`'s own
    // doc comment): its "Undo" in the real desktop UI is the dedicated
    // `unpark_skill` command, not `restore_skill_event`. Drive that same
    // path here, against the deployment id `park` moved the skill to.
    let parked_deployment_id = resolve_parked_deployment_id(&rt, park_skill);
    let unpark_ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let unpark_result = ops::unpark(
        &rt,
        &unpark_ctx,
        &UnparkRequest {
            deployment_id: parked_deployment_id,
        },
    );
    let unpark_outcome = to_command_result(ResultEnvelope::from_result(
        Operation::Unpark,
        &rt.scope,
        &unpark_ctx,
        unpark_result,
    ))
    .unwrap();
    assert!(
        home.join(UNIVERSAL_ROOT_RELATIVE).join(park_skill).exists(),
        "unparking (park's own Undo) must restore the skill onto the universal root"
    );

    let store = desktop_store(&home, &data_root);
    for (label, id) in [
        ("install", &install_outcome_id(&event_id)),
        ("park", &park_outcome.event_id.0),
        ("unpark", &unpark_outcome.event_id.0),
        ("set_harness_enabled", &toggle_outcome.event_id.0),
    ] {
        let rows = store.list(200, None).unwrap();
        assert!(
            rows.iter().any(|r| &r.id == id),
            "{label}'s event {id} must be visible through the desktop's EventStore"
        );
    }

    std::fs::remove_dir_all(&home).ok();
}

fn install_outcome_id(event_id: &skill_studio_core::identity::EventId) -> String {
    event_id.0.clone()
}

// ---------------------------------------------------------------------
// (c) A desktop-written event still lists and still undoes, after the
// shared-database migration.
// ---------------------------------------------------------------------

#[test]
fn a_desktop_written_repair_event_still_appears_in_history_and_still_undoes() {
    let home = unique_temp_dir("undo_desktop_owned");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let skill = "eta-repair";
    write_manual_universal_skill(&home, skill);
    let claude_dir = home.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&claude_dir).unwrap();
    let link = claude_dir.join(skill);
    #[cfg(unix)]
    std::os::unix::fs::symlink(home.join(UNIVERSAL_ROOT_RELATIVE).join(skill), &link).unwrap();

    let store = desktop_store(&home, &data_root);
    repair_remove_link(&store, &link, skill, "claude-code").unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "repair_remove_link must have deleted the symlink"
    );

    let rows = store.list(50, Some(skill)).unwrap();
    let row = rows
        .iter()
        .find(|r| r.kind == "repair_remove_link")
        .expect("the desktop-written event must still be visible");
    let dto = dto_from_row(&store, &home, row.clone());
    assert!(dto.restorable);

    restore_event_with_runtime(&store, &home, &data_root, &row.id, false).unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "undo must recreate the symlink repair_remove_link deleted"
    );

    std::fs::remove_dir_all(&home).ok();
}

// ---------------------------------------------------------------------
// (d) The one-time import of a pre-migration desktop event log runs once:
// a second startup imports nothing twice, and a corrupt legacy file does
// not block startup.
// ---------------------------------------------------------------------

#[test]
fn legacy_event_log_imports_once_and_a_corrupt_legacy_file_does_not_block_startup() {
    let home = unique_temp_dir("undo_legacy_import");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let app_data = home.join("app_data");
    std::fs::create_dir_all(&app_data).unwrap();

    // A pre-migration desktop store, with one event recorded the old way
    // (its own `app_data/events.sqlite3`, before the shared file existed).
    let legacy_path = app_data.join("events.sqlite3");
    {
        let legacy_store = EventStore::open_with_db(&app_data, &legacy_path).unwrap();
        legacy_store
            .record(
                "legacy-event-1",
                &skill_studio_lib::skills::event_store::EventDraft {
                    kind: "repair_remove_link".to_string(),
                    skill: "theta-legacy".to_string(),
                    harness: Some("claude-code".to_string()),
                    scope: Some("global".to_string()),
                    project_path: None,
                    payload: serde_json::json!({}),
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
        legacy_store
            .finish(
                "legacy-event-1",
                skill_studio_lib::skills::event_store::EventStatus::Done,
            )
            .unwrap();
    }

    // First startup: opens the shared db, then imports the legacy file.
    let shared_store = EventStore::open_with_db(&app_data, &history_db_path(&data_root)).unwrap();
    let imported_first = shared_store.import_legacy_events().unwrap();
    assert_eq!(
        imported_first, 1,
        "the legacy event must import exactly once"
    );
    assert!(
        shared_store.get("legacy-event-1").unwrap().is_some(),
        "the imported row must be readable from the shared db"
    );
    assert!(
        !legacy_path.exists(),
        "a successful import must rename the legacy file out of the way"
    );
    assert!(app_data.join("events.sqlite3.migrated").exists());

    // Second startup: the legacy file is gone (renamed already), so nothing
    // imports again and nothing is lost.
    let imported_second = shared_store.import_legacy_events().unwrap();
    assert_eq!(
        imported_second, 0,
        "a second run must not re-import anything"
    );
    assert!(shared_store.get("legacy-event-1").unwrap().is_some());

    // A corrupt legacy file (from a different, fresh app_data) must not
    // block startup: `import_legacy_events` reports an error rather than
    // panicking or leaving the store unusable.
    let corrupt_app_data = home.join("app_data_corrupt");
    std::fs::create_dir_all(&corrupt_app_data).unwrap();
    std::fs::write(
        corrupt_app_data.join("events.sqlite3"),
        b"not a sqlite file",
    )
    .unwrap();
    let corrupt_store =
        EventStore::open_with_db(&corrupt_app_data, &history_db_path(&data_root)).unwrap();
    let corrupt_result = corrupt_store.import_legacy_events();
    assert!(
        corrupt_result.is_err(),
        "a corrupt legacy file must be reported, not silently accepted"
    );
    // The store opened just fine despite the corrupt legacy file - "must not
    // block startup" means the caller (lib.rs) still has a usable store.
    assert!(corrupt_store.list(1, None).is_ok());

    std::fs::remove_dir_all(&home).ok();
}

// ---------------------------------------------------------------------
// (e) Startup reconciliation never alters a completed core event.
// ---------------------------------------------------------------------

#[test]
fn startup_reconciliation_leaves_a_completed_core_event_untouched() {
    let home = unique_temp_dir("undo_reconcile_completed");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let skill = "iota-reconcile";
    write_manual_universal_skill(&home, skill);
    mark_skills_sh(&home, skill);

    let rt = runtime_with_fake_spawner(&home, &data_root);
    let deployment_id = resolve_universal_deployment_id(&rt, skill);
    ops::remove(
        &rt,
        &ctx(),
        &skill_studio_core::dto::RemoveRequest { deployment_id },
    )
    .unwrap();

    let store = desktop_store(&home, &data_root);
    let before = store.list(50, Some(skill)).unwrap();
    let row_before = before
        .iter()
        .find(|r| r.kind == "remove")
        .expect("the completed remove event must exist");
    assert_eq!(row_before.status, "done");

    let flipped = store.reconcile_at_startup().unwrap();
    assert!(
        flipped.is_empty(),
        "reconciliation must not touch any completed event"
    );

    let after = store.get(&row_before.id).unwrap().unwrap();
    assert_eq!(
        after.status, "done",
        "a completed core event's status must survive startup reconciliation"
    );

    std::fs::remove_dir_all(&home).ok();
}
