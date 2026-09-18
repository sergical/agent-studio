//! [`ProcessSpawner`] over the real OS process, for the `harnesses` op's
//! `--version` probe.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use skill_studio_core::ports::{CancelToken, ProcessOutput, ProcessSpawner, ProcessSpec};
use skill_studio_core::CoreError;

/// How often [`RealProcessSpawner::run`] polls a running child for exit
/// while waiting for `ProcessSpec::timeout_ms`'s deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Runs a child process with `std::process::Command` and waits for it to
/// exit, killing it if it outlives `ProcessSpec::timeout_ms`.
///
/// Cancellation via `CancelToken` is not enforced yet: `NeverCancel` is the
/// only token any caller passes today. Caching the result by the
/// executable's path, size, and mtime is a named follow-up in
/// `docs/action-map/harnesses/harness-detection.md`, not this type.
pub struct RealProcessSpawner;

impl RealProcessSpawner {
    /// Builds the spawner. Stateless: every call spawns fresh.
    pub fn new() -> Self {
        RealProcessSpawner
    }
}

impl Default for RealProcessSpawner {
    fn default() -> Self {
        RealProcessSpawner::new()
    }
}

impl ProcessSpawner for RealProcessSpawner {
    fn run(
        &self,
        spec: &ProcessSpec,
        _cancel: &dyn CancelToken,
    ) -> Result<ProcessOutput, CoreError> {
        let mut command = std::process::Command::new(&spec.program);
        command.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| CoreError::io(Path::new(&spec.program), e))?;

        let deadline = Duration::from_millis(spec.timeout_ms);
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => break,
                Ok(None) if start.elapsed() >= deadline => {
                    // The child outlived its deadline: kill and reap it so
                    // the caller never blocks on a hung `--version` probe,
                    // then report `timed_out` rather than guess at output
                    // the process never finished writing.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(ProcessOutput {
                        status: None,
                        stdout: String::new(),
                        stderr: String::new(),
                        timed_out: true,
                    });
                }
                Ok(None) => std::thread::sleep(POLL_INTERVAL),
                Err(e) => return Err(CoreError::io(Path::new(&spec.program), e)),
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|e| CoreError::io(Path::new(&spec.program), e))?;
        Ok(ProcessOutput {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            timed_out: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::ports::NeverCancel;

    #[test]
    fn run_captures_stdout_and_exit_status_or_names_the_missing_field() {
        let spawner = RealProcessSpawner::new();
        let spec = ProcessSpec {
            program: "echo".into(),
            args: vec!["hello".into()],
            cwd: None,
            env: Vec::new(),
            timeout_ms: 2_000,
        };
        let output = spawner.run(&spec, &NeverCancel).unwrap();
        assert_eq!(output.status, Some(0), "echo did not exit 0");
        assert_eq!(output.stdout.trim(), "hello");
        assert!(!output.timed_out);
    }

    /// `a_hung_version_probe_times_out_and_is_killed_or_names_the_probe_that_hangs`:
    /// a real `sh -c 'sleep 30'` child - a mock spawner can't prove a real
    /// OS process gets killed, so this test pays for a real spawn - given a
    /// 50 ms deadline must report `timed_out` rather than block the caller
    /// for anywhere near 30 seconds. Only the outcome is asserted, never an
    /// elapsed-time bound: the test would still be correct on a much slower
    /// CI runner.
    #[test]
    fn a_hung_version_probe_times_out_and_is_killed_or_names_the_probe_that_hangs() {
        let spawner = RealProcessSpawner::new();
        let spec = ProcessSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "sleep 30".into()],
            cwd: None,
            env: Vec::new(),
            timeout_ms: 50,
        };

        let output = spawner.run(&spec, &NeverCancel).unwrap();

        assert!(
            output.timed_out,
            "a probe past its deadline must report timed_out, got {output:?}"
        );
    }

    #[test]
    fn run_reports_a_spawn_error_for_a_missing_binary_rather_than_panicking() {
        let spawner = RealProcessSpawner::new();
        let spec = ProcessSpec {
            program: "definitely-not-a-real-skill-studio-binary".into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            timeout_ms: 2_000,
        };
        let err = spawner
            .run(&spec, &NeverCancel)
            .expect_err("a nonexistent program must not spawn");
        assert_eq!(err.code, skill_studio_core::ErrorCode::Io);
    }
}
