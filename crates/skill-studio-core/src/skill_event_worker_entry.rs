//! Shared private stdio bootstrap for executable hosts.
use std::process::ExitCode;

/// # Safety
/// Call only from the private worker branch before logging, application setup,
/// or any SQLite use. The process must exit immediately after this returns.
pub unsafe fn run_event_worker_stdio() -> ExitCode {
    use crate::{
        skill_event_native::serve_event_database,
        skill_history_worker_bootstrap::receive_history_directory_or_exit,
    };
    use std::{
        io::{Read, Write},
        os::{fd::AsFd, unix::net::UnixStream},
    };
    let descriptor = match std::io::stdin().as_fd().try_clone_to_owned() {
        Ok(descriptor) => descriptor,
        Err(_) => return ExitCode::from(74),
    };
    let mut socket = UnixStream::from(descriptor);
    let directory = receive_history_directory_or_exit(&socket);
    let handshake = (|| -> std::io::Result<()> {
        socket.write_all(b"R")?;
        let mut proceed = [0];
        socket.read_exact(&mut proceed)?;
        if proceed != [1] {
            return Err(std::io::Error::other("Invalid event proceed signal"));
        }
        Ok(())
    })();
    if handshake.is_err() {
        return ExitCode::from(74);
    }
    // This entry runs before logging, signal handlers, runtimes or SQLite.
    // The owning parent enforces the deadline; main exits after this call.
    match unsafe { serve_event_database(socket, directory) } {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(74),
    }
}
