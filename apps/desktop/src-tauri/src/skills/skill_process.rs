// ============================================================================
// Skills Module - skill_process
// Controlled child spawning for Add Skill CLI work: null stdin, bounded
// stdout/stderr, process-group kill on cancel or timeout, and a wait that
// reaps the child. Add Skill must not use `Command::output()` on the UI
// thread; this helper is what the background worker calls instead.
// ============================================================================

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How long a cancelled or timed-out child gets after SIGTERM before SIGKILL.
pub const PROCESS_CANCEL_GRACE: Duration = Duration::from_secs(2);

/// Default Add Skill CLI timeout. Tests pass a shorter value.
pub const DEFAULT_ADD_PROCESS_TIMEOUT: Duration = Duration::from_secs(300);

/// Bytes kept from each of stdout and stderr. Extra output is dropped.
pub const MAX_PROCESS_OUTPUT_BYTES: usize = 64 * 1024;

/// Unique literal so callers can tell cancel from CLI stderr without parsing
/// untrusted process text as a repo identity.
pub const PROCESS_CANCELLED_MESSAGE: &str = "Add skill process cancelled";

/// Unique literal for the timed-out state.
pub const PROCESS_TIMED_OUT_MESSAGE: &str = "Add skill process timed out";

/// One cancellation flag and absolute deadline shared by all work in an Add
/// operation. Synchronous callers get the same finite default deadline.
#[derive(Clone)]
pub struct AddOperationControl {
    cancel: Arc<AtomicBool>,
    deadline: Instant,
}

impl AddOperationControl {
    pub fn new(cancel: Arc<AtomicBool>, timeout: Duration) -> Self {
        Self {
            cancel,
            deadline: Instant::now() + timeout,
        }
    }

    pub fn with_deadline(cancel: Arc<AtomicBool>, deadline: Instant) -> Self {
        Self { cancel, deadline }
    }

    pub fn bounded_default() -> Self {
        Self::new(
            Arc::new(AtomicBool::new(false)),
            DEFAULT_ADD_PROCESS_TIMEOUT,
        )
    }

    pub fn check(&self) -> Result<(), ControlledProcessError> {
        if self.cancel.load(Ordering::SeqCst) {
            Err(ControlledProcessError::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(ControlledProcessError::TimedOut)
        } else {
            Ok(())
        }
    }

    pub fn check_message(&self) -> Result<(), String> {
        self.check().map_err(ControlledProcessError::into_message)
    }

    fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }

    fn remaining(&self) -> Result<Duration, ControlledProcessError> {
        self.check()?;
        Ok(self.deadline.saturating_duration_since(Instant::now()))
    }
}

/// Why a controlled process did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlledProcessError {
    Cancelled,
    TimedOut,
    Failed(String),
}

impl ControlledProcessError {
    pub fn into_message(self) -> String {
        match self {
            Self::Cancelled => PROCESS_CANCELLED_MESSAGE.to_string(),
            Self::TimedOut => PROCESS_TIMED_OUT_MESSAGE.to_string(),
            Self::Failed(message) => message,
        }
    }
}

/// Caps a growing byte buffer at `max` by dropping the oldest bytes.
fn push_bounded(buffer: &mut Vec<u8>, chunk: &[u8], max: usize) {
    if max == 0 {
        return;
    }
    buffer.extend_from_slice(chunk);
    if buffer.len() > max {
        let excess = buffer.len() - max;
        buffer.drain(0..excess);
    }
}

fn drain_pipe_bounded<R: Read>(mut reader: R, sink: Arc<Mutex<Vec<u8>>>, max: usize) {
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Ok(mut guard) = sink.lock() {
                    push_bounded(&mut guard, &chunk[..n], max);
                }
            }
        }
    }
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: i32) {
    // SAFETY: `pid` is the leader created by `process_group(0)`. A negative
    // pid addresses that whole process group. An absent group is harmless.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

#[cfg(unix)]
fn process_group_exists(pid: u32) -> bool {
    // SAFETY: signal 0 changes no process state. It only tests whether the
    // process group exists or cannot be inspected due to permissions.
    let result = unsafe { libc::kill(-(pid as i32), 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn terminate_and_reap(child: &mut std::process::Child, pid: u32) {
    #[cfg(unix)]
    signal_process_group(pid, libc::SIGTERM);
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }

    let deadline = Instant::now() + PROCESS_CANCEL_GRACE;
    #[cfg(unix)]
    while process_group_exists(pid) && Instant::now() < deadline {
        let _ = child.try_wait();
        thread::sleep(Duration::from_millis(20));
    }
    #[cfg(not(unix))]
    while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }

    #[cfg(unix)]
    if process_group_exists(pid) {
        signal_process_group(pid, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn join_finished_reader(handle: thread::JoinHandle<()>) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    if handle.is_finished() {
        let _ = handle.join();
    }
}

/// Run `program args` with stdin closed, bounded output, and kill-on-cancel
/// or timeout. Always waits so the child is reaped.
pub fn run_controlled_command(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    cancel: &AtomicBool,
    timeout: Duration,
    max_output_bytes: usize,
) -> Result<(), ControlledProcessError> {
    if cancel.load(Ordering::SeqCst) {
        return Err(ControlledProcessError::Cancelled);
    }

    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command.spawn().map_err(|error| {
        ControlledProcessError::Failed(format!("Failed to execute {program}: {error}"))
    })?;
    let pid = child.id();

    let stdout_buf = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::new()));
    let stdout_thread = child.stdout.take().map(|stdout| {
        let sink = Arc::clone(&stdout_buf);
        thread::spawn(move || drain_pipe_bounded(stdout, sink, max_output_bytes))
    });
    let stderr_thread = child.stderr.take().map(|stderr| {
        let sink = Arc::clone(&stderr_buf);
        thread::spawn(move || drain_pipe_bounded(stderr, sink, max_output_bytes))
    });

    let started = Instant::now();
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    Ok(())
                } else {
                    let stderr = stderr_buf
                        .lock()
                        .map(|guard| String::from_utf8_lossy(&guard).into_owned())
                        .unwrap_or_default();
                    let stdout = stdout_buf
                        .lock()
                        .map(|guard| String::from_utf8_lossy(&guard).into_owned())
                        .unwrap_or_default();
                    let message = if stderr.is_empty() { stdout } else { stderr };
                    Err(ControlledProcessError::Failed(if message.is_empty() {
                        format!("{program} exited with code {}", status.code().unwrap_or(-1))
                    } else {
                        message
                    }))
                };
            }
            Ok(None) => {
                if cancel.load(Ordering::SeqCst) {
                    terminate_and_reap(&mut child, pid);
                    break Err(ControlledProcessError::Cancelled);
                }
                if started.elapsed() >= timeout {
                    terminate_and_reap(&mut child, pid);
                    break Err(ControlledProcessError::TimedOut);
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                terminate_and_reap(&mut child, pid);
                break Err(ControlledProcessError::Failed(format!(
                    "Failed to wait on {program}: {error}"
                )));
            }
        }
    };

    if let Some(handle) = stdout_thread {
        join_finished_reader(handle);
    }
    if let Some(handle) = stderr_thread {
        join_finished_reader(handle);
    }
    outcome
}

/// Run a command under one operation deadline and return bounded stdout.
pub fn run_controlled_command_output(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    control: &AddOperationControl,
    max_output_bytes: usize,
) -> Result<Vec<u8>, ControlledProcessError> {
    run_controlled_command_io(program, args, cwd, control, max_output_bytes, None)
}

/// Run a command under one operation deadline with stdout redirected to a
/// file. Diagnostic stderr remains bounded in memory.
pub fn run_controlled_command_to_file(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    control: &AddOperationControl,
    output_path: &Path,
    max_output_bytes: usize,
) -> Result<(), ControlledProcessError> {
    run_controlled_command_io(
        program,
        args,
        cwd,
        control,
        max_output_bytes,
        Some(output_path.to_path_buf()),
    )
    .map(|_| ())
}

fn run_controlled_command_io(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    control: &AddOperationControl,
    max_output_bytes: usize,
    output_path: Option<PathBuf>,
) -> Result<Vec<u8>, ControlledProcessError> {
    control.check()?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(path) = &output_path {
        let file = std::fs::File::create(path).map_err(|error| {
            ControlledProcessError::Failed(format!(
                "Failed to create command output {}: {error}",
                path.display()
            ))
        })?;
        command.stdout(Stdio::from(file));
    } else {
        command.stdout(Stdio::piped());
    }
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command.spawn().map_err(|error| {
        ControlledProcessError::Failed(format!("Failed to execute {}: {error}", program.display()))
    })?;
    let pid = child.id();
    let stdout_buf = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::new()));
    let stdout_thread = child.stdout.take().map(|stdout| {
        let sink = Arc::clone(&stdout_buf);
        thread::spawn(move || drain_pipe_bounded(stdout, sink, max_output_bytes))
    });
    let stderr_thread = child.stderr.take().map(|stderr| {
        let sink = Arc::clone(&stderr_buf);
        thread::spawn(move || drain_pipe_bounded(stderr, sink, max_output_bytes))
    });

    let mut outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                break Ok(stdout_buf
                    .lock()
                    .map(|bytes| bytes.clone())
                    .unwrap_or_default())
            }
            Ok(Some(status)) => {
                let stderr = stderr_buf
                    .lock()
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default();
                let stdout = stdout_buf
                    .lock()
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default();
                let message = if stderr.is_empty() { stdout } else { stderr };
                break Err(ControlledProcessError::Failed(if message.is_empty() {
                    format!(
                        "{} exited with code {}",
                        program.display(),
                        status.code().unwrap_or(-1)
                    )
                } else {
                    message
                }));
            }
            Ok(None) => {
                if let Err(error) = control.check() {
                    terminate_and_reap(&mut child, pid);
                    break Err(error);
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                terminate_and_reap(&mut child, pid);
                break Err(ControlledProcessError::Failed(format!(
                    "Failed to wait on {}: {error}",
                    program.display()
                )));
            }
        }
    };
    if let Some(handle) = stdout_thread {
        join_finished_reader(handle);
    }
    if let Some(handle) = stderr_thread {
        join_finished_reader(handle);
    }
    if outcome.is_ok() {
        outcome = Ok(stdout_buf
            .lock()
            .map(|bytes| bytes.clone())
            .unwrap_or_default());
    }
    outcome
}

/// `npx` entry used by Add Skill. Stdin is always null.
pub fn run_controlled_npx(
    args: &[String],
    cwd: Option<&Path>,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<(), ControlledProcessError> {
    run_controlled_command("npx", args, cwd, cancel, timeout, MAX_PROCESS_OUTPUT_BYTES)
}

/// `npx` entry that consumes the remaining time from one Add operation.
pub fn run_controlled_npx_with_control(
    args: &[String],
    cwd: Option<&Path>,
    control: &AddOperationControl,
) -> Result<(), ControlledProcessError> {
    let timeout = control.remaining()?;
    run_controlled_command(
        "npx",
        args,
        cwd,
        control.cancel_flag(),
        timeout,
        MAX_PROCESS_OUTPUT_BYTES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn cancelled_before_spawn_does_not_start_a_child() {
        let cancel = AtomicBool::new(true);
        let error = run_controlled_command(
            "sleep",
            &["30".to_string()],
            None,
            &cancel,
            Duration::from_secs(5),
            MAX_PROCESS_OUTPUT_BYTES,
        )
        .unwrap_err();
        assert_eq!(error, ControlledProcessError::Cancelled);
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_and_reaps_a_sleeping_child() {
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let error = run_controlled_command(
            "sleep",
            &["30".to_string()],
            None,
            &cancel,
            Duration::from_millis(80),
            MAX_PROCESS_OUTPUT_BYTES,
        )
        .unwrap_err();
        assert_eq!(error, ControlledProcessError::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_descendant_that_ignores_term_and_holds_output_pipes() {
        let tmp = tempfile::tempdir().unwrap();
        let pid_file = tmp.path().join("descendant.pid");
        let script = format!(
            "trap 'exit 0' TERM; sh -c 'trap \"\" TERM; echo $$ > \"{}\"; while :; do sleep 1; done' & wait",
            pid_file.display()
        );
        let cancel = AtomicBool::new(false);
        let started = Instant::now();

        let error = run_controlled_command(
            "sh",
            &["-c".to_string(), script],
            None,
            &cancel,
            Duration::from_secs(1),
            MAX_PROCESS_OUTPUT_BYTES,
        )
        .unwrap_err();

        assert_eq!(error, ControlledProcessError::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(4));
        let descendant_pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let alive = unsafe { libc::kill(descendant_pid, 0) } == 0;
        assert!(
            !alive,
            "descendant {descendant_pid} survived process-group kill"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancel_kills_and_reaps_a_sleeping_child() {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::clone(&cancel);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            cancel_flag.store(true, Ordering::SeqCst);
        });
        let error = run_controlled_command(
            "sleep",
            &["30".to_string()],
            None,
            &cancel,
            Duration::from_secs(10),
            MAX_PROCESS_OUTPUT_BYTES,
        )
        .unwrap_err();
        assert_eq!(error, ControlledProcessError::Cancelled);
    }

    #[cfg(unix)]
    #[test]
    fn output_is_capped_at_the_configured_bound() {
        let cancel = AtomicBool::new(false);
        let error = run_controlled_command(
            "sh",
            &[
                "-c".to_string(),
                "printf '%*s' 20000 '' | tr ' ' x >&2; exit 1".to_string(),
            ],
            None,
            &cancel,
            Duration::from_secs(5),
            64,
        )
        .unwrap_err();
        match error {
            ControlledProcessError::Failed(message) => {
                assert!(message.len() <= 64, "{}", message.len())
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn successful_true_is_ok() {
        let cancel = AtomicBool::new(false);
        run_controlled_command(
            "true",
            &[],
            None,
            &cancel,
            Duration::from_secs(5),
            MAX_PROCESS_OUTPUT_BYTES,
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn shared_deadline_times_out_an_external_command() {
        let control =
            AddOperationControl::new(Arc::new(AtomicBool::new(false)), Duration::from_millis(80));
        let error = run_controlled_command_output(
            Path::new("sh"),
            &["-c".to_string(), "sleep 30".to_string()],
            None,
            &control,
            MAX_PROCESS_OUTPUT_BYTES,
        )
        .unwrap_err();
        assert_eq!(error, ControlledProcessError::TimedOut);
    }
}
