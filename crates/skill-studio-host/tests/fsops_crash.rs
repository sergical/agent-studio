//! Unit 1.1's crash test: kills a real child process partway through
//! `fsops::stage`/`swap` on real disk and checks what's left behind. Lives
//! here, not in `skill-studio-core`, because it needs `RealFs` and a real
//! temp directory - the in-memory model test and the other three fixed-name
//! tests for this unit are in `skill-studio-core/tests/fsops.rs`.
//!
//! The child re-enters this same test binary (`std::env::current_exe()`),
//! filtered to this one test by name, with env vars telling it which step
//! to abort at. The env vars being unset is how the parent process (a plain
//! `cargo test` invocation) tells its own copy of this function to run the
//! normal, non-crashing test body instead.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use skill_studio_core::fsops::{self, Root};
use skill_studio_core::identity::PlanId;
use skill_studio_core::journal::{FsJournal, PlanWriter};
use skill_studio_core::ports::{ExclusiveGuard, LeaseMode, LeaseProvider, ScopeFs};
use skill_studio_host::{FileLease, RealFs};

const STEP_ENV: &str = "FSOPS_CRASH_TEST_STEP";
const DIR_ENV: &str = "FSOPS_CRASH_TEST_DIR";
const CONTENT_ENV: &str = "FSOPS_CRASH_TEST_CONTENT";

const TEST_NAME: &str = "a_crash_after_any_step_leaves_the_disk_before_the_change_or_after_it_never_between_or_names_the_half_done_step";

/// Every call in this file now needs a plan writer (unit 1.2 Section B
/// moved journal recording inside `fsops`'s primitives). Each caller -
/// the parent establishing the baseline, and each re-exec'd crash child -
/// begins its own plan against a `FsJournal` rooted at `<root>/.journal`,
/// on `RealFs`, guarded by a real `FileLease` (not `skill-studio-core`'s
/// in-memory `FakeLease` test double, since this crate doesn't depend on
/// `skill-studio-core`'s `testing` feature). Reconciling those plans isn't
/// what this test is about; it only cares what the crash left on disk.
fn begin_plan<'a>(
    journal: &'a FsJournal,
    guard: &'a ExclusiveGuard,
    root_path: PathBuf,
    id: &str,
) -> PlanWriter<'a> {
    PlanWriter::begin(
        journal,
        guard,
        PlanId(id.to_string()),
        Utc::now(),
        "fsops crash test",
        root_path,
        Vec::new(),
    )
    .expect("begin plan")
}

fn open_journal_and_guard(root_path: &Path) -> (FsJournal, FileLease, PathBuf) {
    let journal_dir = root_path.join(".journal");
    let lease_dir = root_path.join(".lease");
    std::fs::create_dir_all(&lease_dir).expect("create lease dir");
    let fs: Arc<dyn ScopeFs> = Arc::new(RealFs::new());
    let journal = FsJournal::new(journal_dir, fs);
    let lease = FileLease::new(lease_dir);
    (journal, lease, root_path.to_path_buf())
}

fn acquire_guard(lease: &FileLease) -> ExclusiveGuard {
    let handle = lease
        .acquire(&[], LeaseMode::Exclusive, Duration::from_secs(5))
        .expect("acquire exclusive lease");
    ExclusiveGuard::from_handle(handle)
}

/// Step 0: abort before touching disk at all. Step 1: abort right after
/// `stage` (the new content sits under a temp name; `skill` is untouched).
/// Step 2: abort right after `swap` (`skill` now shows the new content).
fn run_as_crash_child_if_env_set() {
    let (Ok(step), Ok(dir), Ok(content)) = (
        std::env::var(STEP_ENV),
        std::env::var(DIR_ENV),
        std::env::var(CONTENT_ENV),
    ) else {
        return;
    };
    let step: u32 = step
        .parse()
        .expect("FSOPS_CRASH_TEST_STEP must be a number");
    let root_path = PathBuf::from(dir);
    let fs = RealFs::new();
    let root = Root::open(&fs, root_path.clone()).expect("open root");
    let (journal, lease, root_path) = open_journal_and_guard(&root_path);
    let g = acquire_guard(&lease);
    let plan = begin_plan(&journal, &g, root_path, "01PLANFSOPSCRASHCHILD00001");

    if step == 0 {
        std::process::abort();
    }
    let staged = fsops::stage(
        &root,
        &plan,
        &[(PathBuf::from("SKILL.md"), content.into_bytes())],
    )
    .expect("stage");
    if step == 1 {
        std::process::abort();
    }
    fsops::swap(
        &root,
        &plan,
        Path::new("skill"),
        staged,
        Path::new(".trash"),
    )
    .expect("swap");
    if step == 2 {
        std::process::abort();
    }
    // Every step this test exercises aborts; reaching here means the step
    // constant above and this match fell out of sync.
    unreachable!("crash child ran past its highest defined step ({step})");
}

/// Re-execs this test binary, filtered to just this test, with the crash
/// env vars set, and waits for it to die. Asserts it actually crashed
/// (never finished normally, since every defined step aborts).
fn run_crash_child(root: &Path, step: u32, content: &str) {
    let exe = std::env::current_exe().expect("current_exe");
    let status = Command::new(exe)
        .arg(TEST_NAME)
        .arg("--exact")
        .arg("--nocapture")
        .env(STEP_ENV, step.to_string())
        .env(DIR_ENV, root.to_string_lossy().to_string())
        .env(CONTENT_ENV, content)
        .status()
        .expect("spawn crash child");
    assert!(
        !status.success(),
        "step {step}: the crash child must abort, not finish normally"
    );
}

#[test]
fn a_crash_after_any_step_leaves_the_disk_before_the_change_or_after_it_never_between_or_names_the_half_done_step(
) {
    run_as_crash_child_if_env_set();

    let dir = tempfile::tempdir().expect("tempdir");
    let root_path = dir.path().to_path_buf();
    let fs = RealFs::new();
    let root = Root::open(&fs, root_path.clone()).expect("open root");
    let (journal, lease, plan_root) = open_journal_and_guard(&root_path);
    let g = acquire_guard(&lease);
    let plan = begin_plan(&journal, &g, plan_root, "01PLANFSOPSCRASHPARENT0001");

    // Establish a committed baseline directly (no crash): `skill` exists
    // and holds "v1". This exercises the harder crash shape below - a
    // `swap` that exchanges an *existing* directory, not just a plain
    // rename into empty space.
    let staged = fsops::stage(&root, &plan, &[(PathBuf::from("SKILL.md"), b"v1".to_vec())])
        .expect("baseline stage");
    fsops::swap(
        &root,
        &plan,
        Path::new("skill"),
        staged,
        Path::new(".trash"),
    )
    .expect("baseline swap");
    drop(g);
    assert_eq!(
        std::fs::read(root_path.join("skill/SKILL.md")).expect("baseline content"),
        b"v1"
    );

    for step in 0..3u32 {
        run_crash_child(&root_path, step, "v2");

        let skill_md = root_path.join("skill/SKILL.md");
        assert!(
            skill_md.exists(),
            "step {step}: the crash must never remove the baseline `skill` folder entirely - \
             `swap` only ever exchanges or renames into place, it does not delete"
        );
        let content = std::fs::read(&skill_md).unwrap_or_else(|e| {
            panic!("step {step}: skill/SKILL.md exists but is unreadable: {e}")
        });
        assert!(
            content == b"v1" || content == b"v2",
            "step {step}: skill/SKILL.md holds neither the pre-crash content (\"v1\") nor the \
             fully-written new content (\"v2\") - got {content:?}, a half-done state"
        );
    }
}
