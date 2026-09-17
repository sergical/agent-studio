//! Bootstrap for a dedicated one-shot worker on macOS/Linux. The receiver exits
//! the process on malformed input; it must never run in the desktop or MCP host.
use crate::skill_history_state::HistoryDirectoryTransfer;
use std::{
    fs::File,
    io, mem,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
};

const MARKER: u8 = b'H';
pub const HISTORY_BOOTSTRAP_FAILURE_EXIT: i32 = 78;
#[repr(C)]
struct SingleRight {
    header: libc::cmsghdr,
    descriptor: libc::c_int,
}

fn lengths() -> (usize, usize) {
    // The argument is a fixed c_int size, so neither native length can overflow.
    unsafe {
        (
            libc::CMSG_LEN(mem::size_of::<libc::c_int>() as _) as usize,
            libc::CMSG_SPACE(mem::size_of::<libc::c_int>() as _) as usize,
        )
    }
}
fn fail() -> ! {
    // Process exit closes even handles omitted from truncated macOS control data.
    unsafe { libc::_exit(HISTORY_BOOTSTRAP_FAILURE_EXIT) }
}

/// Send one descriptor and marker without blocking. Any send error invalidates
/// this exchange; the parent must terminate/reap rather than retry the prelude.
pub fn send_history_directory(
    socket: &UnixStream,
    directory: &HistoryDirectoryTransfer<'_>,
) -> io::Result<()> {
    use std::os::fd::AsFd;
    directory
        .revalidate()
        .map_err(|_| io::Error::other("history root changed"))?;
    send_directory_descriptor(socket, directory.as_fd())
}

/// Transport only: callers must authorize and validate the retained directory.
/// After any send error, terminate/reap the recipient instead of retrying.
pub(crate) fn send_directory_descriptor(
    socket: &UnixStream,
    descriptor: std::os::fd::BorrowedFd<'_>,
) -> io::Result<()> {
    let (length, space) = lengths();
    if space != mem::size_of::<SingleRight>()
        || mem::offset_of!(SingleRight, descriptor) != length - mem::size_of::<libc::c_int>()
    {
        return Err(io::Error::other("unsupported ancillary layout"));
    }
    // Native C records contain integer and pointer fields valid when zeroed.
    let mut control: SingleRight = unsafe { mem::zeroed() };
    control.header.cmsg_len = length as _;
    control.header.cmsg_level = libc::SOL_SOCKET;
    control.header.cmsg_type = libc::SCM_RIGHTS;
    control.descriptor = descriptor.as_raw_fd();
    let mut byte = MARKER;
    let mut vector = libc::iovec {
        iov_base: (&mut byte as *mut u8).cast(),
        iov_len: 1,
    };
    // All pointers below refer to live, aligned stack storage for the syscall.
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = (&mut control as *mut SingleRight).cast();
    message.msg_controllen = space as _;
    #[cfg(target_os = "macos")]
    {
        let enabled: libc::c_int = 1;
        // This option prevents a peer close from raising SIGPIPE in the parent.
        if unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_NOSIGPIPE,
                (&enabled as *const libc::c_int).cast(),
                mem::size_of_val(&enabled) as _,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(target_os = "macos")]
    let flags = libc::MSG_DONTWAIT;
    #[cfg(target_os = "linux")]
    let flags = libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL;
    // The kernel reads one bounded payload and one descriptor from live storage.
    let sent = unsafe { libc::sendmsg(socket.as_raw_fd(), &message, flags) };
    if sent == 1 {
        Ok(())
    } else if sent < 0 {
        Err(io::Error::last_os_error())
    } else {
        Err(io::Error::other("incomplete history directory transfer"))
    }
}

/// Worker entry point only, before threads or descendant processes are started.
/// The parent controls the receive deadline by terminating this owned worker.
/// The returned directory still requires the scoped SQLite backend's IO policy.
pub fn receive_history_directory_or_exit(socket: &UnixStream) -> cap_std::fs::Dir {
    let (length, space) = lengths();
    if space != mem::size_of::<SingleRight>()
        || mem::offset_of!(SingleRight, descriptor) != length - mem::size_of::<libc::c_int>()
    {
        fail();
    }
    // No descriptor is owned in userspace until every native header check passes.
    let mut control: SingleRight = unsafe { mem::zeroed() };
    control.descriptor = -1;
    let mut byte = 0_u8;
    let mut vector = libc::iovec {
        iov_base: (&mut byte as *mut u8).cast(),
        iov_len: 1,
    };
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = (&mut control as *mut SingleRight).cast();
    message.msg_controllen = space as _;
    #[cfg(target_os = "linux")]
    let flags = libc::MSG_CMSG_CLOEXEC;
    #[cfg(target_os = "macos")]
    let flags = 0;
    // The kernel writes only into the supplied bounded stack buffers. Never walk
    // control headers: macOS can report a length beyond its returned byte count.
    let received = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, flags) };
    if received != 1
        || byte != MARKER
        || message.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0
        || (message.msg_controllen as usize) < length
        || (message.msg_controllen as usize) > space
        || control.header.cmsg_len as usize != length
        || control.header.cmsg_level != libc::SOL_SOCKET
        || control.header.cmsg_type != libc::SCM_RIGHTS
        || control.descriptor < 0
    {
        fail();
    }
    // This descriptor was installed by SCM_RIGHTS and has not been adopted before.
    let descriptor = unsafe { OwnedFd::from_raw_fd(control.descriptor) };
    // On macOS this must precede any thread/process creation in the one-shot child.
    if unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        fail();
    }
    let file = File::from(descriptor);
    if !file.metadata().is_ok_and(|metadata| metadata.is_dir()) {
        fail();
    }
    cap_std::fs::Dir::from_std_file(file)
}
