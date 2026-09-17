//! Reads getsentry/dotagents' own bookkeeping files - `agents.lock` (what's
//! actually resolved on disk, including the pinned commit) and `agents.toml`
//! (what the user declared, including an optional ref) - so
//! `skill_update_check` can tell a dotagents-managed skill's installed commit
//! from its declared ref without re-deriving either from the skill directory
//! itself. Pure reads through [`crate::ports::ScopeFs`]: missing files are
//! not an error, just an empty result.
//!
//! Ported from the desktop app's `skills/dotagents_ledger.rs`. Like
//! [`crate::lock_file`], the core never resolves `~/.agents` itself: callers
//! pass the already-normalized `agents_dir`.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{CoreError, ErrorCode};
use crate::ports::ScopeFs;

/// Largest `agents.lock` or `agents.toml` the core will read. Larger is
/// treated as corrupt rather than silently truncated.
pub const DOTAGENTS_FILE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// One skill declared in `agents.lock` (joined with `agents.toml` for its ref).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DotagentsSkill {
    /// Skill name, the `agents.lock`/`agents.toml` table key.
    pub name: String,
    /// Source string as `agents.lock` recorded it (e.g. `"owner/repo"` or a
    /// `"git:https://..."` URL).
    pub source: String,
    /// "owner/repo", when `source` points at a GitHub repo. `None` for other
    /// hosts (or a source shape we don't recognize).
    pub github_repo: Option<String>,
    /// Resolved install path, relative to the skill's source.
    pub path: String,
    /// Commit `agents.lock` resolved this skill to, if recorded.
    pub installed_commit: Option<String>,
    /// The `ref` declared in `agents.toml` for this skill's `[[skills]]` row.
    /// `None` for an unpinned or wildcard (`--all`) entry.
    pub declared_ref: Option<String>,
    /// True when `agents.toml` has a `[[skills]]` row for this name at all -
    /// false for a wildcard (`--all`) entry, which `update_skill` re-installs
    /// with `dotagents install` instead of a per-skill `dotagents add`.
    pub has_manifest_row: bool,
}

#[derive(Debug, Deserialize, Default)]
struct AgentsLock {
    #[serde(default)]
    skills: HashMap<String, LockedSkill>,
}

#[derive(Debug, Deserialize)]
struct LockedSkill {
    source: String,
    #[serde(default)]
    resolved_path: Option<String>,
    #[serde(default)]
    resolved_commit: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct AgentsManifest {
    #[serde(default)]
    skills: Vec<ManifestSkill>,
}

#[derive(Debug, Deserialize)]
struct ManifestSkill {
    name: String,
    #[serde(default)]
    r#ref: Option<String>,
}

/// "owner/repo" -> `Some("owner/repo")`; `"git:https://github.com/o/r.git"` ->
/// `Some("o/r")`; any other host (or a source shape that isn't a plain repo
/// slug) -> `None`.
pub fn github_repo_from_source(source: &str) -> Option<String> {
    if let Some(url) = source.strip_prefix("git:") {
        let url = url.trim_end_matches(".git");
        let after_host = url.split("github.com/").nth(1)?;
        let (owner, repo) = after_host.trim_end_matches('/').split_once('/')?;
        return if owner.is_empty() || repo.is_empty() {
            None
        } else {
            Some(format!("{owner}/{repo}"))
        };
    }

    if source.contains("://") {
        return None; // some other host's URL form, not a plain "owner/repo" slug
    }
    let parts: Vec<&str> = source.split('/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        Some(source.to_string())
    } else {
        None
    }
}

/// Read `agents.lock` and `agents.toml` from `agents_dir` (normally
/// `~/.agents`) through `fs` and join them by skill name. Either file
/// missing yields an empty `Vec`, not an error - most machines have no
/// dotagents skills at all.
pub fn read_dotagents_ledger(
    fs: &dyn ScopeFs,
    agents_dir: &Path,
) -> Result<Vec<DotagentsSkill>, CoreError> {
    let lock = read_toml::<AgentsLock>(fs, &agents_dir.join("agents.lock"))?.unwrap_or_default();
    let manifest =
        read_toml::<AgentsManifest>(fs, &agents_dir.join("agents.toml"))?.unwrap_or_default();

    let declared_refs: HashMap<String, Option<String>> = manifest
        .skills
        .into_iter()
        .map(|s| (s.name, s.r#ref))
        .collect();

    let mut skills: Vec<DotagentsSkill> = lock
        .skills
        .into_iter()
        .map(|(name, locked)| {
            let github_repo = github_repo_from_source(&locked.source);
            let manifest_row = declared_refs.get(&name);
            let declared_ref = manifest_row.cloned().flatten();
            let has_manifest_row = manifest_row.is_some();
            DotagentsSkill {
                name,
                source: locked.source,
                github_repo,
                path: locked.resolved_path.unwrap_or_default(),
                installed_commit: locked.resolved_commit,
                declared_ref,
                has_manifest_row,
            }
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

/// Reads and parses `path` as TOML into `T` through `fs`, or `Ok(None)` when
/// `path` doesn't exist.
fn read_toml<T: for<'de> Deserialize<'de>>(
    fs: &dyn ScopeFs,
    path: &Path,
) -> Result<Option<T>, CoreError> {
    let bytes = match fs.read_capped(path, DOTAGENTS_FILE_MAX_BYTES) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(CoreError::io(path, e)),
    };
    let text = std::str::from_utf8(&bytes).map_err(|e| {
        CoreError::new(
            ErrorCode::Io,
            format!("{} is not valid UTF-8: {e}", path.display()),
        )
        .at(path)
    })?;
    toml::from_str(text).map(Some).map_err(|e| {
        CoreError::new(
            ErrorCode::Io,
            format!("failed to parse {}: {e}", path.display()),
        )
        .at(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureBuilder;

    #[test]
    fn github_repo_from_source_handles_plain_slug() {
        assert_eq!(
            github_repo_from_source("getsentry/agent-browser"),
            Some("getsentry/agent-browser".to_string())
        );
    }

    #[test]
    fn github_repo_from_source_handles_git_url() {
        assert_eq!(
            github_repo_from_source("git:https://github.com/getsentry/agent-browser.git"),
            Some("getsentry/agent-browser".to_string())
        );
    }

    #[test]
    fn github_repo_from_source_rejects_non_github_host() {
        assert_eq!(
            github_repo_from_source("git:https://gitlab.com/getsentry/agent-browser.git"),
            None
        );
    }

    /// Flow: a home with no `agents.toml`/`agents.lock` at all (the common
    /// case - most machines have no dotagents skills).
    /// Expectation: `read_dotagents_ledger` returns an empty `Vec`.
    /// A failure here means a fresh install throws instead of reading as
    /// empty, per shared-root.md's "missing files mean empty, not error".
    #[test]
    fn dotagents_ledger_missing_files_read_as_empty_not_error_or_names_the_thrown_error() {
        let fs = FixtureBuilder::new().dir("/home/.agents").build_fs();
        let skills = read_dotagents_ledger(&fs, Path::new("/home/.agents")).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn joins_lock_and_manifest_by_name_pinned_and_unpinned() {
        let fs = FixtureBuilder::new()
            .dir("/home/.agents")
            .file(
                "/home/.agents/agents.lock",
                br#"
[skills.agent-browser]
source = "getsentry/agent-browser"
resolved_path = "skills/agent-browser"
resolved_commit = "1111111111111111111111111111111111aaaa"

[skills.find-bugs]
source = "git:https://github.com/getsentry/find-bugs.git"
resolved_path = "skills/find-bugs"
resolved_commit = "2222222222222222222222222222222222bbbb"
"#,
            )
            .file(
                "/home/.agents/agents.toml",
                br#"
[[skills]]
name = "agent-browser"
source = "getsentry/agent-browser"
path = "skills/agent-browser"
ref = "1111111111111111111111111111111111aaaa"

[[skills]]
name = "find-bugs"
source = "git:https://github.com/getsentry/find-bugs.git"
path = "skills/find-bugs"
"#,
            )
            .build_fs();

        let mut skills = read_dotagents_ledger(&fs, Path::new("/home/.agents")).unwrap();
        skills.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "agent-browser");
        assert_eq!(
            skills[0].github_repo,
            Some("getsentry/agent-browser".to_string())
        );
        assert_eq!(
            skills[0].declared_ref,
            Some("1111111111111111111111111111111111aaaa".to_string())
        );
        assert!(skills[0].has_manifest_row);
        assert_eq!(skills[1].name, "find-bugs");
        assert_eq!(
            skills[1].github_repo,
            Some("getsentry/find-bugs".to_string())
        );
        assert_eq!(skills[1].declared_ref, None);
        assert!(skills[1].has_manifest_row);
    }

    #[test]
    fn wildcard_entry_has_no_manifest_row() {
        let fs = FixtureBuilder::new()
            .dir("/home/.agents")
            .file(
                "/home/.agents/agents.lock",
                br#"
[skills.some-wildcard-skill]
source = "getsentry/some-repo"
resolved_path = "skills/some-wildcard-skill"
resolved_commit = "3333333333333333333333333333333333cccc"
"#,
            )
            // No agents.toml at all - the wildcard case.
            .build_fs();
        let skills = read_dotagents_ledger(&fs, Path::new("/home/.agents")).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].declared_ref, None);
    }
}
