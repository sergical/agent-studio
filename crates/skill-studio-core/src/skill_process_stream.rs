//! Bounded child output for read-only fetch adapters. Command/env authority belongs
//! to the caller; this runner does not sandbox the child or authenticate its output.
use std::{
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct ProcessStreamLimits {
    pub stdout_bytes: u64,
    pub stderr_bytes: usize,
    pub deadline: Instant,
}

#[derive(Debug)]
pub struct ProcessStreamReport {
    pub stdout_bytes: u64,
    pub exit_code: i32,
}

struct OwnedChild {
    child: Child,
    group: i32,
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        // SAFETY: the child was placed in its own process group at spawn.
        unsafe {
            libc::kill(-self.group, libc::SIGTERM);
        }
        let until = Instant::now() + Duration::from_millis(200);
        loop {
            let _ = self.child.try_wait();
            // SAFETY: signal zero only probes the owned group's existence.
            if unsafe { libc::kill(-self.group, 0) } != 0 || Instant::now() >= until {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        // SAFETY: kill the remaining owned group before reaping its leader.
        unsafe {
            libc::kill(-self.group, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: the pipe owns a valid descriptor; existing flags are preserved.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn read_available(pipe: &mut impl Read, buffer: &mut [u8]) -> io::Result<Option<usize>> {
    match pipe.read(buffer) {
        Ok(count) => Ok(Some(count)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn retain_tail(bytes: &mut Vec<u8>, chunk: &[u8], limit: usize) {
    if chunk.len() >= limit {
        bytes.clear();
        bytes.extend_from_slice(&chunk[chunk.len() - limit..]);
    } else {
        let excess = bytes
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(limit);
        bytes.drain(..excess);
        bytes.extend_from_slice(chunk);
    }
}

/// `output` must accept bounded writes without indefinite blocking. The runner
/// enforces the deadline and calls `check` for cancellation. Errors leave partial output.
pub fn run_to_writer(
    command: &mut Command,
    output: &mut impl Write,
    limits: ProcessStreamLimits,
    check: impl Fn() -> Result<(), String>,
) -> Result<ProcessStreamReport, String> {
    run_to_writer_accepting(command, output, limits, check, &[0])
}

pub fn run_to_writer_accepting(
    command: &mut Command,
    output: &mut impl Write,
    limits: ProcessStreamLimits,
    check: impl Fn() -> Result<(), String>,
    accepted_codes: &[i32],
) -> Result<ProcessStreamReport, String> {
    let check = || {
        check()?;
        if Instant::now() >= limits.deadline {
            Err("Process output deadline exceeded".to_string())
        } else {
            Ok(())
        }
    };
    check()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let child = command.spawn().map_err(|error| error.to_string())?;
    let mut owned = OwnedChild {
        group: child.id() as i32,
        child,
    };
    let mut stdout = owned.child.stdout.take().ok_or("Missing child stdout")?;
    let mut stderr = owned.child.stderr.take().ok_or("Missing child stderr")?;
    nonblocking(&stdout).map_err(|error| error.to_string())?;
    nonblocking(&stderr).map_err(|error| error.to_string())?;
    let mut stdout_buffer = [0_u8; 64 * 1024];
    let mut stderr_buffer = [0_u8; 8 * 1024];
    let mut diagnostics = Vec::new();
    let mut written = 0_u64;
    let mut stdout_eof = false;
    let mut stderr_eof = false;
    let mut status = None;
    loop {
        check()?;
        let mut progress = false;
        if !stdout_eof {
            if let Some(count) = read_available(&mut stdout, &mut stdout_buffer)
                .map_err(|error| error.to_string())?
            {
                progress = true;
                stdout_eof = count == 0;
                if count as u64 > limits.stdout_bytes - written {
                    return Err("Process stdout byte limit exceeded".into());
                }
                output
                    .write_all(&stdout_buffer[..count])
                    .map_err(|error| format!("Failed to write process output: {error}"))?;
                written += count as u64;
            }
        }
        if !stderr_eof {
            if let Some(count) = read_available(&mut stderr, &mut stderr_buffer)
                .map_err(|error| error.to_string())?
            {
                progress = true;
                stderr_eof = count == 0;
                retain_tail(
                    &mut diagnostics,
                    &stderr_buffer[..count],
                    limits.stderr_bytes,
                );
            }
        }
        if status.is_none() {
            status = owned.child.try_wait().map_err(|error| error.to_string())?;
        }
        if let Some(status) = status.filter(|_| stdout_eof && stderr_eof) {
            check()?;
            let exit_code = status
                .code()
                .ok_or_else(|| format!("Process exited with {status}"))?;
            if !accepted_codes.contains(&exit_code) {
                return Err(if diagnostics.is_empty() {
                    format!("Process exited with {status}")
                } else {
                    String::from_utf8_lossy(&diagnostics).into_owned()
                });
            }
            return Ok(ProcessStreamReport {
                stdout_bytes: written,
                exit_code,
            });
        }
        if !progress {
            thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, path::Path};

    fn command(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script, "fixture"]);
        command
    }
    fn deadline() -> impl Fn() -> Result<(), String> {
        let end = Instant::now() + Duration::from_secs(3);
        move || {
            if Instant::now() < end {
                Ok(())
            } else {
                Err("deadline".into())
            }
        }
    }
    fn limits(bytes: u64) -> ProcessStreamLimits {
        ProcessStreamLimits {
            stdout_bytes: bytes,
            stderr_bytes: 64,
            deadline: Instant::now() + Duration::from_secs(3),
        }
    }
    fn assert_gone(path: &Path) {
        let pid: i32 = std::fs::read_to_string(path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(1);
        // SAFETY: signal zero observes the fixture PID without changing it.
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < until {
            thread::sleep(Duration::from_millis(5));
        }
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "fixture process remains alive"
        );
    }

    #[test]
    fn exact_limit_and_binary_output_succeed_but_fast_overflow_fails() {
        let mut output = Vec::new();
        let report = run_to_writer(
            &mut command("printf 'a\\000b'"),
            &mut output,
            limits(3),
            deadline(),
        )
        .unwrap();
        assert_eq!(output, b"a\0b");
        assert_eq!(report.stdout_bytes, 3);
        output.clear();
        assert!(run_to_writer(
            &mut command("printf 1234"),
            &mut output,
            limits(3),
            deadline()
        )
        .unwrap_err()
        .contains("limit"));
        assert!(output.len() <= 3);
        assert!(
            run_to_writer(&mut command("true"), &mut Vec::new(), limits(0), deadline()).is_ok()
        );
    }

    #[test]
    fn overflow_stops_and_reaps_a_producer_without_exceeding_disk_budget() {
        let temp = tempfile::tempdir().unwrap();
        let pid = temp.path().join("pid");
        let path = temp.path().join("output");
        let mut producer = command("echo $$ > \"$1\"; while :; do printf 0123456789; done");
        producer.arg(&pid);
        let mut output = std::fs::File::create(&path).unwrap();
        let error =
            run_to_writer(&mut producer, &mut output, limits(1024), deadline()).unwrap_err();
        assert!(error.contains("limit"));
        assert!(output.metadata().unwrap().len() <= 1024);
        assert_gone(&pid);
    }

    #[test]
    fn deadline_cleans_up_descendant_pipes_after_leader_exits() {
        let temp = tempfile::tempdir().unwrap();
        let pid = temp.path().join("pid");
        let mut producer = command("sleep 30 & echo $! > \"$1\"; exit 0");
        producer.arg(&pid);
        let end = Instant::now() + Duration::from_millis(200);
        let error = run_to_writer(
            &mut producer,
            &mut Vec::new(),
            ProcessStreamLimits {
                deadline: end,
                ..limits(1024)
            },
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(error, "Process output deadline exceeded");
        assert_gone(&pid);
    }

    #[test]
    fn cancellation_and_writer_failure_stop_the_child() {
        let temp = tempfile::tempdir().unwrap();
        let pid = temp.path().join("pid");
        let mut producer = command("echo $$ > \"$1\"; while :; do printf data; done");
        producer.arg(&pid);
        let calls = Cell::new(0);
        let end = deadline();
        let error = run_to_writer(&mut producer, &mut Vec::new(), limits(u64::MAX), || {
            end()?;
            calls.set(calls.get() + 1);
            if calls.get() > 20 {
                Err("cancelled".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error, "cancelled");
        assert_gone(&pid);
        struct FailedWriter;
        impl Write for FailedWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("disk failure"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let error = run_to_writer(
            &mut producer,
            &mut FailedWriter,
            limits(u64::MAX),
            deadline(),
        )
        .unwrap_err();
        assert!(error.contains("disk failure"));
        assert_gone(&pid);
    }

    #[test]
    fn drains_large_stderr_and_checks_control_before_spawn() {
        let mut output = Vec::new();
        let error = run_to_writer(
            &mut command("head -c 131072 /dev/zero >&2; printf done; exit 2"),
            &mut output,
            limits(4),
            deadline(),
        )
        .unwrap_err();
        assert_eq!(output, b"done");
        assert_eq!(error.len(), 64);
        let temp = tempfile::tempdir().unwrap();
        let pid = temp.path().join("pid");
        let mut producer = command("echo $$ > \"$1\"");
        producer.arg(&pid);
        assert_eq!(
            run_to_writer(&mut producer, &mut Vec::new(), limits(4), || Err(
                "cancelled".into()
            ))
            .unwrap_err(),
            "cancelled"
        );
        assert!(!pid.exists());
    }
}
