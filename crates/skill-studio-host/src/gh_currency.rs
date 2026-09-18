//! Real `gh` CLI adapters for `skill_studio_core::skill_update_check`'s
//! three currency ports. Read-only `gh api` calls through the user's own
//! `gh` login, mirroring the desktop's `gh_cli.rs`; the app stores no
//! tokens. Codex has no plugin CLI, so [`GhPluginManifestLookup`] is Claude
//! Code marketplaces only, per plan.md unit 3.4.

use std::path::PathBuf;
use std::process::Command;

use skill_studio_core::error::{CoreError, ErrorCode};
use skill_studio_core::skill_update_check::{CommitLookup, PluginManifestLookup, SourceTreeLookup};

use std::collections::HashMap;

/// Runs `gh <args>` and returns stdout, or a [`CoreError`] built from
/// stderr (falling back to stdout, then a generic message) on a non-zero
/// exit or a failure to spawn `gh` at all.
fn run_gh(gh_bin: &std::path::Path, args: &[&str]) -> Result<Vec<u8>, CoreError> {
    let output = Command::new(gh_bin)
        .args(args)
        .output()
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
    /// Resolved path of the `gh` binary.
    pub gh_bin: PathBuf,
}

impl SourceTreeLookup for GhSourceTreeLookup {
    fn tree_shas_at_head(&self, repo: &str) -> Result<HashMap<String, String>, CoreError> {
        let api_path = format!("repos/{repo}/git/trees/HEAD?recursive=1");
        let stdout = run_gh(
            &self.gh_bin,
            &[
                "api",
                &api_path,
                "--jq",
                r#".tree[] | select(.type == "tree") | [.path, .sha] | @tsv"#,
            ],
        )?;
        let text = String::from_utf8_lossy(&stdout);
        let mut shas = HashMap::new();
        for line in text.lines() {
            if let Some((path, sha)) = line.split_once('\t') {
                shas.insert(path.to_string(), sha.to_string());
            }
        }
        Ok(shas)
    }
}

/// [`CommitLookup`] over `gh api repos/<repo>/commits?path=<path>&per_page=1`
/// - the same call the desktop's dotagents currency check already made.
pub struct GhCommitLookup {
    /// Resolved path of the `gh` binary.
    pub gh_bin: PathBuf,
}

impl CommitLookup for GhCommitLookup {
    fn latest_commit(&self, repo: &str, path: &str) -> Result<Option<String>, CoreError> {
        let api_path = format!(
            "repos/{repo}/commits?path={}&per_page=1",
            percent_encoding::utf8_percent_encode(path, percent_encoding::NON_ALPHANUMERIC)
        );
        let stdout = run_gh(&self.gh_bin, &["api", &api_path, "--jq", ".[0].sha"])?;
        let sha = String::from_utf8_lossy(&stdout).trim().to_string();
        if sha.is_empty() || sha == "null" {
            Ok(None)
        } else {
            Ok(Some(sha))
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
