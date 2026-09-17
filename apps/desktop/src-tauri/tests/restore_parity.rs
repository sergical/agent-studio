//! Restore parity between the desktop's own `EventStore`
//! (`apps/desktop/src-tauri/src/skills/event_store.rs`) and the shared
//! core's `ops::{preview_frontmatter_repair, apply_frontmatter_repair,
//! list_events, restore_event}` backed by `SqliteHistoryStore`
//! (`crates/skill-studio-host/src/history.rs`).
//!
//! Each test starts from an identical materialized tree, then runs one copy
//! of it ("Run A") entirely through the desktop's `EventStore` and another
//! copy ("Run B") entirely through the core's ops. Both runs are asserted to
//! leave byte-identical trees (file contents, symlink targets, which paths
//! exist) and to agree on the fields `EventDto` exposes on the wire.
//!
//! Writing this test surfaced a genuine core/desktop divergence, now fixed
//! rather than documented: the desktop's `InverseOp::RestoreBackup` schema
//! stores `pre_fingerprint`/`post_fingerprint` as bare hex (or the literal
//! `"absent"`), but `identity::Fingerprint::as_str()` returns the prefixed
//! `"sha256:<hex>"` form used elsewhere in the core. Three sites were
//! writing or comparing that prefixed form against the bare-hex schema -
//! `events::restore_backup_inverse`, `SqliteHistoryStore::finish`, and
//! `ops::list_events`/`ops::restore_event`'s drift checks - which meant a
//! desktop-authored event could never drift-check clean or restore without
//! `force` through the core, and vice versa once the core patched a row's
//! `post_fingerprint`. Fixed by adding `Fingerprint::bare_hex()` and using
//! it at all three sites (see those files' history). This, plus the earlier
//! `restorable`-column fix and the `desktop_repair_apply_modes` gate fix in
//! `crates/skill-studio-core/src/ops.rs`, are why every case here now
//! passes - both were fixed in production code, so neither is recorded in
//! section 9 of `docs/spec-core-primitives.md`.
//!
//! Two API gaps shape how Run A is built: `EventStore::patch_inverse_post_fingerprint`
//! and the Tauri command `apply_skill_frontmatter_repair` are both
//! unreachable from an external integration-test crate (the first is
//! `pub(crate)`, the second needs live `tauri::AppHandle`/state). Run A
//! therefore replicates `apply_skill_frontmatter_repair`'s `ApplyFix`
//! branch by hand, using only `EventStore`'s public API and
//! `propose_colon_scalar_repair` (both `pub`), and records the finished
//! inverse (with `post_fingerprint` already filled in) in a single
//! `record` call instead of production's record-then-patch. The row this
//! produces is byte-identical to what `patch_inverse_post_fingerprint`
//! would have produced, so this does not change what is being compared.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_studio_core::dto::{
    ListEventsRequest, RepairApplyMode, RepairApplyRequest, RepairPreviewRequest, RestoreRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

use skill_studio_lib::event_store::{
    allocate_id, fingerprint_path, EventDraft as DesktopEventDraft, EventRow, EventStatus,
    EventStore, InverseOp,
};
use skill_studio_lib::skill_frontmatter_repair::propose_colon_scalar_repair;

/// A per-harness root with no `.skill-lock.json` entry, so the core
/// classifies the deployment `Manual`-owned. The core's repair gate
/// (`desktop_repair_apply_modes` in `crates/skill-studio-core/src/ops.rs`)
/// only writes `ApplyFix` for `Manual`/`Copy`/`Fork` owners, matching the
/// desktop's carve-out in `skill_frontmatter_repair::apply_modes` - see
/// `crates/skill-studio-core/tests/repair_and_restore.rs`'s
/// `MANUAL_SKILL_ROOT_RELATIVE` for the same constraint.
const REPAIRABLE_SKILL_RELATIVE: &str = ".claude/skills/zeta-bad";
const MALFORMED_FRONTMATTER: &[u8] =
    b"---\nname: zeta-bad\ndescription: Use this: when needed\n---\nBody.\n";

fn write_repairable_skill(home: &Path) -> PathBuf {
    let dir = home.join(REPAIRABLE_SKILL_RELATIVE);
    fs::create_dir_all(&dir).unwrap();
    let skill_md = dir.join("SKILL.md");
    fs::write(&skill_md, MALFORMED_FRONTMATTER).unwrap();
    skill_md
}

// ---------------------------------------------------------------------
// Tree snapshot: file contents, symlink targets, which paths exist.
// Bookkeeping directories (the desktop's `app_data`, the core's `.history`
// and `.leases`) hold event logs and byte backups, not materialized skill
// state, so they are excluded - the parity claim is about the tree the two
// implementations manage, not their internal history stores.
// ---------------------------------------------------------------------

const BOOKKEEPING_DIRS: &[&str] = &["app_data", ".history", ".leases"];

#[derive(Debug, PartialEq, Eq)]
enum Entry {
    File(Vec<u8>),
    Symlink(PathBuf),
}

fn snapshot_materialized_tree(home: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut out = BTreeMap::new();
    if home.exists() {
        walk_tree(home, home, &mut out);
    }
    out
}

fn walk_tree(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap().to_path_buf();
        if rel.components().count() == 1 {
            if let Some(name) = rel.to_str() {
                if BOOKKEEPING_DIRS.contains(&name) {
                    continue;
                }
            }
        }
        let meta = fs::symlink_metadata(&path).unwrap();
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            out.insert(rel, Entry::Symlink(fs::read_link(&path).unwrap()));
        } else if file_type.is_dir() {
            walk_tree(root, &path, out);
        } else {
            out.insert(rel, Entry::File(fs::read(&path).unwrap()));
        }
    }
}

// ---------------------------------------------------------------------
// Wire-shape comparison: the subset of fields `EventDto` exposes, read off
// either the desktop's raw `EventRow` or the core's `EventDto`.
// ---------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
struct WireShape {
    kind: String,
    skill: String,
    harness: Option<String>,
    scope: Option<String>,
    project_path: Option<PathBuf>,
    status: String,
    restore: &'static str,
    // Each run's own `backup_dir` embeds a randomly-generated event id, so
    // the two independently-run stores never share the literal string; only
    // presence (a backup exists vs. it doesn't) is part of the wire
    // contract being compared here.
    has_backup_dir: bool,
}

fn desktop_wire_shape(row: &EventRow) -> WireShape {
    let restore = if row.reverted_by.is_some() {
        "reverted"
    } else if !row.restorable || row.inverse.is_none() {
        "no_inverse"
    } else {
        "yes"
    };
    WireShape {
        kind: row.kind.clone(),
        skill: row.skill.clone(),
        harness: row.harness.clone(),
        scope: row.scope.clone(),
        project_path: row.project_path.clone().map(PathBuf::from),
        status: row.status.clone(),
        restore,
        has_backup_dir: row.backup_dir.is_some(),
    }
}

fn core_wire_shape(dto: &skill_studio_core::dto::EventDto) -> WireShape {
    use skill_studio_core::dto::RestoreCapability;
    let restore = match dto.restore {
        RestoreCapability::Yes => "yes",
        RestoreCapability::Reverted { .. } => "reverted",
        RestoreCapability::NoInverse => "no_inverse",
        RestoreCapability::UnknownKind => "unknown_kind",
    };
    WireShape {
        kind: dto.kind.clone(),
        skill: dto.skill.0.clone(),
        harness: dto.harness.as_ref().map(|a| a.as_str().to_string()),
        scope: dto.scope.clone(),
        project_path: dto.project_path.clone(),
        status: dto.status.clone(),
        restore,
        has_backup_dir: dto.backup_dir.is_some(),
    }
}

// ---------------------------------------------------------------------
// Run A: the desktop's own `EventStore`, replicating
// `apply_skill_frontmatter_repair`'s `ApplyFix` branch by hand (see the
// module header for why).
// ---------------------------------------------------------------------

fn desktop_store(home: &Path) -> EventStore {
    EventStore::open(&home.join("app_data")).unwrap()
}

/// Applies the one safe deterministic repair to `skill_md` through the
/// desktop's `EventStore`, byte-for-byte matching
/// `apply_skill_frontmatter_repair`'s `ApplyFix` branch. Returns the new
/// event's id.
fn desktop_apply_repair(store: &EventStore, skill_md: &Path, skill: &str) -> String {
    let original = fs::read_to_string(skill_md).unwrap();
    let (proposed, _reason) = propose_colon_scalar_repair(&original).unwrap();

    let event_id = allocate_id();
    let pre_fingerprint = fingerprint_path(skill_md);
    store
        .backup_paths(&event_id, std::slice::from_ref(&skill_md.to_path_buf()))
        .unwrap();
    fs::write(skill_md, proposed.as_bytes()).unwrap();
    let post_fingerprint = fingerprint_path(skill_md);

    store
        .record(
            &event_id,
            DesktopEventDraft {
                kind: "repair_skill_frontmatter".to_string(),
                skill: skill.to_string(),
                // `REPAIRABLE_SKILL_RELATIVE` lives under `.claude/skills`,
                // which the core infers as the Claude Code harness during
                // `ops::scan` - match that here so the two rows agree.
                harness: Some("claude-code".to_string()),
                scope: Some("global".to_string()),
                project_path: None,
                payload: serde_json::json!({}),
                inverse: Some(
                    serde_json::to_value(InverseOp::RestoreBackup {
                        path: skill_md.to_path_buf(),
                        pre_fingerprint,
                        post_fingerprint: Some(post_fingerprint),
                    })
                    .unwrap(),
                ),
                backup_dir: Some(format!("backups/{event_id}")),
                restorable: true,
            },
        )
        .unwrap();
    store.finish(&event_id, EventStatus::Done).unwrap();
    event_id
}

// ---------------------------------------------------------------------
// Run B: the core's ops, backed by the host's `SqliteHistoryStore`.
// ---------------------------------------------------------------------

fn core_runtime_for(home: &Path) -> Runtime {
    core_runtime_at(home, home.join(".history").join("events.sqlite3"))
}

/// Same as `core_runtime_for`, but pointed at an arbitrary `db_path` - used
/// by the crossing-case test to open the desktop's own `app_data/events.sqlite3`.
fn core_runtime_at(home: &Path, db_path: PathBuf) -> Runtime {
    let scope = RuntimeScope::fixture(home);
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
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

/// Applies the one safe deterministic repair to the fixture's only
/// deployment through the core's ops. Returns the repair event's id.
fn core_apply_repair(rt: &Runtime) -> skill_studio_core::identity::EventId {
    let inventory = ops::scan(rt, &ctx(), &Default::default()).unwrap();
    let deployment = &inventory.skills[0].deployments[0];
    let preview = ops::preview_frontmatter_repair(
        rt,
        &ctx(),
        &RepairPreviewRequest {
            deployment_id: deployment.id.clone(),
        },
    )
    .unwrap();
    let outcome = ops::apply_frontmatter_repair(
        rt,
        &ctx(),
        &RepairApplyRequest {
            preview,
            mode: RepairApplyMode::ApplyFix,
        },
    )
    .unwrap();
    match outcome {
        skill_studio_core::dto::RepairOutcome::Applied { event_id, .. } => event_id,
        other => panic!("expected Applied, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// (a) Repair applied then reverted, plus (d) a second restore of an
// already-reverted event is refused.
// ---------------------------------------------------------------------

#[test]
fn repair_apply_then_restore_matches_between_desktop_and_core() {
    let home_a = unique_temp_dir("repair_a");
    let home_b = unique_temp_dir("repair_b");
    let skill_md_a = write_repairable_skill(&home_a);
    write_repairable_skill(&home_b);

    // Run A: desktop.
    let store_a = desktop_store(&home_a);
    let repair_id_a = desktop_apply_repair(&store_a, &skill_md_a, "zeta-bad");
    let row_a = store_a.get(&repair_id_a).unwrap().unwrap();

    // Run B: core.
    let rt_b = core_runtime_for(&home_b);
    let repair_id_b = core_apply_repair(&rt_b);
    let events_b = ops::list_events(&rt_b, &ctx(), &ListEventsRequest::default()).unwrap();
    let dto_b = events_b.iter().find(|e| e.id == repair_id_b).unwrap();

    assert_eq!(
        snapshot_materialized_tree(&home_a),
        snapshot_materialized_tree(&home_b),
        "trees diverged right after the repair was applied"
    );
    assert_eq!(desktop_wire_shape(&row_a), core_wire_shape(dto_b));

    // Restore on both.
    store_a.restore(&repair_id_a, false).unwrap();
    let outcome_b = ops::restore_event(
        &rt_b,
        &ctx(),
        &RestoreRequest {
            event_id: repair_id_b.clone(),
            force: false,
        },
    )
    .unwrap();
    assert_eq!(outcome_b.reverted_event_id, repair_id_b);

    assert_eq!(
        snapshot_materialized_tree(&home_a),
        snapshot_materialized_tree(&home_b),
        "trees diverged after restore"
    );
    let row_a = store_a.get(&repair_id_a).unwrap().unwrap();
    let events_b = ops::list_events(&rt_b, &ctx(), &ListEventsRequest::default()).unwrap();
    let dto_b = events_b.iter().find(|e| e.id == repair_id_b).unwrap();
    assert_eq!(desktop_wire_shape(&row_a), core_wire_shape(dto_b));
    assert!(row_a.reverted_by.is_some());
    assert!(matches!(
        dto_b.restore,
        skill_studio_core::dto::RestoreCapability::Reverted { .. }
    ));

    // (d) A second restore of the same, already-reverted event is refused
    // on both, and neither writes anything.
    let before_a = snapshot_materialized_tree(&home_a);
    let before_b = snapshot_materialized_tree(&home_b);
    let err_a = store_a.restore(&repair_id_a, false).unwrap_err();
    assert!(err_a.contains("already restored"), "unexpected: {err_a}");
    let err_b = ops::restore_event(
        &rt_b,
        &ctx(),
        &RestoreRequest {
            event_id: repair_id_b,
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(err_b.code, skill_studio_core::ErrorCode::AlreadyReverted);
    assert_eq!(before_a, snapshot_materialized_tree(&home_a));
    assert_eq!(before_b, snapshot_materialized_tree(&home_b));

    fs::remove_dir_all(&home_a).ok();
    fs::remove_dir_all(&home_b).ok();
}

// ---------------------------------------------------------------------
// (b) A backup whose path did not exist restores by removing whatever now
// sits there, rather than recreating it.
// ---------------------------------------------------------------------

#[test]
fn restore_of_an_absent_path_backup_removes_it_on_both() {
    let home_a = unique_temp_dir("absent_a");
    let home_b = unique_temp_dir("absent_b");
    fs::create_dir_all(&home_a).unwrap();
    fs::create_dir_all(&home_b).unwrap();
    let missing_a = home_a.join(".agents/skills/never-existed.txt");
    let missing_b = home_b.join(".agents/skills/never-existed.txt");

    // Run A: desktop.
    let store_a = desktop_store(&home_a);
    let id_a = allocate_id();
    store_a
        .backup_paths(&id_a, std::slice::from_ref(&missing_a))
        .unwrap();
    store_a
        .record(
            &id_a,
            DesktopEventDraft {
                kind: "repair_skill_frontmatter".to_string(),
                skill: "never-existed".to_string(),
                harness: None,
                scope: None,
                project_path: None,
                payload: serde_json::json!({}),
                inverse: Some(
                    serde_json::to_value(InverseOp::RestoreBackup {
                        path: missing_a.clone(),
                        pre_fingerprint: "absent".to_string(),
                        post_fingerprint: Some("absent".to_string()),
                    })
                    .unwrap(),
                ),
                backup_dir: Some(format!("backups/{id_a}")),
                restorable: true,
            },
        )
        .unwrap();
    store_a.finish(&id_a, EventStatus::Done).unwrap();

    // Run B: core, hand-built the same way
    // `crates/skill-studio-core/tests/repair_and_restore.rs`'s
    // `a_backup_of_an_absent_path_round_trips_and_restores_by_removing_it`
    // does, since the same `pub(crate)` boundary applies here.
    let rt_b = core_runtime_for(&home_b);
    let id_b = {
        let mut store = rt_b
            .ports
            .history
            .open(
                &rt_b.scope,
                skill_studio_core::ports::HistoryAccess::ReadWrite,
            )
            .unwrap()
            .unwrap();
        let guard =
            skill_studio_core::ports::acquire_exclusive(rt_b.ports.leases.as_ref(), &rt_b.scope)
                .unwrap();
        let id = rt_b.ports.ids.next_event_id();
        store
            .backup_paths(&guard, &id, std::slice::from_ref(&missing_b))
            .unwrap();
        let inverse = serde_json::json!({
            "op": "restore_backup",
            "path": missing_b,
            "pre_fingerprint": "absent",
            "post_fingerprint": "absent",
        });
        store
            .record(
                &guard,
                &id,
                &skill_studio_core::events::EventDraft {
                    kind: skill_studio_core::events::EventKind::RepairSkillFrontmatter,
                    skill: skill_studio_core::identity::SkillName("never-existed".into()),
                    harness: None,
                    scope: None,
                    project_path: None,
                    payload: serde_json::json!({}),
                    inverse: Some(inverse),
                    backup_dir: Some(format!("backups/{}", id.0)),
                },
            )
            .unwrap();
        store
            .finish(
                &guard,
                &id,
                skill_studio_core::events::EventStatus::Done,
                None,
            )
            .unwrap();
        id
    };

    // Something now exists where each event's backup recorded "absent".
    fs::create_dir_all(missing_a.parent().unwrap()).unwrap();
    fs::write(&missing_a, b"created after the fact\n").unwrap();
    fs::create_dir_all(missing_b.parent().unwrap()).unwrap();
    fs::write(&missing_b, b"created after the fact\n").unwrap();
    assert_eq!(
        snapshot_materialized_tree(&home_a),
        snapshot_materialized_tree(&home_b)
    );

    // Something now exists where the backup recorded "absent", so both
    // implementations see drift and need force to proceed.
    store_a.restore(&id_a, true).unwrap();
    let outcome_b = ops::restore_event(
        &rt_b,
        &ctx(),
        &RestoreRequest {
            event_id: id_b,
            force: true,
        },
    )
    .unwrap();
    assert!(!missing_a.exists());
    assert_eq!(outcome_b.restored_paths, vec![missing_b.clone()]);
    assert!(!missing_b.exists());
    assert_eq!(
        snapshot_materialized_tree(&home_a),
        snapshot_materialized_tree(&home_b)
    );

    fs::remove_dir_all(&home_a).ok();
    fs::remove_dir_all(&home_b).ok();
}

// ---------------------------------------------------------------------
// (c) A restore is refused when the file drifted after the event, unless
// forced; force proceeds and preserves the drifted bytes in its own
// backup.
// ---------------------------------------------------------------------

#[test]
fn restore_refuses_drift_without_force_and_force_proceeds_on_both() {
    let home_a = unique_temp_dir("drift_a");
    let home_b = unique_temp_dir("drift_b");
    let skill_md_a = write_repairable_skill(&home_a);
    write_repairable_skill(&home_b);

    let store_a = desktop_store(&home_a);
    let repair_id_a = desktop_apply_repair(&store_a, &skill_md_a, "zeta-bad");
    let rt_b = core_runtime_for(&home_b);
    let repair_id_b = core_apply_repair(&rt_b);

    // Drift both trees identically, bypassing each implementation.
    fs::write(&skill_md_a, b"drifted by another process\n").unwrap();
    let skill_md_b = home_b.join(REPAIRABLE_SKILL_RELATIVE).join("SKILL.md");
    fs::write(&skill_md_b, b"drifted by another process\n").unwrap();
    assert_eq!(
        snapshot_materialized_tree(&home_a),
        snapshot_materialized_tree(&home_b)
    );

    // Without force: refused on both, tree unchanged.
    let err_a = store_a.restore(&repair_id_a, false).unwrap_err();
    assert!(
        err_a.contains(&skill_md_a.display().to_string()),
        "unexpected: {err_a}"
    );
    let err_b = ops::restore_event(
        &rt_b,
        &ctx(),
        &RestoreRequest {
            event_id: repair_id_b.clone(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(err_b.code, skill_studio_core::ErrorCode::DriftConflict);
    assert_eq!(
        snapshot_materialized_tree(&home_a),
        snapshot_materialized_tree(&home_b)
    );

    // With force: proceeds on both, and the trees stay identical (both
    // restore the pre-repair bytes and both stash the drifted bytes in the
    // restore event's own backup).
    let restore_id_a = store_a.restore(&repair_id_a, true).unwrap();
    let outcome_b = ops::restore_event(
        &rt_b,
        &ctx(),
        &RestoreRequest {
            event_id: repair_id_b,
            force: true,
        },
    )
    .unwrap();
    assert!(!outcome_b.restored_paths.is_empty());
    assert_eq!(
        snapshot_materialized_tree(&home_a),
        snapshot_materialized_tree(&home_b)
    );

    // Restoring the restore itself brings back the drifted bytes, on both.
    store_a.restore(&restore_id_a, false).unwrap();
    let restore_row_b = ops::list_events(&rt_b, &ctx(), &ListEventsRequest::default())
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "restore")
        .unwrap();
    ops::restore_event(
        &rt_b,
        &ctx(),
        &RestoreRequest {
            event_id: restore_row_b.id,
            force: false,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read(&skill_md_a).unwrap(),
        b"drifted by another process\n"
    );
    assert_eq!(
        fs::read(&skill_md_b).unwrap(),
        b"drifted by another process\n"
    );
    assert_eq!(
        snapshot_materialized_tree(&home_a),
        snapshot_materialized_tree(&home_b)
    );

    fs::remove_dir_all(&home_a).ok();
    fs::remove_dir_all(&home_b).ok();
}

// ---------------------------------------------------------------------
// (e) Crossing case: record with the desktop's `EventStore`, restore
// through the core, and check that produces the same tree a desktop
// restore of the same event would.
// ---------------------------------------------------------------------

#[test]
fn an_event_recorded_by_the_desktop_restores_through_the_core() {
    let home_cross = unique_temp_dir("cross_core");
    let home_desktop = unique_temp_dir("cross_desktop");
    let skill_md_cross = write_repairable_skill(&home_cross);
    let skill_md_desktop = write_repairable_skill(&home_desktop);

    // Both events are recorded the desktop's way.
    let store_cross = desktop_store(&home_cross);
    let id_cross = desktop_apply_repair(&store_cross, &skill_md_cross, "zeta-bad");
    let store_desktop = desktop_store(&home_desktop);
    let id_desktop = desktop_apply_repair(&store_desktop, &skill_md_desktop, "zeta-bad");
    assert_eq!(
        snapshot_materialized_tree(&home_cross),
        snapshot_materialized_tree(&home_desktop),
        "the two desktop-recorded copies must start identical"
    );

    // `home_desktop`'s copy is restored the desktop's way - the baseline
    // this test compares the core's restore against.
    store_desktop.restore(&id_desktop, false).unwrap();

    // `home_cross`'s copy is restored through the core, pointed straight at
    // the desktop's own `app_data/events.sqlite3` and `app_data/backups/`
    // rather than a core-managed `.history` directory - proving the two
    // implementations share one database and backup layout, not just
    // interchangeable inverse-op semantics.
    let db_path = home_cross.join("app_data").join("events.sqlite3");
    let rt_cross = core_runtime_at(&home_cross, db_path);
    let events = ops::list_events(&rt_cross, &ctx(), &ListEventsRequest::default()).unwrap();
    let row = events
        .iter()
        .find(|e| e.kind == "repair_skill_frontmatter")
        .unwrap();
    assert_eq!(row.id.0, id_cross);
    ops::restore_event(
        &rt_cross,
        &ctx(),
        &RestoreRequest {
            event_id: row.id.clone(),
            force: false,
        },
    )
    .unwrap();

    assert_eq!(
        snapshot_materialized_tree(&home_cross),
        snapshot_materialized_tree(&home_desktop),
        "a core restore of a desktop-recorded event must match a desktop restore"
    );

    fs::remove_dir_all(&home_cross).ok();
    fs::remove_dir_all(&home_desktop).ok();
}
