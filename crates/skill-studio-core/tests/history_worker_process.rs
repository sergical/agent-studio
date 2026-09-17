#![cfg(unix)]
use skill_studio_core::skill_history_worker_process::{HistoryProcessError, HistoryWorkerProcess};
use std::{
    io::{Read, Write},
    os::{fd::AsFd, unix::net::UnixStream},
    process::Command,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

#[test]
fn history_process_child() {
    let Ok(mode) = std::env::var("SKILL_STUDIO_PROCESS_CHILD") else {
        return;
    };
    let descriptor = std::io::stdin().as_fd().try_clone_to_owned().unwrap();
    let mut socket = UnixStream::from(descriptor);
    socket.write_all(b"ready").unwrap();
    match mode.as_str() {
        "success" => {}
        "failure" => std::process::exit(78),
        "hang" => std::thread::sleep(Duration::from_secs(60)),
        _ => panic!("unknown child fixture"),
    }
}

fn child(mode: &str) -> HistoryWorkerProcess {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "history_process_child"])
        .env("SKILL_STUDIO_PROCESS_CHILD", mode);
    let worker = HistoryWorkerProcess::spawn(&mut command).unwrap();
    worker
        .socket()
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut ready = [0; 5];
    worker.socket().read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"ready");
    worker
}

fn assert_reaped(pid: u32) {
    let mut status = 0;
    // WNOHANG observes only this fixture's child PID; it never signals a process.
    let result = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
    assert_eq!(result, -1, "child was not already reaped");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[test]
fn normal_and_failed_exits_are_reaped_and_distinct() {
    for mode in ["success", "failure"] {
        let mut worker = child(mode);
        let result = worker.wait_for_exit(
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(3),
        );
        if mode == "success" {
            result.unwrap();
        } else {
            assert!(
                matches!(result, Err(HistoryProcessError::FailedExit(status)) if status.code() == Some(78))
            );
        }
        assert_reaped(worker.id());
        worker.terminate_and_reap().unwrap();
        assert_reaped(worker.id());
    }
}

#[test]
fn cancellation_and_deadline_stop_a_confirmed_live_child() {
    for cancel in [true, false] {
        let mut worker = child("hang");
        let started = Instant::now();
        let result = worker.wait_for_exit(
            &AtomicBool::new(cancel),
            started + Duration::from_millis(25),
        );
        assert!(matches!(
            (&result, cancel),
            (Err(HistoryProcessError::Cancelled), true)
                | (Err(HistoryProcessError::DeadlineExceeded), false)
        ));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_reaped(worker.id());
    }
}

#[test]
fn dropping_owner_terminates_and_reaps_live_child() {
    let worker = child("hang");
    let pid = worker.id();
    drop(worker);
    assert_reaped(pid);
}

#[test]
fn cancellation_overrides_successful_exit() {
    let mut worker = child("success");
    // EOF proves the worker closed its socket; cancellation still prevents success.
    assert_eq!(worker.socket().read(&mut [0]).unwrap(), 0);
    let result = worker.wait_for_exit(
        &AtomicBool::new(true),
        Instant::now() + Duration::from_secs(3),
    );
    assert!(matches!(result, Err(HistoryProcessError::Cancelled)));
    assert_reaped(worker.id());
}

#[test]
fn missing_worker_executable_returns_spawn_error() {
    let temp = tempfile::tempdir().unwrap();
    let mut command = Command::new(temp.path().join("missing-worker"));
    assert!(
        matches!(HistoryWorkerProcess::spawn(&mut command), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    );
}
