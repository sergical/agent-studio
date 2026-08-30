// ============================================================================
// Skills Module - dotagents_ledger
// Reads getsentry/dotagents' own bookkeeping files - `agents.lock` (what's
// actually resolved on disk, including the pinned commit) and `agents.toml`
// (what the user declared, including an optional ref) - so
// `skill_update_check` can tell a dotagents-managed skill's installed commit
// from its declared ref without re-deriving either from the skill directory
// itself. Pure file reads: missing files are not an error, just an empty
// result.
// ============================================================================

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

/// One skill declared in `agents.lock` (joined with `agents.toml` for its ref).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DotagentsSkill {
    pub name: String,
    pub source: String,
    /// "owner/repo", when `source` points at a GitHub repo. `None` for other
    /// hosts (or a source shape we don't recognize).
    pub github_repo: Option<String>,
    pub path: String,
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
/// `~/.agents`) and join them by skill name. Either file missing yields an
/// empty `Vec`, not an error - most machines have no dotagents skills at all.
pub fn read_dotagents_ledger(agents_dir: &Path) -> Result<Vec<DotagentsSkill>, String> {
    let lock = read_toml::<AgentsLock>(&agents_dir.join("agents.lock"))?.unwrap_or_default();
    let manifest =
        read_toml::<AgentsManifest>(&agents_dir.join("agents.toml"))?.unwrap_or_default();

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

/// Reads and parses `path` as TOML into `T`, or `Ok(None)` when `path` doesn't
/// exist.
fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    toml::from_str(&content)
        .map(Some)
        .map_err(|e| format!("Failed to parse {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

    #[test]
    fn missing_files_yield_empty_vec() {
        let tmp = tempfile::tempdir().unwrap();
        let skills = read_dotagents_ledger(tmp.path()).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn joins_lock_and_manifest_by_name_pinned_and_unpinned() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("agents.lock"),
            r#"
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
        .unwrap();
        fs::write(
            tmp.path().join("agents.toml"),
            r#"
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
        .unwrap();

        let mut skills = read_dotagents_ledger(tmp.path()).unwrap();
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
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("agents.lock"),
            r#"
[skills.some-wildcard-skill]
source = "getsentry/some-repo"
resolved_path = "skills/some-wildcard-skill"
resolved_commit = "3333333333333333333333333333333333cccc"
"#,
        )
        .unwrap();
        // No agents.toml at all - the wildcard case.
        let skills = read_dotagents_ledger(tmp.path()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].declared_ref, None);
    }
}
