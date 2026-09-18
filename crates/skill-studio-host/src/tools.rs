//! [`ToolLookup`] over the real `PATH`.

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

use skill_studio_core::ports::ToolLookup;

/// `ToolLookup` backed by the process `PATH`.
///
/// Unix semantics only: a name resolves when a directory on the search path
/// holds a regular file with any executable bit set. There is no `PATHEXT`
/// step and no extension guessing.
pub struct PathToolLookup {
    /// Directories searched in order, most preferred first.
    search_dirs: Vec<PathBuf>,
}

impl PathToolLookup {
    /// Builds a lookup over the current process's `PATH` environment
    /// variable. An unset or empty `PATH` searches nothing.
    pub fn new() -> Self {
        let path = std::env::var_os("PATH").unwrap_or_default();
        PathToolLookup {
            search_dirs: std::env::split_paths(&path).collect(),
        }
    }

    /// Builds a lookup over an explicit list of directories, most preferred
    /// first. Intended for tests, which do not want to depend on the real
    /// `PATH`.
    pub fn with_search_dirs(search_dirs: Vec<PathBuf>) -> Self {
        PathToolLookup { search_dirs }
    }
}

impl Default for PathToolLookup {
    fn default() -> Self {
        PathToolLookup::new()
    }
}

/// True when `path` names a regular file with an executable bit set for the
/// owner, group, or others.
fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

impl ToolLookup for PathToolLookup {
    fn find_binary(&self, name: &str) -> Option<PathBuf> {
        self.search_dirs.iter().find_map(|dir| {
            let candidate = dir.join(name);
            is_executable_file(&candidate)
                .then(|| std::fs::canonicalize(&candidate).unwrap_or(candidate))
        })
    }
}

/// Markers the login-shell probe script prints around `PATH`, mirroring
/// `skill_editor.rs`'s `MARKER_START`/`MARKER_END`: a login shell's rc files
/// can print a banner (`echo Welcome`, nvm's "Now using node ...") before
/// the value the script asked for, so the parser must find the line between
/// these markers rather than assume `PATH` is the first line printed. The
/// end marker also lets the reader thread stop without waiting for the
/// shell to exit; see `run_with_timeout`'s doc comment for why a plain
/// `read_to_string` would hang on some machines (an rc file that leaves a
/// background process holding the pipe open).
const PATH_MARKER_START: &str = "__skill_studio_path_start__";
const PATH_MARKER_END: &str = "__skill_studio_path_end__";

/// Deadline for the login-shell `PATH` probe, per
/// `docs/action-map/harnesses/harness-detection.md`'s "two-second timeout
/// per process".
const SHELL_PROBE_TIMEOUT: Duration = Duration::from_millis(2000);

/// Reads stdout on a helper thread so a login shell's rc files can't hang
/// this forever; see `PATH_MARKER_END`'s doc comment. Mirrors
/// `skill_editor.rs`'s `run_with_timeout`.
fn run_with_timeout(mut command: Command, end_marker: &str, timeout: Duration) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    let end_marker = end_marker.to_string();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut collected = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let is_end_line = line.contains(&end_marker);
                    collected.push_str(&line);
                    if is_end_line {
                        break;
                    }
                }
            }
        }
        let _ = tx.send(collected);
    });

    let result = rx.recv_timeout(timeout).ok();
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// The login shell command that prints `$PATH`, per
/// `docs/action-map/harnesses/harness-detection.md`'s "PATH resolution":
/// macOS launches a desktop app with the minimal `launchd` PATH, so this
/// asks the user's own login shell instead.
fn login_shell_path_probe(shell: &str) -> Command {
    let mut command = Command::new(shell);
    command.arg("-lic").arg(format!(
        "echo {PATH_MARKER_START}; echo \"$PATH\"; echo {PATH_MARKER_END}"
    ));
    command
}

/// Parses the `$PATH` line between the start and end markers, tolerating
/// any banner text a login shell's rc files print before or after them (an
/// `echo Welcome`, nvm's "Now using node ..."). Mirrors `skill_editor.rs`'s
/// `parse_terminal_editor`. Returns `None` when the start marker never
/// appears (spawn failure, timeout, or a marker the shell mangled).
fn parse_path_probe_output(stdout: &str) -> Option<String> {
    let start = stdout.find(PATH_MARKER_START)?;
    let after_start = &stdout[start + PATH_MARKER_START.len()..];
    let end = after_start
        .find(PATH_MARKER_END)
        .unwrap_or(after_start.len());
    let body = &after_start[..end];
    body.lines()
        .find(|line| !line.trim().is_empty())
        .map(str::to_string)
}

/// Runs the login shell once to read `$PATH`. Returns the fallback
/// directories from harness-detection.md's "PATH resolution" when the probe
/// fails or times out, rather than an empty path, so detection still finds
/// binaries a version-manager shim installs outside the process's own
/// minimal `PATH`.
fn read_login_shell_path(fallback_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let probed = run_with_timeout(
        login_shell_path_probe(&shell),
        PATH_MARKER_END,
        SHELL_PROBE_TIMEOUT,
    )
    .and_then(|output| parse_path_probe_output(&output))
    .map(|line| std::env::split_paths(&line).collect::<Vec<_>>())
    .filter(|dirs| !dirs.is_empty());
    match probed {
        Some(dirs) => dirs,
        None => fallback_dirs.to_vec(),
    }
}

/// Fallback directories checked when the login-shell `PATH` probe fails,
/// per harness-detection.md's "PATH resolution".
fn default_fallback_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = home {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".npm-global/bin"));
        dirs.push(home.join(".volta/bin"));
        dirs.push(home.join(".bun/bin"));
        dirs.extend(nvm_node_bin_dirs(&home.join(".nvm/versions/node")));
    }
    dirs
}

/// `<nvm_node_versions>/*/bin` for every version directory that exists,
/// per harness-detection.md's fallback list: nvm has no single "current"
/// symlink guaranteed to exist, so every installed version's `bin` is a
/// candidate. Returns nothing when `nvm_node_versions` itself doesn't
/// exist (no nvm installed).
fn nvm_node_bin_dirs(nvm_node_versions: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(nvm_node_versions) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path().join("bin"))
        .collect()
}

/// `ToolLookup` that resolves against the user's login-shell `PATH`
/// instead of the process's own (minimal, under `launchd`) `PATH`. Intended
/// for the desktop app; the CLI and MCP server keep using
/// [`PathToolLookup`], whose process `PATH` already comes from a shell.
///
/// Caches the probed `PATH` in a per-instance `OnceLock` rather than a
/// process-wide `static`: the shell still spawns at most once per launch,
/// because `core_runtime::build_runtime_detect` builds exactly one
/// `LoginShellToolLookup` and every harness resolves against that same
/// instance (`harness-detection.md`: "one shell spawn per app start,
/// cached") - but a `static` cache also leaked across tests that construct
/// their own instance, and made the probe itself impossible to fake.
pub struct LoginShellToolLookup {
    search_dirs: OnceLock<Vec<PathBuf>>,
    probe: Box<dyn Fn() -> Vec<PathBuf> + Send + Sync>,
}

impl LoginShellToolLookup {
    /// Builds a lookup that probes the real login shell on first use.
    pub fn new() -> Self {
        Self::with_probe(|| read_login_shell_path(&default_fallback_dirs()))
    }

    /// As [`LoginShellToolLookup::new`], with `probe` standing in for the
    /// real login-shell spawn. Not `pub`: only `new` and this module's
    /// tests construct a lookup with a chosen probe.
    fn with_probe(probe: impl Fn() -> Vec<PathBuf> + Send + Sync + 'static) -> Self {
        LoginShellToolLookup {
            search_dirs: OnceLock::new(),
            probe: Box::new(probe),
        }
    }

    fn search_dirs(&self) -> &[PathBuf] {
        self.search_dirs.get_or_init(|| (self.probe)())
    }
}

impl Default for LoginShellToolLookup {
    fn default() -> Self {
        LoginShellToolLookup::new()
    }
}

impl ToolLookup for LoginShellToolLookup {
    fn find_binary(&self, name: &str) -> Option<PathBuf> {
        PathToolLookup::with_search_dirs(self.search_dirs().to_vec()).find_binary(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_executable(path: &Path) {
        fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn finds_an_executable_on_the_search_path() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("my-tool");
        write_executable(&bin);

        let lookup = PathToolLookup::with_search_dirs(vec![tmp.path().to_path_buf()]);
        let found = lookup.find_binary("my-tool").unwrap();
        assert_eq!(found, fs::canonicalize(&bin).unwrap());
    }

    #[test]
    fn ignores_a_non_executable_file_with_the_same_name() {
        let tmp = tempfile::tempdir().unwrap();
        let not_a_tool = tmp.path().join("not-a-tool");
        fs::write(&not_a_tool, b"plain text").unwrap();
        let mut perms = fs::metadata(&not_a_tool).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&not_a_tool, perms).unwrap();

        let lookup = PathToolLookup::with_search_dirs(vec![tmp.path().to_path_buf()]);
        assert!(lookup.find_binary("not-a-tool").is_none());
    }

    #[test]
    fn returns_none_when_no_search_dir_has_the_name() {
        let tmp = tempfile::tempdir().unwrap();
        let lookup = PathToolLookup::with_search_dirs(vec![tmp.path().to_path_buf()]);
        assert!(lookup.find_binary("does-not-exist").is_none());
    }

    #[test]
    fn stops_at_the_first_match_in_search_order() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        write_executable(&first.path().join("tool"));
        write_executable(&second.path().join("tool"));

        let lookup = PathToolLookup::with_search_dirs(vec![
            first.path().to_path_buf(),
            second.path().to_path_buf(),
        ]);
        let found = lookup.find_binary("tool").unwrap();
        assert_eq!(found, fs::canonicalize(first.path().join("tool")).unwrap());
    }

    /// `a_shell_banner_printed_before_the_marker_never_becomes_the_path_or_names_the_banner_it_kept`:
    /// rc-file banner text (`Welcome to zsh`, nvm's "Now using node ...")
    /// printed before `PATH_MARKER_START` must never be read as the `PATH`
    /// value. Fails if the parser takes the stdout's first line instead of
    /// the first non-empty line after the start marker.
    #[test]
    fn a_shell_banner_printed_before_the_marker_never_becomes_the_path_or_names_the_banner_it_kept()
    {
        let stdout = format!(
            "Welcome to zsh\nNow using node v20.11.0 (npm v10.2.4)\n{PATH_MARKER_START}\n/usr/bin:/bin:/opt/homebrew/bin\n{PATH_MARKER_END}\n"
        );

        let path = parse_path_probe_output(&stdout);

        assert_eq!(
            path.as_deref(),
            Some("/usr/bin:/bin:/opt/homebrew/bin"),
            "a banner line before the marker was read as PATH instead of the real value: {path:?}"
        );
    }

    /// `the_shell_probe_for_path_runs_once_per_launch_not_once_per_harness`:
    /// one `LoginShellToolLookup` resolving three different binaries (one
    /// per fictional harness) must run its probe exactly once, since
    /// `core_runtime::build_runtime_detect` builds exactly one instance and
    /// shares it across every harness. Fails if `find_binary` re-probes
    /// instead of reading the instance's cached `search_dirs`.
    #[test]
    fn the_shell_probe_for_path_runs_once_per_launch_not_once_per_harness() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_in_probe = calls.clone();
        let lookup = LoginShellToolLookup::with_probe(move || {
            calls_in_probe.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Vec::new()
        });

        lookup.find_binary("claude");
        lookup.find_binary("codex");
        lookup.find_binary("pi");

        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "expected exactly one probe run across three find_binary calls on the same lookup"
        );
    }

    /// `every_installed_nvm_node_version_gets_its_own_fallback_bin_dir_or_names_the_version_missed`:
    /// nvm has no single "current" symlink guaranteed to exist, so every
    /// installed version's `bin` directory must be a fallback candidate,
    /// not just one. Fails if only the first or last version directory is
    /// returned.
    #[test]
    fn every_installed_nvm_node_version_gets_its_own_fallback_bin_dir_or_names_the_version_missed()
    {
        let tmp = tempfile::tempdir().unwrap();
        let versions = tmp.path().join("versions/node");
        fs::create_dir_all(versions.join("v18.20.4/bin")).unwrap();
        fs::create_dir_all(versions.join("v20.11.0/bin")).unwrap();
        fs::write(versions.join("not-a-version-dir"), "").unwrap();

        let mut dirs = nvm_node_bin_dirs(&versions);
        dirs.sort();

        assert_eq!(
            dirs,
            vec![versions.join("v18.20.4/bin"), versions.join("v20.11.0/bin")],
            "expected one bin dir per installed version, got {dirs:?}"
        );
    }

    /// A missing `~/.nvm/versions/node` (no nvm installed) must yield no
    /// fallback dirs rather than an error.
    #[test]
    fn no_nvm_install_yields_no_fallback_dirs_or_names_the_error_it_raised_instead() {
        let tmp = tempfile::tempdir().unwrap();
        let versions = tmp.path().join("versions/node");

        assert!(nvm_node_bin_dirs(&versions).is_empty());
    }
}
