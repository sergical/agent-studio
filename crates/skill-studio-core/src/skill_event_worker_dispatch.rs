//! Send-attempt tracking; a complete send is not a commit receipt.
use crate::skill_coordination::CancellationToken;
use crate::skill_history_worker_frame::{write_json_frame, HistoryFrameError};
use std::io::{self, Write};

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) enum DispatchState {
    #[default]
    NotSent,
    MayHaveSent,
}

struct TrackedOutput<'a, W> {
    output: &'a mut W,
    state: &'a mut DispatchState,
    cancellation: &'a CancellationToken,
}
impl<W: Write> Write for TrackedOutput<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.cancellation.is_cancelled() {
            return Err(io::Error::other("event dispatch cancelled"));
        }
        if !bytes.is_empty() {
            *self.state = DispatchState::MayHaveSent;
        }
        self.output.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

#[cfg(test)]
pub(crate) fn send_frame(
    output: &mut impl Write,
    state: &mut DispatchState,
    limit: usize,
    request: &impl serde::Serialize,
) -> Result<(), HistoryFrameError> {
    send_frame_controlled(output, state, limit, request, &CancellationToken::default())
}

pub(crate) fn send_frame_controlled(
    output: &mut impl Write,
    state: &mut DispatchState,
    limit: usize,
    request: &impl serde::Serialize,
    cancellation: &CancellationToken,
) -> Result<(), HistoryFrameError> {
    if cancellation.is_cancelled() {
        return Err(HistoryFrameError::Io(io::Error::other(
            "event dispatch cancelled",
        )));
    }
    write_json_frame(
        &mut TrackedOutput {
            output,
            state,
            cancellation,
        },
        limit,
        request,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenOutput {
        bytes: Vec<u8>,
        remaining: usize,
        panic: bool,
    }
    impl Write for BrokenOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            assert!(!self.panic, "injected send panic");
            if self.remaining == 0 {
                return Err(io::Error::other("injected send failure"));
            }
            let count = self.remaining.min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..count]);
            self.remaining -= count;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn encoding_refusal_leaves_request_unsent() {
        let mut state = DispatchState::NotSent;
        let mut output = Vec::new();
        assert!(matches!(
            send_frame(&mut output, &mut state, 4, &"long request"),
            Err(HistoryFrameError::TooLarge)
        ));
        assert_eq!(state, DispatchState::NotSent);
        assert!(output.is_empty());
    }
    #[test]
    fn first_write_failure_and_partial_prefix_are_uncertain() {
        for remaining in [0, 2, 5] {
            let mut output = BrokenOutput {
                bytes: Vec::new(),
                remaining,
                panic: false,
            };
            let mut state = DispatchState::NotSent;
            assert!(send_frame(&mut output, &mut state, 1024, &"request").is_err());
            assert_eq!(state, DispatchState::MayHaveSent);
            assert_eq!(output.bytes.len(), remaining);
        }
    }
    #[test]
    fn send_panic_preserves_attempt_state_outside_unwind() {
        let mut output = BrokenOutput {
            bytes: Vec::new(),
            remaining: 0,
            panic: true,
        };
        let mut state = DispatchState::NotSent;
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| send_frame(
                &mut output,
                &mut state,
                1024,
                &"request"
            )))
            .is_err()
        );
        assert_eq!(state, DispatchState::MayHaveSent);
    }
    #[test]
    fn complete_send_stays_uncertain_and_later_refusal_does_not_reset_it() {
        let mut output = Vec::new();
        let mut state = DispatchState::NotSent;
        send_frame(&mut output, &mut state, 1024, &"request").unwrap();
        assert_eq!(state, DispatchState::MayHaveSent);
        let sent = output.clone();
        assert!(send_frame(&mut output, &mut state, 1, &"request").is_err());
        assert_eq!(state, DispatchState::MayHaveSent);
        assert_eq!(output, sent);
    }
}

#[test]
fn cancellation_before_send_emits_no_frame() {
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let mut state = DispatchState::NotSent;
    let mut output = Vec::new();
    assert!(
        send_frame_controlled(&mut output, &mut state, 1024, &"request", &cancellation).is_err()
    );
    assert_eq!(state, DispatchState::NotSent);
    assert!(output.is_empty());
}

#[test]
fn cancellation_during_encoding_is_checked_before_first_write() {
    struct CancelOnEncode(CancellationToken);
    impl serde::Serialize for CancelOnEncode {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.0.cancel();
            serializer.serialize_str("request")
        }
    }
    let cancellation = CancellationToken::default();
    let mut state = DispatchState::NotSent;
    let mut output = Vec::new();
    assert!(send_frame_controlled(
        &mut output,
        &mut state,
        1024,
        &CancelOnEncode(cancellation.clone()),
        &cancellation
    )
    .is_err());
    assert_eq!(state, DispatchState::NotSent);
    assert!(output.is_empty());
}

#[test]
fn cancellation_after_partial_socket_write_preserves_uncertainty() {
    use std::{io::Read, os::unix::net::UnixStream};
    struct CancelAfterPrefix {
        socket: UnixStream,
        cancellation: CancellationToken,
    }
    impl Write for CancelAfterPrefix {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let written = self.socket.write(&bytes[..bytes.len().min(2)])?;
            self.cancellation.cancel();
            Ok(written)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.socket.flush()
        }
    }
    let cancellation = CancellationToken::default();
    let (socket, mut receiver) = UnixStream::pair().unwrap();
    let mut output = CancelAfterPrefix {
        socket,
        cancellation: cancellation.clone(),
    };
    let mut state = DispatchState::NotSent;
    let error = send_frame_controlled(&mut output, &mut state, 1024, &"request", &cancellation)
        .unwrap_err();
    assert!(
        matches!(error, HistoryFrameError::Io(ref error) if error.kind() != io::ErrorKind::Interrupted)
    );
    assert_eq!(state, DispatchState::MayHaveSent);
    let mut prefix = [255; 2];
    receiver.read_exact(&mut prefix).unwrap();
    assert_eq!(prefix, [0, 0]);
    receiver.set_nonblocking(true).unwrap();
    assert_eq!(
        receiver.read(&mut prefix).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}
