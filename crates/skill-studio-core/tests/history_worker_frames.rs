#![cfg(unix)]

use skill_studio_core::skill_history_worker_frame::{
    finish_history_frames, read_history_frame, write_history_frame, HistoryFrameError,
    HistoryFrameKind,
};
use std::{
    io::{self, Write},
    os::unix::net::{UnixListener, UnixStream},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn history_frame_child() {
    let Some(path) = std::env::var_os("SKILL_STUDIO_FRAME_SOCKET") else {
        return;
    };
    let mut stream = UnixStream::connect(path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let request: String = read_history_frame(&mut stream, HistoryFrameKind::Request).unwrap();
    assert_eq!(request, "fixture-request");
    match std::env::var("SKILL_STUDIO_FRAME_CASE").unwrap().as_str() {
        "valid" => {
            write_history_frame(&mut stream, HistoryFrameKind::Reply, &"fixture-reply").unwrap()
        }
        "truncated" => stream.write_all(&[0, 0, 0, 10, b'"']).unwrap(),
        "oversized" => stream.write_all(&u32::MAX.to_be_bytes()).unwrap(),
        "extra" => {
            write_history_frame(&mut stream, HistoryFrameKind::Reply, &"fixture-reply").unwrap();
            stream.write_all(&[0]).unwrap();
        }
        _ => panic!("unknown fixture case"),
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn real_child_frames_require_complete_single_reply() {
    for case in ["valid", "truncated", "oversized", "extra"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("worker.sock");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut child = OwnedChild(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "history_frame_child"])
                .env("SKILL_STUDIO_FRAME_SOCKET", &path)
                .env("SKILL_STUDIO_FRAME_CASE", case)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "child did not connect: {case}");
                    assert!(
                        child.0.try_wait().unwrap().is_none(),
                        "child exited before connecting: {case}"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write_history_frame(&mut stream, HistoryFrameKind::Request, &"fixture-request").unwrap();
        let reply: Result<String, _> = read_history_frame(&mut stream, HistoryFrameKind::Reply);
        match case {
            "valid" => {
                assert_eq!(reply.unwrap(), "fixture-reply");
                finish_history_frames(&mut stream).unwrap();
            }
            "truncated" => assert!(
                matches!(reply, Err(HistoryFrameError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof)
            ),
            "oversized" => assert!(matches!(reply, Err(HistoryFrameError::TooLarge))),
            "extra" => {
                assert_eq!(reply.unwrap(), "fixture-reply");
                assert!(matches!(
                    finish_history_frames(&mut stream),
                    Err(HistoryFrameError::TrailingData)
                ));
            }
            _ => unreachable!(),
        }
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success(), "child failed: {case}");
                break;
            }
            assert!(Instant::now() < deadline, "child did not exit: {case}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
