//! Real `gh` CLI adapters for `skill_studio_core::skill_update_check`'s
//! three currency ports. Read-only `gh api` calls through the user's own
//! `gh` login, mirroring the desktop's `gh_cli.rs`; the app stores no
//! tokens. Codex has no plugin CLI, so [`GhPluginManifestLookup`] is Claude
//! Code marketplaces only, per plan.md unit 3.4.

use std::path::PathBuf;
use std::process::{Command, Output};

use skill_studio_core::error::{CoreError, ErrorCode};
use skill_studio_core::skill_update_check::{
    CommitInfo, CommitLookup, PluginManifestLookup, SourceTreeLookup,
};

use std::collections::HashMap;
use std::sync::Arc;

/// Runs `gh` and returns its raw `Output`, or the `std::io::Error` from
/// failing to spawn it at all. A trait, not a bare fn pointer, so tests can
/// script stdout/stderr/exit code without spawning a real `gh` process.
pub trait GhRunner: Send + Sync {
    fn run(&self, args: &[&str]) -> std::io::Result<Output>;
}

/// [`GhRunner`] backed by a real `gh` binary on disk.
pub struct RealGhRunner {
    pub gh_bin: PathBuf,
}

impl GhRunner for RealGhRunner {
    fn run(&self, args: &[&str]) -> std::io::Result<Output> {
        Command::new(&self.gh_bin).args(args).output()
    }
}

/// Runs `gh <args>` and returns stdout, or a [`CoreError`] built from
/// stderr (falling back to stdout, then a generic message) on a non-zero
/// exit or a failure to spawn `gh` at all.
fn run_gh(runner: &dyn GhRunner, args: &[&str]) -> Result<Vec<u8>, CoreError> {
    let output = runner
        .run(args)
        .map_err(|e| CoreError::new(ErrorCode::Unsupported, format!("failed to run gh: {e}")))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let message = if !output.stderr.is_empty() {
        String::from_utf8_lossy(&output.stderr).into_owned()
    } else if !output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        format!("gh exited with {:?}", output.status.code())
    };
    Err(CoreError::new(ErrorCode::Unsupported, message))
}

/// [`SourceTreeLookup`] over `gh api repos/<repo>/git/trees/HEAD?recursive=1`,
/// one call per repo, returning every subtree's SHA at once so a caller
/// checking many skills from the same repo never re-fetches it.
pub struct GhSourceTreeLookup {
    runner: Arc<dyn GhRunner>,
}

impl GhSourceTreeLookup {
    /// A `GhSourceTreeLookup` that shells the real `gh` binary at `gh_bin`.
    pub fn new(gh_bin: PathBuf) -> Self {
        Self {
            runner: Arc::new(RealGhRunner { gh_bin }),
        }
    }

    /// For tests: a `GhSourceTreeLookup` over a scripted [`GhRunner`].
    pub fn with_runner(runner: Arc<dyn GhRunner>) -> Self {
        Self { runner }
    }
}

impl SourceTreeLookup for GhSourceTreeLookup {
    fn tree_shas_at_head(&self, repo: &str) -> Result<HashMap<String, String>, CoreError> {
        let api_path = format!("repos/{repo}/git/trees/HEAD?recursive=1");
        // No `--jq` filter here (unlike the other two lookups): the response
        // must be inspected for `truncated` before its `tree` entries are
        // trusted, and `--jq` would already have thrown that field away.
        let stdout = run_gh(self.runner.as_ref(), &["api", &api_path])?;
        parse_tree_response(repo, &stdout)
    }
}

/// Parses a `gh api repos/<repo>/git/trees/HEAD?recursive=1` response body.
/// A `truncated: true` response means GitHub's recursive listing stopped
/// early - the caller cannot tell "not in the tree" from "not fetched yet"
/// for the paths past the cutoff, so this is an error naming the repo
/// rather than a partial (and silently misleading) map.
fn parse_tree_response(repo: &str, stdout: &[u8]) -> Result<HashMap<String, String>, CoreError> {
    let value: serde_json::Value = serde_json::from_slice(stdout).map_err(|e| {
        CoreError::new(
            ErrorCode::Unsupported,
            format!("{repo}: could not parse tree listing: {e}"),
        )
    })?;
    if value.get("truncated").and_then(serde_json::Value::as_bool) == Some(true) {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            format!("{repo}: tree listing truncated"),
        ));
    }
    let mut shas = HashMap::new();
    for entry in value
        .get("tree")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if entry.get("type").and_then(serde_json::Value::as_str) != Some("tree") {
            continue;
        }
        let (Some(path), Some(sha)) = (
            entry.get("path").and_then(serde_json::Value::as_str),
            entry.get("sha").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        shas.insert(path.to_string(), sha.to_string());
    }
    Ok(shas)
}

/// [`CommitLookup`] over `gh api repos/<repo>/commits?path=<path>&per_page=1`
/// - the same call the desktop's dotagents currency check already made.
pub struct GhCommitLookup {
    runner: Arc<dyn GhRunner>,
}

impl GhCommitLookup {
    /// A `GhCommitLookup` that shells the real `gh` binary at `gh_bin`.
    pub fn new(gh_bin: PathBuf) -> Self {
        Self {
            runner: Arc::new(RealGhRunner { gh_bin }),
        }
    }

    /// For tests: a `GhCommitLookup` over a scripted [`GhRunner`].
    pub fn with_runner(runner: Arc<dyn GhRunner>) -> Self {
        Self { runner }
    }
}

impl CommitLookup for GhCommitLookup {
    fn latest_commit(&self, repo: &str, path: &str) -> Result<Option<CommitInfo>, CoreError> {
        let api_path = format!(
            "repos/{repo}/commits?path={}&per_page=1",
            percent_encoding::utf8_percent_encode(path, percent_encoding::NON_ALPHANUMERIC)
        );
        // `@tsv` over `[.sha, .commit.committer.date]`, the same query the
        // desktop's own `GhCommitLookup` used before this ported it: one
        // call gets both the sha and the date, rather than a second lookup
        // just for "as of <date>".
        let stdout = run_gh(
            self.runner.as_ref(),
            &[
                "api",
                &api_path,
                "--jq",
                ".[0] | [.sha, .commit.committer.date] | @tsv",
            ],
        )?;
        let stdout = String::from_utf8_lossy(&stdout);
        // Only the trailing newline `gh` appends is stripped here, not a
        // full `trim()`: an empty sha with a date still present (`.[0]` had
        // no `sha` but did have a `commit.committer.date`, an edge case
        // `gh`'s `--jq` can produce) leaves a leading tab that `splitn`
        // below relies on to land the date in the second field rather than
        // the first - a plain `trim()` would eat that tab too and shift the
        // date into the sha slot, reporting a bogus "update available".
        let line = stdout.trim_end_matches(['\n', '\r']);
        if line.is_empty() {
            return Ok(None);
        }
        let mut parts = line.splitn(2, '\t');
        let sha = parts.next().unwrap_or_default().to_string();
        let committed_at = parts.next().map(str::to_string);
        if sha.is_empty() {
            Ok(None)
        } else {
            Ok(Some(CommitInfo { sha, committed_at }))
        }
    }
}

/// [`PluginManifestLookup`] over `gh api repos/<marketplace>/contents/...` -
/// left unresolvable ([`Ok(None)`]) until a marketplace's real manifest
/// layout is confirmed against a live account; see `issue-3.4-followup-a.md`.
/// Never guesses a version, so a plugin currency check reads `Unknown`
/// rather than a false "current" or "outdated".
pub struct GhPluginManifestLookup;

impl PluginManifestLookup for GhPluginManifestLookup {
    fn marketplace_version(
        &self,
        _marketplace: &str,
        _plugin: &str,
    ) -> Result<Option<String>, CoreError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::Mutex;

    fn output(status: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: ExitStatus::from_raw(status << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// Scripted `GhRunner`: one queued `Output` per call, in order, and a
    /// record of the `args` each call received.
    struct ScriptedGhRunner {
        outputs: Mutex<Vec<Output>>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl ScriptedGhRunner {
        fn new(outputs: Vec<Output>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into_iter().rev().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl GhRunner for ScriptedGhRunner {
        fn run(&self, args: &[&str]) -> std::io::Result<Output> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|s| (*s).to_string()).collect());
            Ok(self
                .outputs
                .lock()
                .unwrap()
                .pop()
                .expect("test script ran out of queued gh outputs"))
        }
    }

    /// Flow: `gh api` exits non-zero with a 403 rate-limit body on stderr.
    /// Expectation: `Err` whose message is neither "up to date" nor "update
    /// available" wording - a lookup failure must read as unknown, not as
    /// either currency state.
    /// A failure here means a non-zero exit was swallowed into `Ok`, or the
    /// error text leaked one of the currency labels.
    #[test]
    fn a_403_response_maps_to_an_error_or_names_the_leaked_currency_label() {
        let runner = ScriptedGhRunner::new(vec![output(
            1,
            "",
            r#"{"message":"API rate limit exceeded"}"#,
        )]);
        let lookup = GhSourceTreeLookup::with_runner(Arc::new(runner));
        let err = lookup.tree_shas_at_head("obra/write-tests").unwrap_err();
        assert!(err.message.contains("rate limit"));
        assert!(!err.message.to_lowercase().contains("up to date"));
        assert!(!err.message.to_lowercase().contains("update available"));
    }

    /// Flow: `gh` itself fails to spawn (offline, not installed).
    /// Expectation: the same `Err` shape as an API-level failure - the
    /// caller (`skills_sh_currency`) treats both as `Currency::Unknown`.
    /// A failure here means a spawn failure panics or is treated
    /// differently from an API error, instead of mapping to the same
    /// `CoreError`.
    #[test]
    fn a_spawn_failure_maps_to_the_same_error_shape_as_an_api_failure_or_names_the_difference() {
        struct FailingRunner;
        impl GhRunner for FailingRunner {
            fn run(&self, _args: &[&str]) -> std::io::Result<Output> {
                Err(std::io::Error::other("gh: command not found"))
            }
        }
        let lookup = GhSourceTreeLookup::with_runner(Arc::new(FailingRunner));
        let err = lookup.tree_shas_at_head("obra/write-tests").unwrap_err();
        assert_eq!(err.code, ErrorCode::Unsupported);
        assert!(err.message.contains("gh: command not found"));
    }

    /// Flow: `GhCommitLookup` against a path with no commits, where `gh`'s
    /// `@tsv` rendering of `[null, null]` is an empty (tab-only) line.
    /// Expectation: `Ok(None)`, not `Ok(Some(CommitInfo { sha: "", .. }))`.
    /// A failure here means an empty sha was treated as a real commit.
    #[test]
    fn commit_lookup_empty_sha_reads_as_no_commits_or_names_the_fake_sha() {
        let runner = ScriptedGhRunner::new(vec![output(0, "\t\n", "")]);
        let lookup = GhCommitLookup::with_runner(Arc::new(runner));
        let result = lookup
            .latest_commit("obra/write-tests", "skills/x")
            .unwrap();
        assert_eq!(result, None);
    }

    /// Flow: `gh`'s `@tsv` rendering of `[null, "<date>"]` - an empty sha
    /// column followed by a populated date column, the shape `.[0]` produces
    /// when a commit exists for the date field alone but `sha` came back
    /// null (an edge case a plain `trim()` mishandles: it would eat the
    /// leading tab along with the trailing newline, leaving one token that
    /// `splitn` reads as a non-empty sha).
    /// Expectation: `Ok(None)` - the same "no commits" result as an
    /// all-empty line, not `Ok(Some(CommitInfo { sha: "<date>", .. }))`.
    /// A failure here means the date shifted into the sha slot, which
    /// `dotagents_currency`/`fork_currency` would then compare against the
    /// installed commit and report a false "update available".
    #[test]
    fn commit_lookup_empty_sha_with_a_date_reads_as_no_commits_or_names_the_shifted_date() {
        let runner = ScriptedGhRunner::new(vec![output(0, "\t2026-02-01T00:00:00Z\n", "")]);
        let lookup = GhCommitLookup::with_runner(Arc::new(runner));
        let result = lookup
            .latest_commit("obra/write-tests", "skills/x")
            .unwrap();
        assert_eq!(result, None);
    }

    /// Flow: `GhCommitLookup` against a path with a real commit - `gh`'s
    /// `@tsv` line carries the sha and the committer date.
    /// Expectation: `Ok(Some(CommitInfo { sha, committed_at: Some(date) }))` -
    /// the date travels with the sha in one call, not a second lookup.
    /// A failure here means the date column was dropped or swapped with the
    /// sha.
    #[test]
    fn commit_lookup_reads_sha_and_committer_date_from_one_tsv_line() {
        let runner = ScriptedGhRunner::new(vec![output(0, "abc123\t2026-02-01T00:00:00Z\n", "")]);
        let lookup = GhCommitLookup::with_runner(Arc::new(runner));
        let result = lookup
            .latest_commit("obra/write-tests", "skills/x")
            .unwrap()
            .unwrap();
        assert_eq!(result.sha, "abc123");
        assert_eq!(result.committed_at.as_deref(), Some("2026-02-01T00:00:00Z"));
    }

    /// Flow: the tree endpoint's real JSON shape, `tree[]` entries plus a
    /// two-column parse of path and sha.
    /// Expectation: every `"tree"`-typed entry is keyed by path, `"blob"`
    /// entries are skipped.
    /// A failure here means a blob (file) entry leaked into the map, or a
    /// path/sha pair was dropped or swapped.
    #[test]
    fn tree_response_parses_tree_entries_by_path_or_names_the_dropped_entry() {
        let body = serde_json::json!({
            "sha": "head-sha",
            "truncated": false,
            "tree": [
                {"path": "skills/a", "type": "tree", "sha": "sha-a"},
                {"path": "skills/a/SKILL.md", "type": "blob", "sha": "sha-blob"},
                {"path": "skills/b", "type": "tree", "sha": "sha-b"},
            ]
        });
        let runner = ScriptedGhRunner::new(vec![output(0, &body.to_string(), "")]);
        let lookup = GhSourceTreeLookup::with_runner(Arc::new(runner));
        let shas = lookup.tree_shas_at_head("obra/write-tests").unwrap();
        assert_eq!(shas.get("skills/a"), Some(&"sha-a".to_string()));
        assert_eq!(shas.get("skills/b"), Some(&"sha-b".to_string()));
        assert_eq!(shas.len(), 2);
    }

    /// Flow: the tree endpoint reports `truncated: true` (a repo with more
    /// subtrees than one recursive listing covers).
    /// Expectation: `Err` naming the repo and "truncated", not an empty or
    /// partial map that would read as "no skill folder here".
    /// A failure here means truncation was ignored and a real skill folder
    /// past the cutoff silently read as `Currency::Unknown` with no error
    /// on record, or names the wrong repo.
    #[test]
    fn truncated_tree_response_is_an_error_or_names_the_silent_truncation() {
        let body = serde_json::json!({
            "sha": "head-sha",
            "truncated": true,
            "tree": [{"path": "skills/a", "type": "tree", "sha": "sha-a"}]
        });
        let runner = ScriptedGhRunner::new(vec![output(0, &body.to_string(), "")]);
        let lookup = GhSourceTreeLookup::with_runner(Arc::new(runner));
        let err = lookup.tree_shas_at_head("obra/write-tests").unwrap_err();
        assert!(err.message.contains("obra/write-tests"));
        assert!(err.message.contains("truncated"));
    }
}
