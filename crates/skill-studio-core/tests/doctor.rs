// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Unit 5.3: `ops::doctor` must run all six lifecycle invariants from
//! `docs/action-map/lifecycle-states.md`'s Invariants section as one
//! whole-scope pass - not one skill at a time like `ops::fix_skill` - and
//! report zero violations on a healthy home. One fixture per invariant,
//! named in the issue; each failure message names the invariant and the
//! offending path so a caller that cannot repair it automatically can
//! still tell the user where to look.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use skill_studio_core::doctor::DoctorInvariant;
use skill_studio_core::dto::DoctorRequest;
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{
    PlanId, MOVE_ASIDE_DIR_NAME, PARKED_ROOT_RELATIVE, UNIVERSAL_ROOT_RELATIVE,
};
use skill_studio_core::journal::{FsJournal, PlanWriter};
use skill_studio_core::ops;
use skill_studio_core::ports::{
    ExclusiveGuard, LeaseMode, LeaseProvider, PlanStatus, Ports, Runtime, ScopeFs,
};
use skill_studio_core::testing::golden::{ctx, scope_for};
use skill_studio_core::testing::{FakeClock, FakeIds, FakeLease, FixtureBuilder, NoHistory};

const HOME: &str = "/home";

fn skill_md(name: &str) -> Vec<u8> {
    format!("---\nname: {name}\ndescription: fixture skill for the doctor pass.\n---\nBody.\n")
        .into_bytes()
}

fn runtime(fs: Arc<dyn ScopeFs>) -> Runtime {
    let ports = Ports {
        fs,
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FakeLease::default()),
        history: Arc::new(NoHistory),
        sink: Arc::new(skill_studio_core::testing::RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    let scope = scope_for("doctor", Path::new(HOME));
    Runtime::new(&scope, ports).expect("runtime")
}

fn run_doctor(fs: Arc<dyn ScopeFs>) -> skill_studio_core::dto::DoctorReport {
    let rt = runtime(fs);
    ops::doctor(&rt, &ctx(), &DoctorRequest::default()).expect("doctor")
}

/// A home with one properly installed skill and nothing else: no dangling
/// link, no stale registry or lockfile row, no double state, quarantine
/// under its cap, no open journal plan.
fn healthy_home() -> Arc<dyn ScopeFs> {
    Arc::new(
        FixtureBuilder::new()
            .dir(&format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}/alpha"))
            .file(
                &format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}/alpha/SKILL.md"),
                &skill_md("alpha"),
            )
            .alias(
                &format!("{HOME}/.claude/skills/alpha"),
                &format!("../../{UNIVERSAL_ROOT_RELATIVE}/alpha"),
            )
            .build_fs(),
    )
}

#[test]
fn doctor_reports_zero_violations_on_a_healthy_fixture_home_or_names_the_false_positive() {
    let report = run_doctor(healthy_home());
    assert!(
        report.violations.is_empty(),
        "healthy home reported a false positive: {:?}",
        report.violations
    );
    assert_eq!(
        report.checked, 1,
        "expected the one skill in the fixture to be counted as checked"
    );
}

/// Invariant 1: a per-skill Claude Code symlink whose target does not
/// resolve inside the universal root it names - `scan`'s own
/// `ports::confine` (`lifecycle-states.md`'s "link confinement") refuses to
/// walk an out-of-scope target at all, so the observable "does not resolve
/// inside its root" case `diagnose`'s `BrokenLink` detection surfaces is a
/// dangling target, the same shape `fix_and_conflicts.rs`'s
/// `unrepairable_home` fixture uses; reused, not re-derived, by
/// `doctor::check_link_resolves_in_root` (see that function's own doc).
#[test]
fn doctor_finds_a_link_that_escapes_its_root_or_names_the_check_that_missed_it() {
    let fs: Arc<dyn ScopeFs> = Arc::new(
        FixtureBuilder::new()
            .dir(&format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}"))
            .alias(
                &format!("{HOME}/.claude/skills/ghost"),
                &format!("../../{UNIVERSAL_ROOT_RELATIVE}/missing"),
            )
            .build_fs(),
    );

    let report = run_doctor(fs);
    let found = report
        .violations
        .iter()
        .find(|v| v.invariant == DoctorInvariant::LinkResolvesInRoot);
    assert!(
        found.is_some(),
        "check_link_resolves_in_root missed the escaping link: {:?}",
        report.violations
    );
}

/// Invariant 2: a registry `copies` entry names a path with no folder there.
#[test]
fn doctor_finds_a_registry_entry_with_no_folder_or_names_the_check_that_missed_it() {
    let expected_path = format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}/stale-copy");
    let registry = format!(
        r#"{{"copies":{{"stale-copy":{{"name":"stale-copy","path":"{expected_path}","scope":"global","destination":"universal"}}}}}}"#
    );
    let fs: Arc<dyn ScopeFs> = Arc::new(
        FixtureBuilder::new()
            .dir(&format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}"))
            .file(&format!("{HOME}/.agents/skill-studio.json"), registry.as_bytes())
            .build_fs(),
    );

    let report = run_doctor(fs);
    let found = report
        .violations
        .iter()
        .find(|v| v.invariant == DoctorInvariant::RegistryEntryHasFolder);
    assert!(
        found.is_some(),
        "check_registry_entry_has_folder missed the stale copy entry: {:?}",
        report.violations
    );
    assert_eq!(found.unwrap().path, Path::new(&expected_path));
}

/// Invariant 3: a `.skill-lock.json` entry names a skill with no folder
/// anywhere the scan covered.
#[test]
fn doctor_finds_a_lockfile_entry_with_no_folder_or_names_the_check_that_missed_it() {
    let fs: Arc<dyn ScopeFs> = Arc::new(
        FixtureBuilder::new()
            .dir(&format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}"))
            .file(
                &format!("{HOME}/.agents/.skill-lock.json"),
                br#"{"version":3,"skills":{
                    "ghost-skill":{"source":"o/r","sourceType":"github","sourceUrl":"https://example.com","skillFolderHash":"abc","installedAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}
                }}"#,
            )
            .build_fs(),
    );

    let report = run_doctor(fs);
    let found = report
        .violations
        .iter()
        .find(|v| v.invariant == DoctorInvariant::LockfileEntryHasFolder);
    assert!(
        found.is_some(),
        "check_lockfile_entry_has_folder missed the ghost lockfile row: {:?}",
        report.violations
    );
}

/// Invariant 4: the same skill exists both as a per-harness canonical copy
/// and under the parked root - the shape `park_skill`'s own lifecycle row
/// says never happens once a park lands cleanly.
#[test]
fn doctor_finds_a_folder_in_two_states_at_once_or_names_the_check_that_missed_it() {
    let name = "double-state";
    let fs: Arc<dyn ScopeFs> = Arc::new(
        FixtureBuilder::new()
            .dir(&format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}"))
            .file(
                &format!("{HOME}/.claude/skills/{name}/SKILL.md"),
                &skill_md(name),
            )
            .file(
                &format!("{HOME}/{PARKED_ROOT_RELATIVE}/{name}/SKILL.md"),
                &skill_md(name),
            )
            .build_fs(),
    );

    let report = run_doctor(fs);
    let found = report
        .violations
        .iter()
        .find(|v| v.invariant == DoctorInvariant::NoFolderInTwoStates);
    assert!(
        found.is_some(),
        "check_no_folder_in_two_states missed the double-parked skill: {:?}",
        report.violations
    );
}

/// Invariant 5: the quarantine holding directory has more entries than
/// [`skill_studio_core::doctor::QUARANTINE_RETENTION_CAP`].
#[test]
fn doctor_finds_a_quarantine_folder_past_its_retention_cap_or_names_the_check_that_missed_it() {
    let mut builder = FixtureBuilder::new().dir(&format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}"));
    let cap = skill_studio_core::doctor::QUARANTINE_RETENTION_CAP;
    for i in 0..(cap + 1) {
        builder = builder.file(
            &format!(
                "{HOME}/{UNIVERSAL_ROOT_RELATIVE}/.skill-studio-quarantine/{i:04}-quarantined/SKILL.md"
            ),
            &skill_md("quarantined"),
        );
    }
    let fs: Arc<dyn ScopeFs> = Arc::new(builder.build_fs());

    let report = run_doctor(fs);
    let found = report
        .violations
        .iter()
        .find(|v| v.invariant == DoctorInvariant::QuarantineWithinCap);
    assert!(
        found.is_some(),
        "check_quarantine_within_cap missed the over-cap quarantine dir: {:?}",
        report.violations
    );
}

/// Invariant 6: a journal plan begun but never finished - the case startup
/// reconcile never ran against, since nothing restarted the process here.
#[test]
fn doctor_finds_an_open_plan_at_rest_in_the_journal_or_names_the_check_that_missed_it() {
    let fs: Arc<dyn ScopeFs> = Arc::new(
        FixtureBuilder::new()
            .dir(&format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}"))
            .dir(&format!("{HOME}/.agents/skill-studio-journal"))
            .build_fs(),
    );
    let journal = FsJournal::new(
        Path::new(HOME).join(".agents").join("skill-studio-journal"),
        fs.clone(),
    );
    let lease = FakeLease::default();
    let handle = lease
        .acquire(&[], LeaseMode::Exclusive, Duration::from_secs(0))
        .unwrap();
    let guard = ExclusiveGuard::from_handle(handle);
    PlanWriter::begin(
        &journal,
        &guard,
        PlanId("01PLANDOCTORINTEGRATE0001".into()),
        Utc::now(),
        "doctor integration test plan",
        Path::new(HOME).join(UNIVERSAL_ROOT_RELATIVE),
        Vec::new(),
    )
    .unwrap();

    let report = run_doctor(fs);
    let found = report
        .violations
        .iter()
        .find(|v| v.invariant == DoctorInvariant::JournalHasNoOpenPlan);
    assert!(
        found.is_some(),
        "check_journal_has_no_open_plan missed the open plan: {:?}",
        report.violations
    );
}

/// Unit 2.7 (captured real homes) is deferred, so this stands in with the
/// largest fixture `FixtureBuilder` can express: every harness (Claude
/// Code, Codex, legacy and current OpenCode), a project root, a parked
/// skill, a quarantined entry, a disabled-per-harness (move-aside) skill,
/// a lockfile, and a journal with one finished plan. Follow-up: swap in
/// the 2.7 capture once it exists.
#[test]
fn doctor_completes_without_a_panic_on_a_real_home_directory_snapshot() {
    let mut b = FixtureBuilder::new()
        .dir(&format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}/gamma"))
        .file(
            &format!("{HOME}/{UNIVERSAL_ROOT_RELATIVE}/gamma/SKILL.md"),
            &skill_md("gamma"),
        )
        .alias(
            &format!("{HOME}/.claude/skills/gamma"),
            &format!("../../{UNIVERSAL_ROOT_RELATIVE}/gamma"),
        )
        .alias(
            &format!("{HOME}/.codex/skills/gamma"),
            &format!("../../{UNIVERSAL_ROOT_RELATIVE}/gamma"),
        )
        .dir(&format!("{HOME}/.config/opencode/skill/delta"))
        .file(
            &format!("{HOME}/.config/opencode/skill/delta/SKILL.md"),
            &skill_md("delta"),
        )
        .dir(&format!("{HOME}/.config/opencode/skills/epsilon"))
        .file(
            &format!("{HOME}/.config/opencode/skills/epsilon/SKILL.md"),
            &skill_md("epsilon"),
        )
        .dir(&format!("{HOME}/proj/.git"))
        .dir(&format!("{HOME}/proj/.claude/skills/eta"))
        .file(
            &format!("{HOME}/proj/.claude/skills/eta/SKILL.md"),
            &skill_md("eta"),
        )
        .dir(&format!("{HOME}/{PARKED_ROOT_RELATIVE}/zeta"))
        .file(
            &format!("{HOME}/{PARKED_ROOT_RELATIVE}/zeta/SKILL.md"),
            &skill_md("zeta"),
        )
        .dir(&format!(
            "{HOME}/.claude/skills/{MOVE_ASIDE_DIR_NAME}/kappa"
        ))
        .file(
            &format!("{HOME}/.claude/skills/{MOVE_ASIDE_DIR_NAME}/kappa/SKILL.md"),
            &skill_md("kappa"),
        )
        .file(
            &format!(
                "{HOME}/{UNIVERSAL_ROOT_RELATIVE}/.skill-studio-quarantine/0001-old/SKILL.md"
            ),
            &skill_md("old"),
        )
        .file(
            &format!("{HOME}/.agents/.skill-lock.json"),
            br#"{"version":3,"skills":{"gamma":{"source":"o/r","sourceType":"github","sourceUrl":"https://example.com","skillFolderHash":"abc","installedAt":"2024-01-01T00:00:00Z","updatedAt":"2024-01-01T00:00:00Z"}}}"#,
        )
        .dir(&format!("{HOME}/.agents/skill-studio-journal"));
    b = b.dir(&format!("{HOME}/.agents/skill-studio-journal"));
    let fs: Arc<dyn ScopeFs> = Arc::new(b.build_fs());

    let journal = FsJournal::new(
        Path::new(HOME).join(".agents").join("skill-studio-journal"),
        fs.clone(),
    );
    let lease = FakeLease::default();
    let handle = lease
        .acquire(&[], LeaseMode::Exclusive, Duration::from_secs(0))
        .unwrap();
    let guard = ExclusiveGuard::from_handle(handle);
    let plan = PlanWriter::begin(
        &journal,
        &guard,
        PlanId("01PLANDOCTORFINISHED00001".into()),
        Utc::now(),
        "finished plan, no longer open",
        Path::new(HOME).join(UNIVERSAL_ROOT_RELATIVE),
        Vec::new(),
    )
    .unwrap();
    plan.finish(PlanStatus::Done).unwrap();

    // Must not panic; a panic here fails the test on its own.
    let report = run_doctor(fs);
    assert!(
        report.checked > 0,
        "expected the largest fixture to count at least one skill checked"
    );
}
