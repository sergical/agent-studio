//! Ownership and cleanup for a one-shot history child. These operations can
//! block and belong on a supervisor thread, never the UI or async executor thread.
//! Framed IO, admission, and publication checks remain adapter responsibilities.
use std::{
    io,
    net::Shutdown,
    os::{fd::OwnedFd, unix::net::UnixStream},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub enum HistoryProcessError {
    Cancelled,
    DeadlineExceeded,
    FailedExit(ExitStatus),
    Io(io::Error),
}
impl From<io::Error> for HistoryProcessError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct HistoryWorkerProcess {
    child: Child,
    socket: UnixStream,
}
impl HistoryWorkerProcess {
    /// The adapter supplies its trusted executable and private worker arguments.
    /// The socket is inherited as stdin; stdout and stderr cannot fill pipes.
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        let (socket, child_socket) = UnixStream::pair()?;
        let descriptor: OwnedFd = child_socket.into();
        let child = command
            .stdin(Stdio::from(descriptor))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        // Command retains its configured Stdio, so clear the parent's extra copy.
        command.stdin(Stdio::null());
        Ok(Self {
            child: child?,
            socket,
        })
    }

    pub fn socket(&self) -> &UnixStream {
        &self.socket
    }
    pub(crate) fn observe_exit(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Check cancellation before accepting even an already completed child.
    /// This deadline controls exit waiting; the adapter must also supervise IO.
    pub fn wait_for_exit(
        &mut self,
        cancellation: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), HistoryProcessError> {
        loop {
            let failure = if cancellation.load(Ordering::Acquire) {
                Some(HistoryProcessError::Cancelled)
            } else if Instant::now() >= deadline {
                Some(HistoryProcessError::DeadlineExceeded)
            } else {
                None
            };
            if let Some(failure) = failure {
                self.terminate_and_reap()?;
                return Err(failure);
            }
            if let Some(status) = self.observe_exit()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(HistoryProcessError::FailedExit(status))
                };
            }
            std::thread::sleep(
                Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }

    /// Repeated calls are safe after reaping; Child caches the exit status.
    pub fn terminate_and_reap(&mut self) -> io::Result<ExitStatus> {
        let _ = self.socket.shutdown(Shutdown::Both);
        if let Some(status) = self.observe_exit()? {
            return Ok(status);
        }
        self.child.kill()?;
        self.child.wait()
    }
}
impl Drop for HistoryWorkerProcess {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
