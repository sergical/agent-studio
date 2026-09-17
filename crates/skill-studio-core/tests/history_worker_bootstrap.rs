#![cfg(any(target_os = "macos", target_os = "linux"))]
use skill_studio_core::{
    skill_history_state::HistoryStateRoot,
    skill_history_worker_bootstrap::{
        receive_history_directory_or_exit, send_history_directory, HISTORY_BOOTSTRAP_FAILURE_EXIT,
    },
    skill_history_worker_process::{HistoryProcessError, HistoryWorkerProcess},
};
use std::{
    io::{Read, Write},
    mem,
    os::{
        fd::{AsFd, AsRawFd},
        unix::net::UnixStream,
    },
    process::Command,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

#[test]
fn history_bootstrap_child() {
    if std::env::var_os("SKILL_STUDIO_BOOTSTRAP_CHILD").is_none() {
        return;
    }
    let mut socket = UnixStream::from(std::io::stdin().as_fd().try_clone_to_owned().unwrap());
    let directory = receive_history_directory_or_exit(&socket);
    // F_GETFD observes the received directory descriptor without changing it.
    assert_ne!(
        unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    socket.write_all(b"ready").unwrap();
    let mut trigger = [0];
    socket.read_exact(&mut trigger).unwrap();
    assert_eq!(&trigger, b"r");
    assert_eq!(directory.read_to_string("marker").unwrap(), "original");
    socket.write_all(b"original").unwrap();
}
fn worker() -> HistoryWorkerProcess {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "history_bootstrap_child"])
        .env("SKILL_STUDIO_BOOTSTRAP_CHILD", "1");
    let worker = HistoryWorkerProcess::spawn(&mut command).unwrap();
    worker
        .socket()
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    worker
}
fn reaped(pid: u32) {
    let mut status = 0;
    // Observe the fixture's known child, without signalling another process.
    assert_eq!(
        unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}
fn send_fixture(socket: &UnixStream, descriptor: libc::c_int, count: usize, marker: u8) {
    // Test-only construction permits counts the production sender cannot express.
    // Word-aligned storage is rounded up to fit the native ancillary length.
    let space = unsafe { libc::CMSG_SPACE((count * mem::size_of::<libc::c_int>()) as _) as usize };
    let mut storage = vec![0_usize; space.div_ceil(mem::size_of::<usize>())];
    let mut byte = marker;
    let mut vector = libc::iovec {
        iov_base: (&mut byte as *mut u8).cast(),
        iov_len: 1,
    };
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    if count > 0 {
        message.msg_control = storage.as_mut_ptr().cast();
        message.msg_controllen = space as _;
        // Every header and descriptor write stays within the allocated control space.
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN((count * mem::size_of::<libc::c_int>()) as _) as _;
            for index in 0..count {
                libc::CMSG_DATA(header)
                    .cast::<libc::c_int>()
                    .add(index)
                    .write_unaligned(descriptor);
            }
        }
    }
    // All pointers refer to live storage during this one-byte send.
    assert_eq!(unsafe { libc::sendmsg(socket.as_raw_fd(), &message, 0) }, 1);
}
#[test]
fn transferred_directory_survives_replacement_and_parent_refuses_changed_root() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("marker"), "original").unwrap();
    let root = HistoryStateRoot::bind(&path).unwrap();
    let transfer = root.prepare_directory_transfer().unwrap();
    let mut child = worker();
    send_history_directory(child.socket(), &transfer).unwrap();
    let mut ready = [0; 5];
    child.socket().read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"ready");
    std::fs::rename(&path, temp.path().join("old")).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("marker"), "replacement").unwrap();
    assert!(transfer.revalidate().is_err());
    assert!(send_history_directory(child.socket(), &transfer).is_err());
    child.socket().write_all(b"r").unwrap();
    let mut result = [0; 8];
    child.socket().read_exact(&mut result).unwrap();
    assert_eq!(&result, b"original");
    child
        .wait_for_exit(
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(3),
        )
        .unwrap();
    reaped(child.id());
}
#[test]
fn rejected_transfers_exit_and_close_even_unreported_handles() {
    for (count, marker) in [
        (0, b'H'),
        (1, 0),
        (1, b'H'),
        (2, b'H'),
        (4, b'H'),
        (16, b'H'),
        (64, b'H'),
        (128, b'H'),
    ] {
        let mut child = worker();
        // Created after spawn: the child can acquire this endpoint only by SCM_RIGHTS.
        let (handle, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        send_fixture(child.socket(), handle.as_raw_fd(), count, marker);
        drop(handle);
        let result = child.wait_for_exit(
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(3),
        );
        assert!(
            matches!(result,Err(HistoryProcessError::FailedExit(status)) if status.code()==Some(HISTORY_BOOTSTRAP_FAILURE_EXIT)),
            "count={count}"
        );
        reaped(child.id());
        assert_eq!(peer.read(&mut [0]).unwrap(), 0, "leaked handles: {count}");
    }
    let mut child = worker();
    let file = tempfile::tempfile().unwrap();
    send_fixture(child.socket(), file.as_raw_fd(), 1, b'H');
    assert!(
        matches!(child.wait_for_exit(&AtomicBool::new(false),Instant::now()+Duration::from_secs(3)),Err(HistoryProcessError::FailedExit(status)) if status.code()==Some(HISTORY_BOOTSTRAP_FAILURE_EXIT))
    );
    reaped(child.id());
}
