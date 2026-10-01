//! [`ProcessSpawner`] over the real OS process, for the `harnesses` op's
//! `--version` probe and the desktop's `npx`/`git` mutations.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdout, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use skill_studio_core::ports::{CancelToken, ProcessOutput, ProcessSpawner, ProcessSpec};
use skill_studio_core::CoreError;

use crate::tools::is_executable_file;

/// How often [`RealProcessSpawner::run`] polls a running child for exit
/// while waiting for `ProcessSpec::timeout_ms`'s deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long the timeout path waits for a reader thread to see EOF after
/// killing the child's whole process group, before giving up on it and
/// returning whatever was collected so far (possibly nothing). Bounded so a
/// reader that somehow never sees EOF can't turn the very deadline this
/// spawner exists to enforce into a second, unbounded hang.
const KILLED_READER_GRACE: Duration = Duration::from_millis(250);

/// Runs a child process with `std::process::Command` and waits for it to
/// exit, killing it if it outlives `ProcessSpec::timeout_ms`.
///
/// Cancellation via `CancelToken` is not enforced yet: `NeverCancel` is the
/// only token any caller passes today. Caching the result by the
/// executable's path, size, and mtime is a named follow-up in
/// `docs/action-map/harnesses/harness-detection.md`, not this type.
pub struct RealProcessSpawner {
    /// Directories searched to resolve a bare `ProcessSpec::program` and
    /// prepended to the child's `PATH`, ahead of this process's own. Empty
    /// for [`RealProcessSpawner::new`]: the CLI and MCP server already run
    /// under a terminal shell's full `PATH`, so they keep spawning exactly
    /// as before. Non-empty for [`RealProcessSpawner::with_search_path`],
    /// which the desktop app uses: launched from Finder, it inherits
    /// `launchd`'s minimal `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`), where
    /// neither `npx` nor the `node` its `#!/usr/bin/env node` shebang needs
    /// can be found.
    search_dirs: Vec<PathBuf>,
}

impl RealProcessSpawner {
    /// Builds the spawner. Stateless: every call spawns fresh.
    pub fn new() -> Self {
        RealProcessSpawner {
            search_dirs: Vec::new(),
        }
    }

    /// As [`RealProcessSpawner::new`], but resolving a bare `program` name
    /// against `search_dirs` first and prepending `search_dirs` to every
    /// child's `PATH` (unless `ProcessSpec::env` already sets `PATH`
    /// itself). Pass the same directories a `LoginShellToolLookup` probed,
    /// so the spawn agrees with whatever `find_binary` already promised the
    /// caller was there.
    pub fn with_search_path(search_dirs: Vec<PathBuf>) -> Self {
        RealProcessSpawner { search_dirs }
    }

    /// Resolves `program` to an absolute path under `search_dirs` when it is
    /// a bare name (no `/`); falls back to `program` unchanged otherwise, or
    /// when no `search_dirs` entry has it (`std::process::Command` then
    /// resolves it against the child's own `PATH`, set below).
    fn resolve_program(&self, program: &str) -> PathBuf {
        if program.contains('/') {
            return PathBuf::from(program);
        }
        self.search_dirs
            .iter()
            .map(|dir| dir.join(program))
            .find(|candidate| is_executable_file(candidate))
            .unwrap_or_else(|| PathBuf::from(program))
    }

    /// `search_dirs` joined with this process's own `PATH`, for a child that
    /// needs to resolve a second binary itself (`npx`'s `#!/usr/bin/env
    /// node` shebang needs `node` on the child's `PATH`, not just its own
    /// `argv[0]` resolved).
    fn child_path(&self) -> OsString {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        // A dir containing the PATH separator can't be represented in a
        // joined PATH string. Skip just that dir instead of letting
        // `join_paths` fail and falling back to `inherited` alone, which
        // would silently drop every other `search_dirs` entry too.
        let search_dirs = self
            .search_dirs
            .iter()
            .filter(|dir| !dir.as_os_str().to_string_lossy().contains(':'))
            .cloned();
        // An empty (unset or "") inherited PATH must contribute nothing, not
        // an empty path segment: `split_paths` on "" yields one empty
        // component, which `Command` resolves as the child's cwd - for
        // `npx` that is a project folder, not a directory to trust bare
        // names from.
        let inherited_dirs: Vec<PathBuf> = if inherited.is_empty() {
            Vec::new()
        } else {
            std::env::split_paths(&inherited).collect()
        };
        std::env::join_paths(search_dirs.chain(inherited_dirs)).unwrap_or(inherited)
    }

    /// The `PATH` `run` gives the child for `spec`.
    fn effective_path(&self, spec: &ProcessSpec) -> OsString {
        if let Some((_, value)) = spec.env.iter().find(|(key, _)| key == "PATH") {
            return OsString::from(value);
        }
        if self.search_dirs.is_empty() {
            return std::env::var_os("PATH").unwrap_or_default();
        }
        self.child_path()
    }

    /// For a failed `npx` run, appends one `Ran:` line naming the program,
    /// its argv, and the `node` on the child's `PATH`, so a wrong Node
    /// (Homebrew's instead of the user's mise) shows in the error. When that
    /// `node` cannot start at all, the output says so instead of passing on
    /// npx's raw dyld text. Never reads environment values or tokens.
    fn describe_failed_npx(
        &self,
        spec: &ProcessSpec,
        program: &Path,
        mut output: ProcessOutput,
    ) -> ProcessOutput {
        let is_npx = Path::new(&spec.program)
            .file_name()
            .is_some_and(|n| n == "npx");
        if !is_npx || output.status == Some(0) {
            return output;
        }
        let path = self.effective_path(spec);
        let node = std::env::split_paths(&path)
            .map(|dir| dir.join("node"))
            .find(|candidate| is_executable_file(candidate));
        if let Some(node) = &node {
            if let Some(broken) = broken_node_reason(node, &path) {
                output.stderr = format!("The Node at {} is broken: {broken}", node.display());
            }
        }
        let node_text = node.map_or_else(|| "not found".to_string(), |n| n.display().to_string());
        let mut line = format!("Ran: {}", program.display());
        for arg in &spec.args {
            line.push(' ');
            line.push_str(arg);
        }
        line.push_str(" (node: ");
        line.push_str(&node_text);
        line.push(')');
        if !output.stderr.is_empty() && !output.stderr.ends_with('\n') {
            output.stderr.push('\n');
        }
        output.stderr.push_str(&line);
        output
    }
}

/// The first dyld "Library not loaded" line from `node --version`, when
/// `node` fails to start for that reason. `None` for a working Node or any
/// other failure.
fn broken_node_reason(node: &Path, path: &OsString) -> Option<String> {
    let result = std::process::Command::new(node)
        .arg("--version")
        .env("PATH", path)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if result.status.success() {
        return None;
    }
    String::from_utf8_lossy(&result.stderr)
        .lines()
        .find(|line| line.contains("Library not loaded"))
        .map(|line| line.trim().to_string())
}

impl Default for RealProcessSpawner {
    fn default() -> Self {
        RealProcessSpawner::new()
    }
}

/// Spawns a thread that drains `pipe` to a `Vec<u8>`. Must start before the
/// caller polls the child for exit: a child that writes more than the pipe
/// buffer (about 64 KB) and isn't read blocks on that write forever, so
/// `try_wait` would never see it exit and the caller would always hit its
/// deadline instead of its real completion.
fn spawn_drain<R: Read + Send + 'static>(pipe: R) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut pipe = pipe;
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

/// Kills every process in `pid`'s process group (set via `process_group(0)`
/// at spawn below), not just `pid` itself. `npx` runs the installed package
/// in a `node` grandchild that inherits the pipe's write end; a lone
/// `child.kill()` only signals the direct child, leaving that grandchild
/// running and the pipe held open, so the reader thread's `read_to_end`
/// would never see EOF. Shells out to the `kill` binary rather than
/// `libc::kill` so this crate doesn't need unsafe code for it (the same
/// tradeoff `lease.rs`'s liveness probe makes with `sysinfo`). Uses the
/// POSIX `-s KILL -- -<pid>` form rather than `-KILL -<pid>`: procps `kill`
/// (Linux) reads a bare negative pid as an option and rejects it, where
/// `-- -<pid>` unambiguously marks it as the operand. Returns whether the
/// group kill itself succeeded; the caller still calls `child.kill()` on
/// the direct child regardless, so `child.wait()` can't block even if this
/// returns `false` (missing `kill` binary, a parsing difference, ...).
#[cfg(unix)]
fn kill_process_group(pid: u32) -> bool {
    let group = format!("-{pid}");
    std::process::Command::new("kill")
        .args(["-s", "KILL", "--", &group])
        .status()
        .is_ok_and(|status| status.success())
}

/// Joins `handle` if it finishes within [`KILLED_READER_GRACE`], else
/// abandons it and returns nothing collected. Only used after the child's
/// whole process group has already been killed, so the pipe's write end is
/// expected to close almost immediately - the bound exists for the case
/// where, for whatever reason, it doesn't.
fn join_killed_reader(handle: Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
    let Some(handle) = handle else {
        return Vec::new();
    };
    let deadline = Instant::now() + KILLED_READER_GRACE;
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    if handle.is_finished() {
        handle.join().unwrap_or_default()
    } else {
        Vec::new()
    }
}

impl ProcessSpawner for RealProcessSpawner {
    fn run(
        &self,
        spec: &ProcessSpec,
        _cancel: &dyn CancelToken,
    ) -> Result<ProcessOutput, CoreError> {
        let program = self.resolve_program(&spec.program);
        let mut command = std::process::Command::new(&program);
        command.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        if !self.search_dirs.is_empty() && !spec.env.iter().any(|(key, _)| key == "PATH") {
            command.env("PATH", self.child_path());
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Makes the child the leader of its own new process group, so
            // the timeout path below can kill the whole group - see
            // `kill_process_group`'s doc comment for why a lone
            // `child.kill()` isn't enough.
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|e| CoreError::io(Path::new(&spec.program), e))?;

        // Drain both pipes concurrently with the poll loop below, not after
        // it: see `spawn_drain`'s doc comment.
        let stdout_reader: Option<JoinHandle<Vec<u8>>> =
            child.stdout.take().map(spawn_drain::<ChildStdout>);
        let stderr_reader: Option<JoinHandle<Vec<u8>>> =
            child.stderr.take().map(spawn_drain::<ChildStderr>);

        let deadline = Duration::from_millis(spec.timeout_ms);
        let start = Instant::now();
        let exit_status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if start.elapsed() >= deadline => break None,
                Ok(None) => std::thread::sleep(POLL_INTERVAL),
                Err(e) => return Err(CoreError::io(Path::new(&spec.program), e)),
            }
        };

        let Some(status) = exit_status else {
            // The child outlived its deadline: kill and reap its whole
            // process group (not just the direct child - see
            // `kill_process_group`), then report `timed_out` rather than
            // guess at output the process never finished writing. The
            // reader-thread joins are bounded (`join_killed_reader`): the
            // group kill should close every pipe write end almost at once,
            // but nothing here should be able to block `run` forever.
            #[cfg(unix)]
            let _ = kill_process_group(child.id());
            // Always also kill the direct child: if the group kill above
            // failed for any reason, this still guarantees `child.wait()`
            // below can't block for the rest of the deadline-less sleep.
            let _ = child.kill();
            let _ = child.wait();
            let stdout = join_killed_reader(stdout_reader);
            let stderr = join_killed_reader(stderr_reader);
            return Ok(self.describe_failed_npx(
                spec,
                &program,
                ProcessOutput {
                    status: None,
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    timed_out: true,
                },
            ));
        };

        let stdout = stdout_reader
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let stderr = stderr_reader
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        Ok(self.describe_failed_npx(
            spec,
            &program,
            ProcessOutput {
                status: status.code(),
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
                timed_out: false,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::ports::NeverCancel;

    fn write_script(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn run_fake_npx(dir: &Path) -> ProcessOutput {
        let spec = ProcessSpec {
            program: "npx".into(),
            args: vec!["skills".into(), "update".into(), "foo".into()],
            cwd: None,
            env: Vec::new(),
            timeout_ms: 10_000,
        };
        RealProcessSpawner::with_search_path(vec![dir.to_path_buf()])
            .run(&spec, &NeverCancel)
            .unwrap()
    }

    /// `a_failed_npx_names_the_program_argv_and_node_it_ran_or_names_the_missing_part`:
    /// a user must be able to see which `npx` and `node` ran. Fails if the
    /// failure output lacks the resolved program, the argv, or the node path.
    #[test]
    fn a_failed_npx_names_the_program_argv_and_node_it_ran_or_names_the_missing_part() {
        let tmp = tempfile::tempdir().unwrap();
        write_script(&tmp.path().join("npx"), "echo boom >&2; exit 1");
        write_script(&tmp.path().join("node"), "echo v22.0.0");

        let output = run_fake_npx(tmp.path());

        let expected = format!(
            "Ran: {0}/npx skills update foo (node: {0}/node)",
            tmp.path().display()
        );
        assert!(
            output.stderr.contains("boom") && output.stderr.contains(&expected),
            "stderr lacks `{expected}`: {:?}",
            output.stderr
        );
    }

    /// `a_node_that_cannot_load_its_library_is_reported_as_broken_or_shows_the_raw_npx_text`:
    /// Homebrew's `node` fails with dyld "Library not loaded" after a
    /// dependency upgrade. Fails if the output keeps npx's raw text instead
    /// of naming the broken Node.
    #[test]
    fn a_node_that_cannot_load_its_library_is_reported_as_broken_or_shows_the_raw_npx_text() {
        let tmp = tempfile::tempdir().unwrap();
        write_script(&tmp.path().join("npx"), "echo 'env: node: bad' >&2; exit 1");
        write_script(
            &tmp.path().join("node"),
            "echo 'dyld[1]: Library not loaded: libsimdjson.dylib' >&2; exit 1",
        );

        let output = run_fake_npx(tmp.path());

        let broken = format!(
            "The Node at {}/node is broken: dyld[1]: Library not loaded",
            tmp.path().display()
        );
        assert!(
            output.stderr.contains(&broken),
            "stderr does not report the broken Node: {:?}",
            output.stderr
        );
    }

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
    /// a real child that records its pid and then `exec`s `sleep 30` - a
    /// mock spawner can't prove a real OS process gets killed, so this test
    /// pays for a real spawn - must come back as `timed_out`, and the pid it
    /// recorded must be gone once `run` returns (`kill -0` fails), which
    /// proves the child was killed and reaped rather than left running.
    /// The deadline is generous so the shell has time to write its pid;
    /// nothing asserts an elapsed-time bound, so a slow CI runner still
    /// gets a correct verdict.
    #[test]
    fn a_hung_version_probe_times_out_and_is_killed_or_names_the_probe_that_hangs() {
        let tmp = tempfile::tempdir().unwrap();
        let pid_file = tmp.path().join("pid");
        let spawner = RealProcessSpawner::new();
        let spec = ProcessSpec {
            program: "sh".into(),
            args: vec![
                "-c".into(),
                format!("echo $$ > '{}'; exec sleep 30", pid_file.display()),
            ],
            cwd: None,
            env: Vec::new(),
            timeout_ms: 1_000,
        };

        let output = spawner.run(&spec, &NeverCancel).unwrap();

        assert!(
            output.timed_out,
            "a probe past its deadline must report timed_out, got {output:?}"
        );
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let pid = pid.trim();
        assert!(
            !pid.is_empty(),
            "the child never recorded its pid, so the kill cannot be checked"
        );
        let still_alive = std::process::Command::new("kill")
            .args(["-0", pid])
            .status()
            .unwrap()
            .success();
        assert!(
            !still_alive,
            "the hung probe (pid {pid}) is still running after run() returned - it was abandoned, not killed"
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

    fn write_executable_script(path: &Path, script: &str) {
        std::fs::write(path, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    /// `a_packaged_apps_minimal_process_path_still_resolves_a_bare_npx_and_the_node_its_shebang_needs_or_names_which_lookup_starved`:
    /// a fake `npx` (`#!/bin/sh` that `exec`s `/usr/bin/env node`) and a
    /// fake `node`, both only in a temp dir the *process's own* `PATH` does
    /// not contain, mimic a packaged desktop app launched from Finder under
    /// `launchd`'s minimal `PATH`: without `with_search_path`, `Command::new
    /// ("npx")` can't find the bare name, and even given `npx`'s absolute
    /// path directly, its own `env node` step still can't find `node` on
    /// the child's `PATH`. Fails (red) on `RealProcessSpawner::new()` -
    /// spawn error or empty stdout - and on `with_search_path` without the
    /// child `PATH` env write.
    #[test]
    fn a_packaged_apps_minimal_process_path_still_resolves_a_bare_npx_and_the_node_its_shebang_needs_or_names_which_lookup_starved(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        write_executable_script(
            &tmp.path().join("npx"),
            "#!/bin/sh\nexec /usr/bin/env node\n",
        );
        write_executable_script(&tmp.path().join("node"), "#!/bin/sh\necho fake-node-ok\n");
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        assert!(
            !std::env::split_paths(&inherited_path).any(|dir| dir == tmp.path()),
            "test setup bug: the fake tool dir must not already be on this process's own PATH"
        );

        let spawner = RealProcessSpawner::with_search_path(vec![tmp.path().to_path_buf()]);
        let spec = ProcessSpec {
            program: "npx".into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            // Generous: this only needs to outlast a trivial `sh`/`env`
            // spawn, but a full `cargo test --workspace` run schedules this
            // alongside other tests that spawn and sleep real child
            // processes, and 2s was tight enough under that load to time
            // out this fake `npx` before it ever ran.
            timeout_ms: 5_000,
        };

        let output = spawner.run(&spec, &NeverCancel).unwrap();

        assert!(!output.timed_out, "the fake npx never ran: {output:?}");
        assert_eq!(
            output.stdout.trim(),
            "fake-node-ok",
            "npx's own `env node` step could not find node on the child's PATH: {output:?}"
        );
    }

    /// `a_child_writing_past_the_pipe_buffer_still_finishes_by_its_deadline_or_names_the_hang`:
    /// `sh -c 'yes x | head -c 200000'` writes 200 KB to stdout, more than a
    /// pipe's buffer (about 64 KB). Draining stdout only starts after
    /// `try_wait` first sees the child exit; if nothing reads the pipe while
    /// polling, the write blocks, the child never exits, and `run` always
    /// hits the deadline instead of the child's real completion. Fails (red)
    /// on the old poll-then-read order: `timed_out` comes back `true` and
    /// `stdout` is empty or truncated instead of the full 200000 bytes.
    #[test]
    fn a_child_writing_past_the_pipe_buffer_still_finishes_by_its_deadline_or_names_the_hang() {
        let spawner = RealProcessSpawner::new();
        let spec = ProcessSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "yes x | head -c 200000".into()],
            cwd: None,
            env: Vec::new(),
            timeout_ms: 5_000,
        };

        let output = spawner.run(&spec, &NeverCancel).unwrap();

        assert!(
            !output.timed_out,
            "a child writing past the pipe buffer was reported as hung: {output:?}"
        );
        assert_eq!(
            output.stdout.len(),
            200_000,
            "expected the full 200000 bytes the child wrote, got {}",
            output.stdout.len()
        );
    }

    /// `a_timed_out_childs_grandchild_that_inherited_the_pipe_is_still_killed_or_names_the_orphan_left_running`:
    /// the child backgrounds a grandchild that inherits the stdout pipe's
    /// write end and holds it open for 30s, then the child itself blocks
    /// past a 500ms deadline. A lone `child.kill()` (the direct child only)
    /// leaves the grandchild running with the pipe open, so the old
    /// unbounded `JoinHandle::join()` on the reader thread never sees EOF
    /// and `run` never returns. Fails (red) on that: `run` hangs well past
    /// the grandchild's own 30s sleep instead of returning near the
    /// deadline, and the grandchild's pid is still alive afterward.
    #[test]
    fn a_timed_out_childs_grandchild_that_inherited_the_pipe_is_still_killed_or_names_the_orphan_left_running(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let grandchild_pid_file = tmp.path().join("grandchild_pid");
        let spawner = RealProcessSpawner::new();
        let spec = ProcessSpec {
            program: "sh".into(),
            args: vec![
                "-c".into(),
                format!(
                    "sh -c 'echo $$ > \"{}\"; exec sleep 30' & sleep 30",
                    grandchild_pid_file.display()
                ),
            ],
            cwd: None,
            env: Vec::new(),
            timeout_ms: 500,
        };

        let start = Instant::now();
        let output = spawner.run(&spec, &NeverCancel).unwrap();
        let elapsed = start.elapsed();

        assert!(
            output.timed_out,
            "a child whose grandchild holds the pipe open must still report timed_out, got {output:?}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "run took {elapsed:?} - it waited on the grandchild's pipe instead of bounding the join"
        );
        let grandchild_pid = std::fs::read_to_string(&grandchild_pid_file).unwrap_or_default();
        let grandchild_pid = grandchild_pid.trim();
        assert!(
            !grandchild_pid.is_empty(),
            "the backgrounded grandchild never recorded its pid"
        );
        let grandchild_alive = std::process::Command::new("kill")
            .args(["-0", grandchild_pid])
            .status()
            .unwrap()
            .success();
        assert!(
            !grandchild_alive,
            "the grandchild (pid {grandchild_pid}) survived the timeout - only the direct child was killed"
        );
    }
}
