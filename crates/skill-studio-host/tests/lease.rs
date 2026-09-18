//! Guards unit 1.3: `FileLease` acquires one lease per root, across real OS
//! processes, not just threads in one. `two_writers_to_different_roots...`
//! stays in-process; the other two re-enter this same test binary as a
//! second process so the busy/takeover checks exercise real pids, not a
//! thread id this process could fake.

// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too. `print_stdout`/`print_stderr`
// are the IPC channel the child holder process uses to signal readiness to
// the parent, not debug output.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use skill_studio_core::error::ErrorCode;
use skill_studio_core::ports::{LeaseKey, LeaseMode, LeaseProvider};
use skill_studio_host::FileLease;

/// Set only on the re-entered child process; carries `<lease_root>\x01<root>`.
const CHILD_ENV: &str = "SKILL_STUDIO_LEASE_TEST_CHILD";

fn key(root: &Path) -> LeaseKey {
    LeaseKey {
        canonical_root: root.to_path_buf(),
    }
}

/// Not a real test: when `CHILD_ENV` is unset (every normal run) this is a
/// no-op pass. The parent tests below re-invoke the test binary filtered to
/// exactly this name with `CHILD_ENV` set, so it instead acquires the
/// lease, announces readiness on stdout, and blocks on stdin until the
/// parent lets it go - a real second process holding a real lease.
#[test]
fn lease_test_child_worker() {
    let Ok(payload) = std::env::var(CHILD_ENV) else {
        return;
    };
    let (lease_root, root) = payload
        .split_once('\u{1}')
        .expect("child payload is lease_root\\x01root");
    let lease = FileLease::new(PathBuf::from(lease_root));
    let keys = [key(Path::new(root))];
    let _handle = lease
        .acquire(&keys, LeaseMode::Exclusive, Duration::from_secs(5))
        .expect("child failed to acquire the lease it was asked to hold");

    println!("held");
    std::io::stdout().flush().expect("flush stdout");

    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .expect("read stdin until the parent closes it");
}

/// Spawns a real second process that acquires an exclusive lease on `root`
/// and blocks holding it. Returns once the child has confirmed the lease is
/// held, so the caller never races the child's acquire.
fn spawn_holder(lease_root: &Path, root: &Path) -> Child {
    let exe = std::env::current_exe().expect("current_exe");
    let mut child = Command::new(exe)
        .arg("lease_test_child_worker")
        .arg("--exact")
        .arg("--nocapture")
        .env(
            CHILD_ENV,
            format!("{}\u{1}{}", lease_root.display(), root.display()),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn child holder process");

    // libtest's own "running 1 test" header lands on stdout before our
    // "held" line, so scan past it instead of assuming the first line.
    let stdout = child.stdout.take().expect("child stdout");
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .expect("read the child's ready line");
        assert_ne!(n, 0, "child exited before confirming it holds the lease");
        if line.trim() == "held" {
            break;
        }
    }
    // libtest still writes its own footer lines after ours; keep draining
    // them on a background thread so the child never sees a broken pipe.
    std::thread::spawn(move || {
        let mut sink = String::new();
        while reader.read_line(&mut sink).unwrap_or(0) > 0 {
            sink.clear();
        }
    });
    child
}

#[test]
fn a_second_process_holding_the_lease_gets_busy_with_the_first_holders_pid_and_age_or_names_the_field_missing_from_the_error(
) {
    let dir = tempfile::tempdir().unwrap();
    let lease_root = dir.path().join("leases");
    let root = dir.path().join("root-a");
    std::fs::create_dir_all(&root).unwrap();

    let mut child = spawn_holder(&lease_root, &root);
    let child_pid = child.id();

    // The child's "held" line lands right after it writes the lease's
    // holder record; a short pause keeps the age strictly positive without
    // asserting anything about its size.
    std::thread::sleep(Duration::from_millis(20));

    let lease = FileLease::new(lease_root);
    let Err(err) = lease.acquire(&[key(&root)], LeaseMode::Exclusive, Duration::ZERO) else {
        panic!("a held lease must refuse a second exclusive acquire")
    };
    assert_eq!(err.code, ErrorCode::ScopeBusy);
    let busy = err
        .busy
        .expect("ScopeBusy from a live holder must carry LeaseBusy");
    assert_eq!(busy.pid, child_pid, "Busy named the wrong holder pid");
    assert!(
        busy.age > Duration::ZERO,
        "Busy reported a zero age for a lease the child has held"
    );

    drop(child.stdin.take());
    child.wait().expect("child exits once stdin closes");
}

#[test]
fn a_lease_whose_pid_is_gone_is_taken_over_by_the_next_writer_or_names_the_write_that_still_refuses(
) {
    let dir = tempfile::tempdir().unwrap();
    let lease_root = dir.path().join("leases");
    let root = dir.path().join("root-b");
    std::fs::create_dir_all(&root).unwrap();

    let mut child = spawn_holder(&lease_root, &root);
    child.kill().expect("kill the holder");
    child.wait().expect("reap the killed holder");

    let lease = FileLease::new(lease_root);
    lease
        .acquire(
            &[key(&root)],
            LeaseMode::Exclusive,
            Duration::from_millis(200),
        )
        .expect("a lease whose recorded pid is gone must be taken over");
}

#[test]
fn two_writers_to_different_roots_never_block_each_other_or_names_the_root_that_serialized_them() {
    let dir = tempfile::tempdir().unwrap();
    let lease_root = dir.path().join("leases");
    let root_c = dir.path().join("root-c");
    let root_d = dir.path().join("root-d");
    std::fs::create_dir_all(&root_c).unwrap();
    std::fs::create_dir_all(&root_d).unwrap();

    let lease = FileLease::new(lease_root);
    let first = lease
        .acquire(&[key(&root_c)], LeaseMode::Exclusive, Duration::ZERO)
        .expect("root-c must acquire uncontended");
    let second = lease
        .acquire(&[key(&root_d)], LeaseMode::Exclusive, Duration::ZERO)
        .expect("root-d must acquire uncontended; root-c serialized it");
    assert_eq!(first.mode(), LeaseMode::Exclusive);
    assert_eq!(second.mode(), LeaseMode::Exclusive);
}
