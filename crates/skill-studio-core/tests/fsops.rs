// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Unit 1.1: the four `fsops` tests that live on the in-memory `FixtureFs`,
//! a model test comparing random sequences of primitives against a plain
//! reference map, a root-confinement test, a swap/symlink-race test, and a
//! stale-write test. The crash test (real disk, a re-exec'd child process)
//! lives in `skill-studio-host`'s test suite instead, since it needs
//! `RealFs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use proptest::prelude::*;
use skill_studio_core::fsops::{self, read_stamp, Root};
use skill_studio_core::identity::PlanId;
use skill_studio_core::journal::{FsJournal, PlanWriter};
use skill_studio_core::ports::{ExclusiveGuard, LeaseMode, LeaseProvider, ScopeFs};
use skill_studio_core::testing::{FailingFs, FakeLease, FixtureBuilder};

/// Every fsops call in these tests now records its step through a plan,
/// per unit 1.2 Section B. The journal itself is not under test here (see
/// `skill_studio_core::journal`'s own tests and `tests/journal.rs`), so
/// these tests just need a plan writer backed by a scratch journal
/// directory that never collides with the fixture under test.
fn begin_test_plan<'a>(
    journal: &'a FsJournal,
    guard: &'a ExclusiveGuard,
    root_path: PathBuf,
) -> PlanWriter<'a> {
    PlanWriter::begin(
        journal,
        guard,
        PlanId("01PLANFSOPSTEST0000000001".into()),
        Utc::now(),
        "fsops test",
        root_path,
        Vec::new(),
    )
    .expect("begin plan")
}

fn test_guard(lease: &FakeLease) -> ExclusiveGuard {
    let handle = lease
        .acquire(&[], LeaseMode::Exclusive, Duration::from_secs(0))
        .expect("acquire exclusive lease");
    ExclusiveGuard::from_handle(handle)
}

const SKILL_NAMES: [&str; 2] = ["alpha", "beta"];
const CONTENTS: [&[u8]; 3] = [b"one", b"two", b"three"];

#[derive(Debug, Clone, Copy)]
enum Op {
    /// Writes a brand new (or replacement) skill folder via `stage`+`swap`.
    Create { skill: usize, content: usize },
    /// Edits an existing skill's `SKILL.md` via `read_stamp`+`write_file`;
    /// a no-op when the model has no such skill yet.
    Update { skill: usize, content: usize },
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..SKILL_NAMES.len(), 0..CONTENTS.len())
            .prop_map(|(skill, content)| Op::Create { skill, content }),
        (0..SKILL_NAMES.len(), 0..CONTENTS.len())
            .prop_map(|(skill, content)| Op::Update { skill, content }),
    ]
}

fn skill_md(root: &Path, skill: &str) -> PathBuf {
    root.join(skill).join("SKILL.md")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Given a random sequence of `Create`/`Update` ops, when each is
    /// applied through `fsops::stage`/`swap`/`write_file` on a `FixtureFs`
    /// root, then after every step the folder's observable content matches
    /// a plain `BTreeMap` reference model; on the first divergence the
    /// panic names the step index, the op, and what disagreed.
    #[test]
    fn stage_swap_link_writefile_model_matches_an_in_memory_reference_fs_or_names_the_diverging_step(
        ops in proptest::collection::vec(op_strategy(), 1..12)
    ) {
        let root_path = PathBuf::from("/root");
        let fs = FixtureBuilder::new().dir("/root").dir("/journal").build_fs();
        let root = Root::open(&fs, root_path.clone()).expect("open root");
        let journal = FsJournal::new(PathBuf::from("/journal"), Arc::new(fs.clone()));
        let lease = FakeLease::default();
        let g = test_guard(&lease);
        let plan = begin_test_plan(&journal, &g, root_path.clone());

        let mut model: BTreeMap<&str, Vec<u8>> = BTreeMap::new();

        for (i, op) in ops.iter().enumerate() {
            match *op {
                Op::Create { skill, content } => {
                    let name = SKILL_NAMES[skill];
                    let bytes = CONTENTS[content].to_vec();
                    let staged = fsops::stage(&root, &plan, &[(PathBuf::from("SKILL.md"), bytes.clone())])
                        .unwrap_or_else(|e| panic!("step {i} (Create {name:?}): stage failed: {e}"));
                    fsops::swap(&root, &plan, Path::new(name), &staged, Path::new(".trash"))
                        .unwrap_or_else(|e| panic!("step {i} (Create {name:?}): swap failed: {e}"));
                    model.insert(name, bytes);
                }
                Op::Update { skill, content } => {
                    let name = SKILL_NAMES[skill];
                    if !model.contains_key(name) {
                        continue;
                    }
                    let bytes = CONTENTS[content].to_vec();
                    let target = skill_md(&root_path, name);
                    let stamp = read_stamp(&fs, &target)
                        .unwrap_or_else(|e| panic!("step {i} (Update {name:?}): read_stamp failed: {e}"));
                    fsops::write_file(&root, &plan, &PathBuf::from(name).join("SKILL.md"), &bytes, &stamp)
                        .unwrap_or_else(|e| panic!("step {i} (Update {name:?}): write_file failed: {e}"));
                    model.insert(name, bytes);
                }
            }

            for name in SKILL_NAMES {
                let path = skill_md(&root_path, name);
                match model.get(name) {
                    Some(expected) => {
                        let actual = fs.read_capped(&path, u64::MAX).unwrap_or_else(|e| {
                            panic!(
                                "step {i} ({op:?}): expected {name}/SKILL.md to exist after this \
                                 step, but reading it failed: {e}"
                            )
                        });
                        assert!(
                            &actual == expected,
                            "step {i} ({op:?}): {name}/SKILL.md content diverged from the \
                             reference model (got {actual:?}, want {expected:?})"
                        );
                    }
                    None => {
                        assert!(
                            fs.symlink_metadata(&path).is_err(),
                            "step {i} ({op:?}): {name}/SKILL.md exists on the fixture but the \
                             reference model has no entry for {name}"
                        );
                    }
                }
            }
        }
    }
}

/// Given a root with an existing directory at `final_name`, when the
/// directory is replaced by a symlink between an earlier `stage` and the
/// `swap` call (the shape a TOCTOU race between the two steps would take),
/// then `swap` refuses with `ReplacedBySymlink` naming that path, exchanges
/// nothing, and leaves the symlink and the staged folder exactly as they
/// were.
#[test]
fn swap_refuses_a_directory_replaced_by_a_symlink_between_stage_and_swap_or_names_the_unchecked_step(
) {
    let root_path = PathBuf::from("/root");
    let fs = FixtureBuilder::new()
        .dir("/root")
        .dir("/root/gamma")
        .dir("/journal")
        .build_fs();
    let root = Root::open(&fs, root_path.clone()).expect("open root");
    let journal = FsJournal::new(PathBuf::from("/journal"), Arc::new(fs.clone()));
    let lease = FakeLease::default();
    let g = test_guard(&lease);
    let plan = begin_test_plan(&journal, &g, root_path.clone());

    let staged = fsops::stage(
        &root,
        &plan,
        &[(PathBuf::from("SKILL.md"), b"new content".to_vec())],
    )
    .expect("stage");
    let staged_path = staged.path().to_path_buf();

    // Simulate the race: something outside fsops empties and replaces
    // `gamma` with a symlink after `stage` ran but before `swap` does.
    fs.fsops_remove_dir(Path::new("/root/gamma"))
        .expect("remove the now-empty old directory");
    fs.fsops_symlink(Path::new("/elsewhere"), Path::new("/root/gamma"))
        .expect("plant a symlink where the directory used to be");

    let err = fsops::swap(
        &root,
        &plan,
        Path::new("gamma"),
        &staged,
        Path::new(".trash"),
    )
    .expect_err("swap must refuse a target that is no longer a directory");
    match err {
        fsops::FsOpsError::ReplacedBySymlink { path } => {
            assert_eq!(
                path,
                PathBuf::from("/root/gamma"),
                "must name the raced path"
            );
        }
        other => panic!("expected ReplacedBySymlink, got {other}"),
    }

    // Nothing was exchanged: the symlink is untouched, the staged folder
    // is exactly where `stage` left it.
    let facts = fs
        .symlink_metadata(Path::new("/root/gamma"))
        .expect("gamma still exists");
    assert_eq!(facts.kind, skill_studio_core::ports::FileKind::Symlink);
    assert_eq!(
        fs.read_capped(&staged_path.join("SKILL.md"), u64::MAX)
            .expect("staged content untouched"),
        b"new content"
    );
}

/// Given a root with an existing regular file at the link's target name,
/// when `link` is asked to point that name at something else, then it
/// refuses with `WouldReplaceFile` naming the file, and leaves that file's
/// bytes exactly as they were rather than silently losing them under a new
/// symlink.
#[test]
fn link_over_a_regular_file_is_refused_or_names_the_file_it_replaced() {
    let root_path = PathBuf::from("/root");
    let fs = FixtureBuilder::new()
        .dir("/root")
        .file("/root/skill-current", b"not a symlink, a real file")
        .file("/root/new.txt", b"new")
        .dir("/journal")
        .build_fs();
    let root = Root::open(&fs, root_path.clone()).expect("open root");
    let journal = FsJournal::new(PathBuf::from("/journal"), Arc::new(fs.clone()));
    let lease = FakeLease::default();
    let g = test_guard(&lease);
    let plan = begin_test_plan(&journal, &g, root_path.clone());

    let err = fsops::link(
        &root,
        &plan,
        Path::new("skill-current"),
        Path::new("new.txt"),
    )
    .expect_err("link must refuse an existing non-symlink target");
    match err {
        fsops::FsOpsError::WouldReplaceFile { path } => {
            assert_eq!(
                path,
                root_path.join("skill-current"),
                "must name the file it would have replaced"
            );
        }
        other => panic!("expected WouldReplaceFile, got {other}"),
    }

    assert_eq!(
        fs.read_capped(&root_path.join("skill-current"), u64::MAX)
            .expect("the original file must still be there"),
        b"not a symlink, a real file",
        "link must not touch the file it refused to replace"
    );
}

/// Given a root, when a name is confined that would escape it via a `..`
/// segment, an absolute path, or a symlink among its ancestors that points
/// outside, then `confine` (and every primitive built on it) refuses
/// before writing anything, and the escape attempt leaves no trace
/// anywhere on the fixture.
#[test]
fn root_confinement_refuses_a_name_that_escapes_the_root_and_writes_no_bytes_or_names_the_path_that_leaked(
) {
    let root_path = PathBuf::from("/root");
    let fs = FixtureBuilder::new()
        .dir("/root")
        .dir("/outside")
        .alias("/root/escape", "/outside")
        .build_fs();
    let root = Root::open(&fs, root_path).expect("open root");

    let dotdot = Path::new("../outside/leak.txt");
    let err = root.confine(dotdot).expect_err("`..` must be refused");
    assert!(
        matches!(err, fsops::FsOpsError::Escapes { .. }),
        "expected Escapes, got {err}"
    );

    let absolute = Path::new("/outside/leak.txt");
    let err = root
        .confine(absolute)
        .expect_err("an absolute path must be refused");
    assert!(
        matches!(err, fsops::FsOpsError::Escapes { .. }),
        "expected Escapes, got {err}"
    );

    let via_symlink = Path::new("escape/leak.txt");
    let err = root
        .confine(via_symlink)
        .expect_err("a symlink ancestor pointing outside the root must be refused");
    assert!(
        matches!(err, fsops::FsOpsError::Escapes { .. }),
        "expected Escapes, got {err}"
    );

    assert!(
        fs.symlink_metadata(Path::new("/outside/leak.txt")).is_err(),
        "no bytes should have reached /outside/leak.txt"
    );
    assert!(
        fs.read_dir(Path::new("/outside"))
            .expect("read /outside")
            .is_empty(),
        "no bytes should have reached /outside at all"
    );
}

/// Given a root with a two-hop ancestor symlink chain - `inner` (inside the
/// root) points at `mid`, and `mid` points outside the root - when a name
/// under `inner` is confined, then the escape is refused by the second hop,
/// not silently accepted after only the first is checked; a chain whose
/// every hop stays inside the root is accepted instead.
#[test]
fn confine_rejects_a_two_hop_symlink_chain_that_leaves_the_root_or_names_the_accepted_escape() {
    let fs = FixtureBuilder::new()
        .dir("/root")
        .dir("/outside")
        .dir("/root/mid_ok")
        .alias("/root/inner", "/root/mid")
        .alias("/root/mid", "/outside")
        .alias("/root/inner_ok", "/root/mid_ok")
        .build_fs();
    let root = Root::open(&fs, PathBuf::from("/root")).expect("open root");

    let err = root
        .confine(Path::new("inner/file.txt"))
        .expect_err("a chain that leaves the root on its second hop must be refused");
    assert!(
        matches!(err, fsops::FsOpsError::Escapes { .. }),
        "expected Escapes, got {err}"
    );

    let resolved = root
        .confine(Path::new("inner_ok/file.txt"))
        .expect("a chain whose every hop stays inside the root must be accepted");
    assert_eq!(resolved, PathBuf::from("/root/mid_ok/file.txt"));
}

/// Given a root with an existing directory at `final_name`, when creating
/// the quarantine directory fails, then `swap` refuses before its
/// crash-critical exchange runs: `final_name` still shows the old folder
/// and the staged folder still sits at its own (unexchanged) path, not
/// half-committed with the exchange done but the old tree unquarantined.
#[test]
fn swap_prepares_the_quarantine_before_the_exchange_or_names_the_half_committed_swap() {
    let fixture = FixtureBuilder::new()
        .dir("/root")
        .dir("/root/gamma")
        .file("/root/gamma/SKILL.md", b"old content")
        .dir("/journal")
        .build_fs();
    let failing = FailingFs::wrap(Arc::new(fixture.clone()));
    let root = Root::open(&failing, PathBuf::from("/root")).expect("open root");
    let journal = FsJournal::new(PathBuf::from("/journal"), Arc::new(fixture.clone()));
    let lease = FakeLease::default();
    let g = test_guard(&lease);
    let plan = begin_test_plan(&journal, &g, PathBuf::from("/root"));

    let staged = fsops::stage(
        &root,
        &plan,
        &[(PathBuf::from("SKILL.md"), b"new content".to_vec())],
    )
    .expect("stage");
    let staged_path = staged.path().to_path_buf();

    failing.fail_next_create_dir();
    let err = fsops::swap(
        &root,
        &plan,
        Path::new("gamma"),
        &staged,
        Path::new(".trash"),
    )
    .expect_err("swap must refuse when the quarantine directory fails to create");
    assert!(
        matches!(err, fsops::FsOpsError::Io { .. }),
        "expected Io, got {err}"
    );

    assert_eq!(
        fixture
            .read_capped(Path::new("/root/gamma/SKILL.md"), u64::MAX)
            .expect("gamma must still hold its original content"),
        b"old content",
        "the exchange must not have run before the quarantine directory was ready"
    );
    assert_eq!(
        fixture
            .read_capped(&staged_path.join("SKILL.md"), u64::MAX)
            .expect("the staged folder must still sit at its own path"),
        b"new content",
        "the new content must not have been exchanged into gamma yet"
    );
}

/// Given a root with an existing directory at `final_name` and a
/// `quarantine_dir` that is a symlink pointing outside the root, when
/// `swap` runs, then it refuses before moving the exchanged-out old tree
/// anywhere, rather than following the symlink and moving the old tree
/// outside the root.
#[test]
fn swap_refuses_a_quarantine_dir_that_is_a_symlink_out_of_the_root_or_names_the_moved_tree() {
    let fs = FixtureBuilder::new()
        .dir("/root")
        .dir("/root/gamma")
        .file("/root/gamma/SKILL.md", b"old content")
        .dir("/outside")
        .alias("/root/.trash", "/outside")
        .dir("/journal")
        .build_fs();
    let root = Root::open(&fs, PathBuf::from("/root")).expect("open root");
    let journal = FsJournal::new(PathBuf::from("/journal"), Arc::new(fs.clone()));
    let lease = FakeLease::default();
    let g = test_guard(&lease);
    let plan = begin_test_plan(&journal, &g, PathBuf::from("/root"));

    let staged = fsops::stage(
        &root,
        &plan,
        &[(PathBuf::from("SKILL.md"), b"new content".to_vec())],
    )
    .expect("stage");
    let staged_path = staged.path().to_path_buf();

    let err = fsops::swap(
        &root,
        &plan,
        Path::new("gamma"),
        &staged,
        Path::new(".trash"),
    )
    .expect_err("swap must refuse a quarantine dir that is a symlink out of the root");
    assert!(
        matches!(err, fsops::FsOpsError::ReplacedBySymlink { .. }),
        "expected ReplacedBySymlink, got {err}"
    );

    assert!(
        fs.read_dir(Path::new("/outside"))
            .expect("read /outside")
            .is_empty(),
        "the exchanged-out old tree must not have been moved outside the root"
    );
    assert_eq!(
        fs.read_capped(&staged_path.join("SKILL.md"), u64::MAX)
            .expect("the staged folder must still sit at its own path"),
        b"new content",
        "swap must not have exchanged before refusing the quarantine dir"
    );
}

/// Given a caller that read a file's stamp, then the file changes
/// underneath it before the caller's `write_file` call, when `write_file`
/// runs with the stale stamp, then it refuses with `StaleRead` naming the
/// file and leaves the file holding the content the concurrent writer put
/// there, not the caller's bytes and not a mix of both.
#[test]
fn writefile_refuses_a_stale_read_and_leaves_the_original_content_or_names_the_overwritten_file() {
    let root_path = PathBuf::from("/root");
    let fs = FixtureBuilder::new()
        .dir("/root")
        .file("/root/SKILL.md", b"original")
        .dir("/journal")
        .build_fs();
    let root = Root::open(&fs, root_path.clone()).expect("open root");
    let journal = FsJournal::new(PathBuf::from("/journal"), Arc::new(fs.clone()));
    let lease = FakeLease::default();
    let g = test_guard(&lease);
    let plan = begin_test_plan(&journal, &g, root_path.clone());

    let target = root_path.join("SKILL.md");
    let stamp = read_stamp(&fs, &target).expect("read the stamp before the race");

    // A concurrent writer changes the file after the caller's read.
    let racer_stamp = read_stamp(&fs, &target).expect("racer read");
    fsops::write_file(
        &root,
        &plan,
        Path::new("SKILL.md"),
        b"raced in first",
        &racer_stamp,
    )
    .expect("the concurrent writer's own write succeeds");

    let err = fsops::write_file(
        &root,
        &plan,
        Path::new("SKILL.md"),
        b"caller's bytes",
        &stamp,
    )
    .expect_err("write_file must refuse the stale stamp");
    match err {
        fsops::FsOpsError::StaleRead { path } => {
            assert_eq!(path, target, "must name the overwritten file");
        }
        other => panic!("expected StaleRead, got {other}"),
    }

    assert_eq!(
        fs.read_capped(&target, u64::MAX)
            .expect("read final content"),
        b"raced in first",
        "the file must hold the concurrent writer's content, not the caller's stale write"
    );
}
