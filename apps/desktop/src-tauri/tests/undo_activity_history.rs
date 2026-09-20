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
    FixSkillRequest, InstallFile, InstallMethod, InstallOutcome, InstallRequest, ScanRequest,
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

use skill_studio_lib::open_event_store_at;
use skill_studio_lib::skills::core_runtime::{
    build_runtime_write_at, history_db_path, to_command_result,
};
use skill_studio_lib::skills::event_commands::{dto_from_row, restore_event_with_runtime};
use skill_studio_lib::skills::event_store::{
    allocate_id, fingerprint_path, EventDraft, EventStatus, EventStore, InverseOp,
};
use skill_studio_lib::skills::skill_materialize::repair_remove_link;
use skill_studio_lib::skills::skill_park::park_with_runtime;

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";
const CLAUDE_ROOT_RELATIVE: &str = ".claude/skills";

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
    let dto = dto_from_row(&store, &home, &data_root, row.clone());
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
    let dto = dto_from_row(&store, &home, &data_root, row.clone());
    assert!(dto.restorable);

    restore_event_with_runtime(&store, &home, &data_root, &row.id, false).unwrap();
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "undo must recreate the symlink repair_remove_link deleted"
    );

    std::fs::remove_dir_all(&home).ok();
}

// Second fix-round item 2: `move_aside_disable`/`move_aside_restore` carry
// `backup_dir: None` and an `InverseOp::MoveBack`, and no core code ever
// writes them - they belong back in `DESKTOP_OWNED_KINDS` (removing them
// was the earlier round's mistake), and this row shape is exactly what an
// old desktop-written row looks like today.
#[test]
fn a_legacy_move_aside_disable_row_undoes_through_the_desktop() {
    let home = unique_temp_dir("undo_move_aside_disable");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let skill = "iota-move-aside";
    let claude_dir = home.join(CLAUDE_ROOT_RELATIVE);
    std::fs::create_dir_all(&claude_dir).unwrap();
    let original = claude_dir.join(skill);
    std::fs::write(&original, "disabled aside").unwrap();
    let moved_aside = claude_dir.join(format!("{skill}.disabled"));
    std::fs::rename(&original, &moved_aside).unwrap();

    let store = desktop_store(&home, &data_root);
    let id = allocate_id();
    let inverse = InverseOp::MoveBack {
        from: moved_aside.clone(),
        to: original.clone(),
        pre_fingerprint: fingerprint_path(&original),
        post_fingerprint: None,
    };
    store
        .record(
            &id,
            &EventDraft {
                kind: "move_aside_disable".to_string(),
                skill: skill.to_string(),
                harness: Some("claude-code".to_string()),
                scope: None,
                project_path: None,
                payload: serde_json::json!({ "from": moved_aside, "to": original }),
                inverse: Some(serde_json::to_value(&inverse).unwrap()),
                backup_dir: None,
                restorable: true,
            },
        )
        .unwrap();
    store.finish(&id, EventStatus::Done).unwrap();

    let row = store.get(&id).unwrap().unwrap();
    let dto = dto_from_row(&store, &home, &data_root, row.clone());
    assert!(dto.restorable);

    restore_event_with_runtime(&store, &home, &data_root, &row.id, false).unwrap();
    assert!(
        original.is_file(),
        "undo must move the skill back from its disabled path"
    );
    assert!(std::fs::symlink_metadata(&moved_aside).is_err());

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

// ---------------------------------------------------------------------
// (f) `repair_skill_frontmatter` has two live writers (the desktop's
// `apply_skill_frontmatter_repair` and the core's `ops::fix_skill`) that
// write the same kind string over the same `restore_backup` inverse shape,
// under `backup_dir`s resolved against different roots - dispatch must
// probe which root actually has the row's manifest, not just its kind.
// ---------------------------------------------------------------------

/// Simulates exactly what `apply_skill_frontmatter_repair` writes (backup
/// under the desktop's own `app_data`, then a `repair_skill_frontmatter`
/// event pointing at it) without needing a `tauri::AppHandle` - the same
/// "record the write shape directly" approach the legacy-import test above
/// uses for a pre-migration row.
#[test]
fn a_desktop_written_frontmatter_repair_undoes_through_the_desktop_and_restores_skill_md() {
    let home = unique_temp_dir("undo_desktop_frontmatter_repair");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let skill = "kappa-repair";
    write_manual_universal_skill(&home, skill);
    let skill_md = home
        .join(UNIVERSAL_ROOT_RELATIVE)
        .join(skill)
        .join("SKILL.md");
    let original = std::fs::read(&skill_md).unwrap();

    let store = desktop_store(&home, &data_root);
    let event_id = allocate_id();
    let pre_fingerprint = fingerprint_path(&skill_md);
    store
        .backup_paths(&event_id, std::slice::from_ref(&skill_md))
        .unwrap();
    store
        .record(
            &event_id,
            &EventDraft {
                kind: "repair_skill_frontmatter".to_string(),
                skill: skill.to_string(),
                harness: None,
                scope: Some("global".to_string()),
                project_path: None,
                payload: serde_json::json!({}),
                inverse: Some(
                    serde_json::to_value(InverseOp::RestoreBackup {
                        path: skill_md.clone(),
                        pre_fingerprint,
                        post_fingerprint: None,
                    })
                    .unwrap(),
                ),
                backup_dir: Some(format!("backups/{event_id}")),
                restorable: true,
            },
        )
        .unwrap();
    store.finish(&event_id, EventStatus::Done).unwrap();

    // The "repair" itself: overwrite SKILL.md, as apply_skill_frontmatter_repair
    // would once its own record() call above lands.
    std::fs::write(
        &skill_md,
        b"---\nname: kappa-repair\ndescription: fixed\n---\nBody.\n",
    )
    .unwrap();

    restore_event_with_runtime(&store, &home, &data_root, &event_id, false).unwrap();
    assert_eq!(
        std::fs::read(&skill_md).unwrap(),
        original,
        "undo of a desktop-written repair must restore SKILL.md from the desktop's own backup"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// A malformed-frontmatter fixture, matching `tests/fix_parity.rs`'s own
/// `write_fixture`: an unquoted `: ` in `description` is the one repair
/// `propose_colon_scalar_repair` (and so `ops::fix_skill`) applies.
fn write_malformed_frontmatter_skill(home: &Path, skill: &str) {
    let dir = home.join(".claude/skills").join(skill);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {skill}\ndescription: Use this: when needed\n---\nBody.\n"),
    )
    .unwrap();
}

#[test]
fn a_core_written_frontmatter_repair_via_fix_skill_undoes_through_the_core() {
    let home = unique_temp_dir("undo_core_frontmatter_repair");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let skill = "lambda-bad";
    write_malformed_frontmatter_skill(&home, skill);
    let skill_md = home.join(".claude/skills").join(skill).join("SKILL.md");
    let original = std::fs::read(&skill_md).unwrap();

    let rt = build_runtime_write_at(&home, &data_root).unwrap();
    let outcome = ops::fix_skill(
        &rt,
        &ctx(),
        &FixSkillRequest {
            skill: SkillName(skill.to_string()),
        },
    )
    .unwrap();
    let skill_studio_core::dto::FixApplied::FrontmatterRepair { event_id, .. } = outcome
        .applied
        .into_iter()
        .next()
        .expect("fix_skill must have applied the colon-scalar repair");
    assert_ne!(
        std::fs::read(&skill_md).unwrap(),
        original,
        "fix_skill must actually have rewritten SKILL.md"
    );

    let store = desktop_store(&home, &data_root);
    let row = store
        .get(&event_id.0)
        .unwrap()
        .expect("the core's fix_skill event must be visible through the desktop's EventStore");
    assert_eq!(row.kind, "repair_skill_frontmatter");

    restore_event_with_runtime(&store, &home, &data_root, &row.id, false).unwrap();
    assert_eq!(
        std::fs::read(&skill_md).unwrap(),
        original,
        "undo of a core-written repair must restore SKILL.md from the core's own backup"
    );

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn backup_path_of_a_core_remove_row_points_at_an_existing_folder() {
    let home = unique_temp_dir("undo_backup_path_core_root");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let skill = "mu-backup-path";
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
    let rows = store.list(50, Some(skill)).unwrap();
    let row = rows
        .iter()
        .find(|r| r.kind == "remove")
        .expect("the core's remove event must exist")
        .clone();
    let dto = dto_from_row(&store, &home, &data_root, row);
    let backup_path = dto
        .backup_path
        .expect("a remove event must carry a backup_dir");
    assert!(
        Path::new(&backup_path).is_dir(),
        "backup_path must resolve to the core's own backup root ({backup_path}), not a \
         nonexistent path under the desktop's app_data"
    );

    std::fs::remove_dir_all(&home).ok();
}

// ---------------------------------------------------------------------
// (g) `EventStore::list` sorts by `ts DESC, rowid DESC`: an imported legacy
// row gets appended at the end of the table (highest `rowid`) regardless of
// its original `ts`, so `rowid` alone would sort it above events the core
// wrote just now.
// ---------------------------------------------------------------------

#[test]
fn list_orders_an_imported_old_row_below_a_newer_native_row() {
    let home = unique_temp_dir("undo_list_order");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let app_data = home.join("app_data");
    std::fs::create_dir_all(&app_data).unwrap();

    let legacy_path = app_data.join("events.sqlite3");
    {
        let legacy_store = EventStore::open_with_db(&app_data, &legacy_path).unwrap();
        legacy_store
            .record(
                "legacy-old-event",
                &EventDraft {
                    kind: "repair_remove_link".to_string(),
                    skill: "nu-old".to_string(),
                    harness: None,
                    scope: None,
                    project_path: None,
                    payload: serde_json::json!({}),
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
        legacy_store
            .finish("legacy-old-event", EventStatus::Done)
            .unwrap();
        // Force `ts` far in the past - the timestamp `import_legacy_events`
        // carries over unchanged, unlike the `rowid` the shared table
        // reassigns on import.
        legacy_store
            .conn
            .execute(
                "UPDATE events SET ts = '2000-01-01T00:00:00+00:00' WHERE id = ?1",
                rusqlite::params!["legacy-old-event"],
            )
            .unwrap();
    }

    let shared_store = EventStore::open_with_db(&app_data, &history_db_path(&data_root)).unwrap();
    // A native row, written after the import, so it gets both a later `ts`
    // and (via `import_legacy_events` appending) not necessarily a later
    // `rowid` than the about-to-be-imported legacy row.
    shared_store
        .record(
            "native-new-event",
            &EventDraft {
                kind: "repair_remove_link".to_string(),
                skill: "xi-new".to_string(),
                harness: None,
                scope: None,
                project_path: None,
                payload: serde_json::json!({}),
                inverse: None,
                backup_dir: None,
                restorable: false,
            },
        )
        .unwrap();
    shared_store
        .finish("native-new-event", EventStatus::Done)
        .unwrap();
    shared_store.import_legacy_events().unwrap();

    let rows = shared_store.list(50, None).unwrap();
    let native_idx = rows
        .iter()
        .position(|r| r.id == "native-new-event")
        .expect("native row must be listed");
    let legacy_idx = rows
        .iter()
        .position(|r| r.id == "legacy-old-event")
        .expect("imported legacy row must be listed");
    assert!(
        native_idx < legacy_idx,
        "the newer native row must sort above the older imported row, regardless of rowid order"
    );

    std::fs::remove_dir_all(&home).ok();
}

// ---------------------------------------------------------------------
// (h) `lib.rs::open_event_store_at` is the seam that fixed issue #263's
// original bug (opening a desktop-only `events.sqlite3` instead of the
// core's shared history database); this test goes through it, not a
// hand-built `EventStore::open_with_db`, so reverting that seam back to
// `EventStore::open(&app_data)` turns it red - proven by hand while writing
// this test (temporarily changed `open_event_store_at`'s body to
// `skills::event_store::EventStore::open(app_data_dir).ok()`, ignoring
// `data_root`, and re-ran this test: it failed because the core-written
// event was invisible; reverted before committing).
// ---------------------------------------------------------------------

#[test]
fn open_event_store_at_reads_the_shared_core_history_database() {
    let home = unique_temp_dir("undo_open_event_store_seam");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let app_data = home.join("app_data");
    std::fs::create_dir_all(&app_data).unwrap();
    let skill = "omicron-seam";
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

    let store = open_event_store_at(&app_data, &data_root)
        .expect("open_event_store_at must open successfully");
    let rows = store.list(50, Some(skill)).unwrap();
    assert!(
        rows.iter().any(|r| r.kind == "remove"),
        "open_event_store_at must open the same shared database core ops writes through, \
         not a separate desktop-only events.sqlite3"
    );

    std::fs::remove_dir_all(&home).ok();
}

// ---------------------------------------------------------------------
// (i) Legacy import is idempotent under an id collision and safe to retry
// after a failed rename.
// ---------------------------------------------------------------------

#[test]
fn legacy_import_keeps_the_shared_rows_copy_on_an_id_collision() {
    let home = unique_temp_dir("undo_legacy_import_collision");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let app_data = home.join("app_data");
    std::fs::create_dir_all(&app_data).unwrap();

    let legacy_path = app_data.join("events.sqlite3");
    {
        let legacy_store = EventStore::open_with_db(&app_data, &legacy_path).unwrap();
        legacy_store
            .record(
                "colliding-id",
                &EventDraft {
                    kind: "repair_remove_link".to_string(),
                    skill: "pi-legacy".to_string(),
                    harness: None,
                    scope: None,
                    project_path: None,
                    payload: serde_json::json!({}),
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
        legacy_store
            .finish("colliding-id", EventStatus::Done)
            .unwrap();
    }

    // The shared store already has a row with the same id, written natively
    // (e.g. an earlier partial import, or an id somehow reused) - `INSERT OR
    // IGNORE` must keep this one, not overwrite it with the legacy copy.
    let shared_store = EventStore::open_with_db(&app_data, &history_db_path(&data_root)).unwrap();
    shared_store
        .record(
            "colliding-id",
            &EventDraft {
                kind: "install".to_string(),
                skill: "pi-shared".to_string(),
                harness: None,
                scope: None,
                project_path: None,
                payload: serde_json::json!({}),
                inverse: None,
                backup_dir: None,
                restorable: false,
            },
        )
        .unwrap();
    shared_store
        .finish("colliding-id", EventStatus::Done)
        .unwrap();

    shared_store.import_legacy_events().unwrap();
    let row = shared_store.get("colliding-id").unwrap().unwrap();
    assert_eq!(
        row.skill, "pi-shared",
        "INSERT OR IGNORE must keep the shared store's own row over the legacy import"
    );
    assert_eq!(row.kind, "install");

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn legacy_import_retries_without_duplicates_after_a_failed_rename() {
    let home = unique_temp_dir("undo_legacy_import_rename_retry");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let app_data = home.join("app_data");
    std::fs::create_dir_all(&app_data).unwrap();

    let legacy_path = app_data.join("events.sqlite3");
    {
        let legacy_store = EventStore::open_with_db(&app_data, &legacy_path).unwrap();
        legacy_store
            .record(
                "rho-retry-event",
                &EventDraft {
                    kind: "repair_remove_link".to_string(),
                    skill: "rho-retry".to_string(),
                    harness: None,
                    scope: None,
                    project_path: None,
                    payload: serde_json::json!({}),
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
        legacy_store
            .finish("rho-retry-event", EventStatus::Done)
            .unwrap();
    }

    let shared_store = EventStore::open_with_db(&app_data, &history_db_path(&data_root)).unwrap();

    // Simulate "commit succeeded, rename failed": import once, then put the
    // legacy file straight back (as if the `fs::rename` inside
    // `import_legacy_events` had failed and left the original file in
    // place) rather than where `.migrated` would leave it.
    let imported_first = shared_store.import_legacy_events().unwrap();
    assert_eq!(imported_first, 1);
    std::fs::rename(app_data.join("events.sqlite3.migrated"), &legacy_path).unwrap();

    let imported_second = shared_store.import_legacy_events().unwrap();
    assert_eq!(
        imported_second, 0,
        "retrying after a simulated failed rename must import nothing new"
    );

    let rows = shared_store.list(50, Some("rho-retry")).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "the retried import must not have duplicated the row"
    );

    std::fs::remove_dir_all(&home).ok();
}

// ---------------------------------------------------------------------
// (j) Startup reconciliation flips every `pending` row to `interrupted`,
// not just the first one it finds, and never touches a `done` row.
// ---------------------------------------------------------------------

#[test]
fn reconcile_at_startup_interrupts_every_pending_row_and_leaves_done_rows_alone() {
    let home = unique_temp_dir("undo_reconcile_two_pending");
    std::fs::create_dir_all(&home).unwrap();
    let data_root = data_root_for(&home);
    let store = desktop_store(&home, &data_root);

    for id in ["sigma-pending-1", "sigma-pending-2"] {
        store
            .record(
                id,
                &EventDraft {
                    kind: "repair_remove_link".to_string(),
                    skill: id.to_string(),
                    harness: None,
                    scope: None,
                    project_path: None,
                    payload: serde_json::json!({}),
                    inverse: None,
                    backup_dir: None,
                    restorable: false,
                },
            )
            .unwrap();
        // Left `pending` deliberately - `record` never calls `finish`.
    }
    store
        .record(
            "sigma-done",
            &EventDraft {
                kind: "repair_remove_link".to_string(),
                skill: "sigma-done".to_string(),
                harness: None,
                scope: None,
                project_path: None,
                payload: serde_json::json!({}),
                inverse: None,
                backup_dir: None,
                restorable: false,
            },
        )
        .unwrap();
    store.finish("sigma-done", EventStatus::Done).unwrap();

    let flipped = store.reconcile_at_startup().unwrap();
    let flipped_ids: Vec<&str> = flipped.iter().map(|r| r.id.as_str()).collect();
    assert!(flipped_ids.contains(&"sigma-pending-1"));
    assert!(flipped_ids.contains(&"sigma-pending-2"));
    assert_eq!(
        store.get("sigma-pending-1").unwrap().unwrap().status,
        "interrupted"
    );
    assert_eq!(
        store.get("sigma-pending-2").unwrap().unwrap().status,
        "interrupted"
    );
    assert_eq!(
        store.get("sigma-done").unwrap().unwrap().status,
        "done",
        "reconcile must not touch an already-completed row"
    );

    std::fs::remove_dir_all(&home).ok();
}
