//! Blocking worker IO with short socket waits and an absolute control deadline.
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

pub(crate) struct WorkerSocket<'a, C> {
    socket: &'a UnixStream,
    cancelled: C,
    deadline: Instant,
}
impl<'a, C: Fn() -> bool> WorkerSocket<'a, C> {
    pub(crate) fn new(socket: &'a UnixStream, cancelled: C, deadline: Instant) -> io::Result<Self> {
        socket.set_read_timeout(Some(Duration::from_millis(10)))?;
        socket.set_write_timeout(Some(Duration::from_millis(10)))?;
        Ok(Self {
            socket,
            cancelled,
            deadline,
        })
    }
    fn check(&self) -> io::Result<()> {
        if (self.cancelled)() || Instant::now() >= self.deadline {
            return Err(io::Error::other("worker exchange stopped"));
        }
        Ok(())
    }
}
fn retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}
impl<C: Fn() -> bool> Read for WorkerSocket<'_, C> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.socket.read(bytes) {
                Err(error) if retryable(&error) => continue,
                result => return result,
            }
        }
    }
}
impl<C: Fn() -> bool> Write for WorkerSocket<'_, C> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.socket.write(bytes) {
                Err(error) if retryable(&error) => continue,
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_coordination::CancellationToken;
    #[test]
    fn silent_peer_is_stopped_by_deadline() {
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut controlled = WorkerSocket::new(
            &socket,
            || false,
            Instant::now() + Duration::from_millis(20),
        )
        .unwrap();
        assert_eq!(
            controlled.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::Other
        );
    }
    #[test]
    fn cancellation_interrupts_a_silent_read() {
        let (socket, _peer) = UnixStream::pair().unwrap();
        let cancellation = CancellationToken::default();
        let signal = cancellation.clone();
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            signal.cancel();
        });
        let mut controlled = WorkerSocket::new(
            &socket,
            || cancellation.is_cancelled(),
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(
            controlled.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::Other
        );
        assert!(cancellation.is_cancelled());
        sender.join().unwrap();
    }
}
