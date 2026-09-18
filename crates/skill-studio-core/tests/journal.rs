//! Unit 1.2: the integration test proving every one of `fsops`'s four
//! primitives records a journal entry itself, since Section B moved the
//! recording into the primitives (no more `journaled_*` wrappers). The five
//! lower-level journal tests live inline in `skill_studio_core::journal`'s
//! own test module.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use skill_studio_core::fsops::{self, read_stamp, Root};
use skill_studio_core::identity::PlanId;
use skill_studio_core::journal::{FsJournal, PlanWriter};
use skill_studio_core::ports::{
    ExclusiveGuard, Journal, LeaseMode, LeaseProvider, PlanStatus, ScopeFs,
};
use skill_studio_core::testing::{FakeLease, FixtureBuilder};

fn guard(lease: &FakeLease) -> ExclusiveGuard {
    let handle = lease
        .acquire(&[], LeaseMode::Exclusive, Duration::from_secs(0))
        .expect("acquire exclusive lease");
    ExclusiveGuard::from_handle(handle)
}

/// Given a plan, when each of `fsops`'s four primitives is called, then the
/// plan's recorded steps name all four, in the order they ran; on failure the
/// panic names whichever primitive's call left no matching step.
#[test]
fn every_fsops_call_records_a_journal_entry_or_names_the_unjournaled_write() {
    let fs: Arc<dyn ScopeFs> = Arc::new(
        FixtureBuilder::new()
            .dir("/root")
            .dir("/journal")
            .build_fs(),
    );
    let root_path = PathBuf::from("/root");
    let root = Root::open(fs.as_ref(), root_path.clone()).expect("open root");
    let journal = FsJournal::new(PathBuf::from("/journal"), fs.clone());
    let lease = FakeLease::default();
    let g = guard(&lease);

    let plan = PlanWriter::begin(
        &journal,
        &g,
        PlanId("01PLANFSOPS000000000001".into()),
        Utc::now(),
        "exercise every fsops primitive",
        root_path.clone(),
        Vec::new(),
    )
    .expect("begin");

    let staged = fsops::stage(
        &root,
        &plan,
        &[(PathBuf::from("SKILL.md"), b"hello".to_vec())],
    )
    .expect("stage");
    fsops::swap(
        &root,
        &plan,
        Path::new("alpha"),
        staged,
        Path::new(".trash"),
    )
    .expect("swap");
    fsops::link(&root, &plan, Path::new("alpha-link"), Path::new("alpha")).expect("link");
    let target = root_path.join("alpha").join("SKILL.md");
    let stamp = read_stamp(fs.as_ref(), &target).expect("read stamp");
    fsops::write_file(
        &root,
        &plan,
        Path::new("alpha/SKILL.md"),
        b"updated",
        &stamp,
    )
    .expect("write_file");

    let id = plan.id().clone();
    plan.finish(PlanStatus::Done).expect("finish");

    let record = journal
        .all()
        .expect("read plans back")
        .into_iter()
        .find(|p| p.id == id)
        .expect("the plan begun above");

    let names: Vec<&str> = record.steps.iter().map(|s| s.primitive_name()).collect();
    for expected in ["stage", "swap", "link", "write_file"] {
        assert!(
            names.contains(&expected),
            "no journal entry recorded for {expected}; recorded steps were {names:?}"
        );
    }
    assert_eq!(
        names,
        vec!["stage", "swap", "link", "write_file"],
        "steps must be recorded in the order the primitives ran"
    );
}
