// ============================================================================
// Skills Module - gh_cli
// One place that spawns the `gh` CLI and classifies its failures. Every
// caller (`skill_update_check`'s commit lookup, `skill_fork`'s tarball
// fetch, `skill_pack`'s repo-create/publish/read) shells out through
// `run_gh` instead of building its own `Command`, so "not logged in" is
// detected the same way everywhere: `gh`'s own error text tells the user to
// run `gh auth login`.
// ============================================================================

use std::path::Path;
use std::process::Command;

/// Why a `gh` invocation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhError {
    /// `gh`'s stderr told the user to run `gh auth login`.
    NotLoggedIn,
    /// Any other failure, with `gh`'s own message (stderr, falling back to
    /// stdout, falling back to a "failed to run gh" wrapper).
    Failed(String),
}

impl GhError {
    /// The message to show the user, regardless of which variant this is.
    pub fn message(&self) -> String {
        match self {
            GhError::NotLoggedIn => "Not logged into gh - run `gh auth login` first".to_string(),
            GhError::Failed(message) => message.clone(),
        }
    }
}

/// True when a `gh` error message indicates the user isn't logged in - the
/// same stderr substring `skill_update_check::is_not_logged_in` already
/// checked before this module existed.
fn is_not_logged_in(message: &str) -> bool {
    message.contains("gh auth login")
}

/// Runs `gh_bin <args>`, optionally piping `stdin` to it, and returns raw
/// stdout bytes on success. A non-zero exit is classified into
/// `GhError::NotLoggedIn` or `GhError::Failed` from `gh`'s own stderr (or
/// stdout, when stderr is empty).
pub(crate) fn run_gh(
    gh_bin: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> Result<Vec<u8>, GhError> {
    use std::io::Write;
    use std::process::Stdio;

    let mut cmd = Command::new(gh_bin);
    cmd.args(args);
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| GhError::Failed(format!("Failed to run gh: {e}")))?;

    if let Some(input) = stdin {
        if let Some(mut child_stdin) = child.stdin.take() {
            let _ = child_stdin.write_all(input);
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| GhError::Failed(format!("Failed to run gh: {e}")))?;

    if output.status.success() {
        return Ok(output.stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let message = if stderr.is_empty() { stdout } else { stderr };
    if is_not_logged_in(&message) {
        Err(GhError::NotLoggedIn)
    } else {
        Err(GhError::Failed(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_binary_is_a_failed_error_not_a_panic() {
        let err = run_gh(
            Path::new("/nonexistent/gh-binary"),
            &["auth", "status"],
            None,
        )
        .expect_err("missing binary must fail");
        assert!(matches!(err, GhError::Failed(_)));
    }

    #[test]
    fn not_logged_in_detection_matches_gh_auth_login_substring() {
        assert!(is_not_logged_in(
            "To get started with GitHub CLI, please run:  gh auth login"
        ));
        assert!(!is_not_logged_in("some other failure"));
    }

    #[test]
    fn run_gh_reports_the_binary_that_failed_to_spawn() {
        let err = run_gh(Path::new("/nonexistent/gh-binary"), &["api", "x"], None).unwrap_err();
        assert!(err.message().contains("Failed to run gh"));
    }
}
