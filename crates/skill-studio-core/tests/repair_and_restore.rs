//! Real-disk integration tests for Phase 2's mutation path: frontmatter
//! repair, event listing with drift, restore, and crash recovery.
//!
//! These use `skill-studio-host`'s real adapters (`RealFs`, `FileLease`,
//! `SqliteHistoryOpener`) rather than the in-memory `FixtureFs`, because
//! `SqliteHistoryStore::backup_paths`/`read_manifest`/`read_backup_bytes`
//! operate on raw disk outside the `ScopeFs` confinement (see
//! `skill-studio-host/src/history.rs`), so a fake filesystem can't stand in
//! for them here.

use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::{
    DriftState, ListEventsRequest, RepairApplyMode, RepairApplyRequest, RepairPreviewRequest,
    RestoreRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";
/// Root `repairable_home` places its skill under: a per-harness root with no
/// lock entry, so the deployment is `Manual`-owned. Core only writes
/// `ApplyFix` (see `desktop_repair_apply_modes` in `ops.rs`), which the
/// desktop's gate offers to `Manual`/`Copy`/`Fork` owners but not to a
/// `SkillsSh`/`Dotagents`-owned canonical universal deployment (those get
/// `ForkAndFix`/`FixInstalledCopy` instead) - so this fixture must be
/// `Manual`-owned for `preview_frontmatter_repair`/`apply_frontmatter_repair`
/// to accept it.
const MANUAL_SKILL_ROOT_RELATIVE: &str = ".claude/skills";

/// Materializes a home with one `Manual`-owned skill (a per-harness root, no
/// lock entry) whose `SKILL.md` has the unquoted `description: a: b` shape
/// `propose_colon_scalar_repair` knows how to fix - see
/// `MANUAL_SKILL_ROOT_RELATIVE`'s doc comment for why it must be `Manual`.
fn repairable_home(home: &Path) {
    let dir = home.join(MANUAL_SKILL_ROOT_RELATIVE).join("zeta-bad");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        b"---\nname: zeta-bad\ndescription: Use this: when needed\n---\nBody.\n",
    )
    .unwrap();
}

fn runtime_for(home: &Path) -> Runtime {
    let history_root = home.join(".history");
    let db_path = history_root.join("events.sqlite3");
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

/// preview -> apply -> list (drift clean) -> restore -> list (reverted),
/// with the restored bytes matching the original byte for byte.
#[test]
fn apply_then_restore_round_trips_the_original_bytes() {
    let home = unique_temp_dir("round_trip");
    repairable_home(&home);
    let rt = runtime_for(&home);
    let skill_md = home
        .join(MANUAL_SKILL_ROOT_RELATIVE)
        .join("zeta-bad")
        .join("SKILL.md");
    let original_bytes = std::fs::read(&skill_md).unwrap();

    let inventory = ops::scan(&rt, &ctx(), &Default::default()).unwrap();
    let deployment = &inventory.skills[0].deployments[0];
    // `Manual`-owned deployments are `ReadOnly` (see
    // `MANUAL_SKILL_ROOT_RELATIVE`'s doc comment); the repair gate's
    // `owner_kind != Manual` carve-out is what makes this one repairable
    // despite that, which the rest of this test proves by succeeding.
    assert_eq!(
        deployment.owner_kind,
        skill_studio_core::identity::LifecycleOwnerKind::Manual
    );
    assert_eq!(
        deployment.mutability,
        skill_studio_core::identity::DeploymentMutability::ReadOnly
    );

    let preview = ops::preview_frontmatter_repair(
        &rt,
        &ctx(),
        &RepairPreviewRequest {
            deployment_id: deployment.id.clone(),
        },
    )
    .unwrap();

    let apply_outcome = ops::apply_frontmatter_repair(
        &rt,
        &ctx(),
        &RepairApplyRequest {
            preview: preview.clone(),
            mode: RepairApplyMode::ApplyFix,
        },
    )
    .unwrap();
    let repair_event_id = match apply_outcome {
        skill_studio_core::dto::RepairOutcome::Applied { event_id, .. } => event_id,
        other => panic!("expected Applied, got {other:?}"),
    };
    let repaired_bytes = std::fs::read(&skill_md).unwrap();
    assert_ne!(repaired_bytes, original_bytes);
    assert_eq!(
        skill_studio_core::identity::Fingerprint::of_bytes(&repaired_bytes),
        preview.proposed_fingerprint
    );

    // Applying the same preview again is a no-op, not a second write.
    let again = ops::apply_frontmatter_repair(
        &rt,
        &ctx(),
        &RepairApplyRequest {
            preview: preview.clone(),
            mode: RepairApplyMode::ApplyFix,
        },
    )
    .unwrap();
    assert!(matches!(
        again,
        skill_studio_core::dto::RepairOutcome::AlreadyApplied { .. }
    ));

    let events_before = ops::list_events(
        &rt,
        &ctx(),
        &ListEventsRequest {
            check_drift: true,
            ..Default::default()
        },
    )
    .unwrap();
    let repair_row = events_before
        .iter()
        .find(|e| e.id == repair_event_id)
        .unwrap();
    assert_eq!(repair_row.drift, DriftState::Clean);
    assert_eq!(
        repair_row.restore,
        skill_studio_core::dto::RestoreCapability::Yes
    );

    let restore_outcome = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: repair_event_id.clone(),
            force: false,
        },
    )
    .unwrap();
    assert_eq!(restore_outcome.reverted_event_id, repair_event_id);
    let restored_bytes = std::fs::read(&skill_md).unwrap();
    assert_eq!(restored_bytes, original_bytes);

    let events_after = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    let reverted_row = events_after
        .iter()
        .find(|e| e.id == repair_event_id)
        .unwrap();
    assert!(matches!(
        reverted_row.restore,
        skill_studio_core::dto::RestoreCapability::Reverted { .. }
    ));

    // A second restore of the same row is refused, not silently repeated.
    let second = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: repair_event_id,
            force: false,
        },
    );
    assert_eq!(
        second.unwrap_err().code,
        skill_studio_core::ErrorCode::AlreadyReverted
    );

    std::fs::remove_dir_all(&home).ok();
}

/// Mutating the repaired file between apply and restore is reported as
/// drift, and a plain restore (without `force`) is refused and writes
/// nothing.
#[test]
fn restore_refuses_a_drifted_file_without_force() {
    let home = unique_temp_dir("drift");
    repairable_home(&home);
    let rt = runtime_for(&home);
    let skill_md = home
        .join(MANUAL_SKILL_ROOT_RELATIVE)
        .join("zeta-bad")
        .join("SKILL.md");

    let inventory = ops::scan(&rt, &ctx(), &Default::default()).unwrap();
    let deployment = &inventory.skills[0].deployments[0];
    let preview = ops::preview_frontmatter_repair(
        &rt,
        &ctx(),
        &RepairPreviewRequest {
            deployment_id: deployment.id.clone(),
        },
    )
    .unwrap();
    let apply_outcome = ops::apply_frontmatter_repair(
        &rt,
        &ctx(),
        &RepairApplyRequest {
            preview,
            mode: RepairApplyMode::ApplyFix,
        },
    )
    .unwrap();
    let repair_event_id = match apply_outcome {
        skill_studio_core::dto::RepairOutcome::Applied { event_id, .. } => event_id,
        other => panic!("expected Applied, got {other:?}"),
    };

    // Drift the file by hand, bypassing the core.
    std::fs::write(&skill_md, b"drifted by another process\n").unwrap();
    let drifted_bytes = std::fs::read(&skill_md).unwrap();

    let err = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: repair_event_id.clone(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, skill_studio_core::ErrorCode::DriftConflict);
    assert_eq!(std::fs::read(&skill_md).unwrap(), drifted_bytes);

    // `force` proceeds, backing up the drifted bytes first.
    let outcome = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: repair_event_id,
            force: true,
        },
    )
    .unwrap();
    assert!(!outcome.restored_paths.is_empty());

    std::fs::remove_dir_all(&home).ok();
}

/// A backup taken of a path that did not exist round-trips `None` through
/// [`skill_studio_core::events::BackupEntry::fingerprint`] and the on-disk
/// manifest (Section A of the migration), and restoring that event removes
/// whatever now sits at the path rather than writing bytes.
#[test]
fn a_backup_of_an_absent_path_round_trips_and_restores_by_removing_it() {
    let home = unique_temp_dir("absent_path");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);
    let missing = home.join(UNIVERSAL_ROOT_RELATIVE).join("never-existed.txt");

    let (backup_dir, id) = {
        let mut store = rt
            .ports
            .history
            .open(
                &rt.scope,
                skill_studio_core::ports::HistoryAccess::ReadWrite,
            )
            .unwrap()
            .unwrap();
        let guard =
            skill_studio_core::ports::acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope)
                .unwrap();
        let id = rt.ports.ids.next_event_id();
        let manifest = store
            .backup_paths(&guard, &id, std::slice::from_ref(&missing))
            .unwrap();

        // The manifest itself already reports the path as absent.
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.entries[0].fingerprint, None);

        // Reading the manifest back (as `restore_event` does) round-trips
        // that `None` rather than losing the distinction between "absent"
        // and "not yet checked".
        let read_back = store.read_manifest(&manifest.backup_dir).unwrap();
        assert_eq!(read_back.entries[0].fingerprint, None);

        // Hand-built in the same shape `ops::apply_frontmatter_repair` and
        // `ops::restore_event` use (`events::restore_backup_inverse`'s
        // format): the helper itself is `pub(crate)`, so an integration
        // test exercises the same on-the-wire contract through this literal
        // instead.
        let inverse = serde_json::json!({
            "op": "restore_backup",
            "path": missing,
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
                    backup_dir: Some(manifest.backup_dir.clone()),
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
        (manifest.backup_dir, id)
    };
    let _ = backup_dir;

    // Something now exists at the path the original event backed up as
    // absent - restoring that event should remove it.
    std::fs::create_dir_all(missing.parent().unwrap()).unwrap();
    std::fs::write(&missing, b"created after the fact\n").unwrap();
    assert!(missing.exists());

    let outcome = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: id,
            force: true,
        },
    )
    .unwrap();
    assert_eq!(outcome.restored_paths, vec![missing.clone()]);
    assert!(!missing.exists());

    std::fs::remove_dir_all(&home).ok();
}

/// A restore whose original event's backup manifest has no entry for the
/// target path fails without ever claiming `reverted_by` durably against a
/// mutation that never happened: a second restore attempt of the same event
/// hits the same failure rather than `AlreadyReverted`, so the event stays
/// retryable.
#[test]
fn a_restore_that_fails_before_mutating_leaves_the_event_revertible() {
    let home = unique_temp_dir("bad_manifest");
    std::fs::create_dir_all(&home).unwrap();
    let rt = runtime_for(&home);

    let target_path = home.join(UNIVERSAL_ROOT_RELATIVE).join("target.txt");
    std::fs::create_dir_all(target_path.parent().unwrap()).unwrap();
    std::fs::write(&target_path, b"live bytes\n").unwrap();
    // Backed up under the same event id, but a different path than the one
    // the inverse names below - so the manifest lookup by `target_path`
    // finds nothing.
    let decoy_path = home.join(UNIVERSAL_ROOT_RELATIVE).join("decoy.txt");
    std::fs::write(&decoy_path, b"decoy\n").unwrap();

    let id = {
        let mut store = rt
            .ports
            .history
            .open(
                &rt.scope,
                skill_studio_core::ports::HistoryAccess::ReadWrite,
            )
            .unwrap()
            .unwrap();
        let guard =
            skill_studio_core::ports::acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope)
                .unwrap();
        let id = rt.ports.ids.next_event_id();
        let manifest = store
            .backup_paths(&guard, &id, std::slice::from_ref(&decoy_path))
            .unwrap();
        let inverse = serde_json::json!({
            "op": "restore_backup",
            "path": target_path,
            "pre_fingerprint": "deadbeef",
            "post_fingerprint": "deadbeef",
        });
        store
            .record(
                &guard,
                &id,
                &skill_studio_core::events::EventDraft {
                    kind: skill_studio_core::events::EventKind::RepairSkillFrontmatter,
                    skill: skill_studio_core::identity::SkillName("target".into()),
                    harness: None,
                    scope: None,
                    project_path: None,
                    payload: serde_json::json!({}),
                    inverse: Some(inverse),
                    backup_dir: Some(manifest.backup_dir.clone()),
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

    let first = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: id.clone(),
            force: true,
        },
    )
    .unwrap_err();
    assert_eq!(first.code, skill_studio_core::ErrorCode::Io);
    assert_eq!(std::fs::read(&target_path).unwrap(), b"live bytes\n");

    // The claim was never made durable, so a second attempt fails the same
    // way instead of reporting `AlreadyReverted`.
    let second = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: id,
            force: true,
        },
    )
    .unwrap_err();
    assert_eq!(second.code, skill_studio_core::ErrorCode::Io);

    std::fs::remove_dir_all(&home).ok();
}

/// A restore whose write fails after the claim is taken releases the claim
/// (rather than leaving the event stuck `AlreadyReverted`), so a later
/// retry with a working filesystem succeeds.
#[test]
fn a_restore_whose_write_fails_releases_the_claim_for_a_later_retry() {
    let home = unique_temp_dir("write_fails");
    repairable_home(&home);
    let failing_fs = Arc::new(skill_studio_core::testing::FailingFs::wrap(Arc::new(
        RealFs::new(),
    )));
    let rt = {
        let history_root = home.join(".history");
        let db_path = history_root.join("events.sqlite3");
        let scope = RuntimeScope::fixture(&home);
        let ports = Ports {
            fs: failing_fs.clone(),
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
    };
    let skill_md = home
        .join(MANUAL_SKILL_ROOT_RELATIVE)
        .join("zeta-bad")
        .join("SKILL.md");
    let original_bytes = std::fs::read(&skill_md).unwrap();

    let inventory = ops::scan(&rt, &ctx(), &Default::default()).unwrap();
    let deployment = &inventory.skills[0].deployments[0];
    let preview = ops::preview_frontmatter_repair(
        &rt,
        &ctx(),
        &RepairPreviewRequest {
            deployment_id: deployment.id.clone(),
        },
    )
    .unwrap();
    let apply_outcome = ops::apply_frontmatter_repair(
        &rt,
        &ctx(),
        &RepairApplyRequest {
            preview,
            mode: RepairApplyMode::ApplyFix,
        },
    )
    .unwrap();
    let repair_event_id = match apply_outcome {
        skill_studio_core::dto::RepairOutcome::Applied { event_id, .. } => event_id,
        other => panic!("expected Applied, got {other:?}"),
    };
    let repaired_bytes = std::fs::read(&skill_md).unwrap();

    failing_fs.fail_next_write_atomic();
    let failed = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: repair_event_id.clone(),
            force: false,
        },
    )
    .unwrap_err();
    assert_eq!(failed.code, skill_studio_core::ErrorCode::Io);
    // Nothing moved: the file still holds the pre-restore (repaired) bytes.
    assert_eq!(std::fs::read(&skill_md).unwrap(), repaired_bytes);

    // The claim was released, so a retry (with the filesystem working
    // again) succeeds instead of reporting `AlreadyReverted`.
    let outcome = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: repair_event_id,
            force: false,
        },
    )
    .unwrap();
    assert!(!outcome.restored_paths.is_empty());
    assert_eq!(std::fs::read(&skill_md).unwrap(), original_bytes);

    std::fs::remove_dir_all(&home).ok();
}

/// A row left `pending` by a crashed process (never `finish`ed) is flipped
/// to `interrupted` the next time a mutation session starts, and a second
/// startup is a no-op rather than double-flipping it.
#[test]
fn a_crashed_pending_row_is_marked_interrupted_idempotently() {
    let home = unique_temp_dir("crash_recovery");
    repairable_home(&home);
    let rt = runtime_for(&home);

    // Simulate a crash mid-mutation: open the store directly, record a row,
    // and never call `finish`.
    let pending_id = {
        let mut store = rt
            .ports
            .history
            .open(
                &rt.scope,
                skill_studio_core::ports::HistoryAccess::ReadWrite,
            )
            .unwrap()
            .unwrap();
        let guard =
            skill_studio_core::ports::acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope)
                .unwrap();
        let id = rt.ports.ids.next_event_id();
        store
            .record(
                &guard,
                &id,
                &skill_studio_core::events::EventDraft {
                    kind: skill_studio_core::events::EventKind::RepairSkillFrontmatter,
                    skill: skill_studio_core::identity::SkillName("zeta-bad".into()),
                    harness: None,
                    scope: None,
                    project_path: None,
                    payload: serde_json::json!({}),
                    inverse: None,
                    backup_dir: None,
                },
            )
            .unwrap();
        id
        // `guard` and `store` drop here, releasing the exclusive lease -
        // the row is left `pending`, exactly as an interrupted process
        // would leave it.
    };

    // First `MutationSession::begin` (any write op takes one) recovers it.
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    let row = events.iter().find(|e| e.id == pending_id).unwrap();
    assert_eq!(row.status, "pending");

    let session = skill_studio_core::ports::MutationSession::begin(&rt, &ctx()).unwrap();
    session.finish(&rt, &ctx());
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    let row = events.iter().find(|e| e.id == pending_id).unwrap();
    assert_eq!(row.status, "interrupted");

    // A second `begin` finds nothing pending left to recover; the row's
    // terminal state is stable.
    let session = skill_studio_core::ports::MutationSession::begin(&rt, &ctx()).unwrap();
    session.finish(&rt, &ctx());
    let events = ops::list_events(&rt, &ctx(), &ListEventsRequest::default()).unwrap();
    let row = events.iter().find(|e| e.id == pending_id).unwrap();
    assert_eq!(row.status, "interrupted");

    std::fs::remove_dir_all(&home).ok();
}

/// The envelope's `event_id` is the field a surface reads to learn what a
/// mutating call recorded. It is filled from the outcome inside
/// `from_result`, so every surface reports the same id without each call
/// site remembering to set it; an outcome that wrote nothing leaves it
/// `None`.
#[test]
fn the_envelope_carries_the_event_id_a_mutating_call_recorded() {
    use skill_studio_core::ops::{Operation, ResultEnvelope};

    let home = unique_temp_dir("envelope_event");
    repairable_home(&home);
    let rt = runtime_for(&home);

    let inventory = ops::scan(&rt, &ctx(), &Default::default()).unwrap();
    let deployment_id = inventory.skills[0].deployments[0].id.clone();
    let preview =
        ops::preview_frontmatter_repair(&rt, &ctx(), &RepairPreviewRequest { deployment_id })
            .unwrap();
    let request = RepairApplyRequest {
        preview,
        mode: RepairApplyMode::ApplyFix,
    };

    let c = ctx();
    let applied = ops::apply_frontmatter_repair(&rt, &c, &request);
    let envelope = ResultEnvelope::from_result(
        Operation::ApplyFrontmatterRepair,
        &rt.scope,
        c.correlation_id.clone(),
        applied,
    );
    let repair_event_id = envelope
        .event_id
        .clone()
        .expect("an applied repair records an event");
    match envelope.data.as_ref().unwrap() {
        skill_studio_core::dto::RepairOutcome::Applied { event_id, .. } => {
            assert_eq!(*event_id, repair_event_id);
        }
        other => panic!("expected Applied, got {other:?}"),
    }

    // The second apply writes nothing, so it names no event.
    let c = ctx();
    let again = ops::apply_frontmatter_repair(&rt, &c, &request);
    let envelope = ResultEnvelope::from_result(
        Operation::ApplyFrontmatterRepair,
        &rt.scope,
        c.correlation_id.clone(),
        again,
    );
    assert_eq!(envelope.event_id, None);

    // A restore names the restore event it created, not the one it reverted.
    let c = ctx();
    let restored = ops::restore_event(
        &rt,
        &c,
        &RestoreRequest {
            event_id: repair_event_id.clone(),
            force: false,
        },
    );
    let envelope = ResultEnvelope::from_result(
        Operation::RestoreEvent,
        &rt.scope,
        c.correlation_id.clone(),
        restored,
    );
    let outcome = envelope.data.as_ref().unwrap();
    assert_eq!(envelope.event_id.as_ref(), Some(&outcome.restore_event_id));
    assert_eq!(outcome.reverted_event_id, repair_event_id);
    assert_ne!(outcome.restore_event_id, repair_event_id);

    std::fs::remove_dir_all(&home).ok();
}
