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

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The exact selected dotagents rows that a Fork removes.  The full provider
/// documents remain outside this value so callers can retain unknown fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsDetachIntent {
    name: String,
    locked: toml::Value,
    declared: toml::Value,
}

/// Upstream identity recorded by dotagents.  This identifies the requested
/// fetch; it does not assert anything about fetched bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsForkSource {
    source: String,
    repo: String,
    path: String,
    commit: String,
    declared_ref: Option<String>,
}

impl DotagentsForkSource {
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn repo(&self) -> &str {
        &self.repo
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn commit(&self) -> &str {
        &self.commit
    }
    pub fn declared_ref(&self) -> Option<&str> {
        self.declared_ref.as_deref()
    }
}

impl DotagentsDetachIntent {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn from_documents(name: &str, lock: &str, manifest: &str) -> Result<Self, String> {
        if name.is_empty() || name.contains(['/', '\\']) || matches!(name, "." | ".." | "*") {
            return Err("Detach requires an exact skill name".into());
        }
        let lock: toml::Value = toml::from_str(lock).map_err(|error| error.to_string())?;
        let manifest: toml::Value = toml::from_str(manifest).map_err(|error| error.to_string())?;
        if lock.get("version").and_then(toml::Value::as_integer) != Some(1)
            || manifest.get("version").and_then(toml::Value::as_integer) != Some(1)
        {
            return Err("Fork requires version 1 provider documents".into());
        }
        let locked = lock
            .get("skills")
            .and_then(toml::Value::as_table)
            .and_then(|skills| skills.get(name))
            .cloned()
            .ok_or("Detach requires a locked skill")?;
        let mut selected = manifest
            .get("skills")
            .and_then(toml::Value::as_array)
            .ok_or("Detach requires a named manifest entry")?
            .iter()
            .filter(|skill| skill.get("name").and_then(toml::Value::as_str) == Some(name));
        let declared = selected
            .next()
            .cloned()
            .ok_or("Detach requires a named manifest entry")?;
        if selected.next().is_some() {
            return Err("Detach requires exactly one named manifest entry".into());
        }
        if !locked.is_table() || !declared.is_table() {
            return Err("Selected provider rows have an invalid shape".into());
        }
        Ok(Self {
            name: name.into(),
            locked,
            declared,
        })
    }

    pub fn fork_source(&self) -> Result<DotagentsForkSource, String> {
        let source = self
            .locked
            .get("source")
            .and_then(toml::Value::as_str)
            .ok_or("Fork source is missing")?;
        let repo = github_repo_from_source(source)
            .ok_or("Fork source must identify a GitHub repository")?;
        let commit = self
            .locked
            .get("resolved_commit")
            .and_then(toml::Value::as_str)
            .ok_or("Fork requires an installed commit")?;
        if !matches!(commit.len(), 40 | 64)
            || !commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("Fork requires a full lowercase installed commit ID".into());
        }
        let path = self.locked.get("resolved_path").map_or(Ok(""), |value| {
            value.as_str().ok_or("Fork source path must be a string")
        })?;
        let path = if path == "." { "" } else { path };
        if !path.is_empty()
            && (path.contains(['\\', '\0'])
                || path.split('/').any(|part| matches!(part, "" | "." | "..")))
        {
            return Err("Fork source path must be repository-relative".into());
        }
        let declared_ref = self
            .declared
            .get("ref")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or("Declared fork ref must be a string")
            })
            .transpose()?;
        Ok(DotagentsForkSource {
            source: source.into(),
            repo,
            path: path.into(),
            commit: commit.into(),
            declared_ref,
        })
    }

    /// Produce documents that retain every non-selected TOML value.
    pub fn propose_document_detach(
        &self,
        lock: &str,
        manifest: &str,
    ) -> Result<(String, String), String> {
        if Self::from_documents(&self.name, lock, manifest)? != *self {
            return Err("Detach proposal no longer matches the selected entries".into());
        }
        let mut lock_edit = lock
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| error.to_string())?;
        let mut manifest_edit = manifest
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| error.to_string())?;
        if lock_edit.to_string() != lock || manifest_edit.to_string() != manifest {
            return Err(
                "Provider syntax cannot be edited without unrelated formatting changes".into(),
            );
        }
        lock_edit
            .get_mut("skills")
            .and_then(toml_edit::Item::as_table_like_mut)
            .ok_or("Lock skills cannot be edited as a table")?
            .remove(&self.name);
        let rows = manifest_edit
            .get_mut("skills")
            .and_then(toml_edit::Item::as_array_of_tables_mut)
            .ok_or("Manifest skills cannot be edited as an array")?;
        let index = rows
            .iter()
            .position(|row| row.get("name").and_then(toml_edit::Item::as_str) == Some(&self.name))
            .ok_or("Detach proposal no longer matches the selected entries")?;
        rows.remove(index);
        Ok((lock_edit.to_string(), manifest_edit.to_string()))
    }
}

/// One skill declared in `agents.lock` (joined with `agents.toml` for its ref).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct AgentsLock {
    #[serde(default)]
    pub(crate) skills: HashMap<String, LockedSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LockedSkill {
    pub(crate) source: String,
    #[serde(default)]
    pub(crate) resolved_path: Option<String>,
    #[serde(default)]
    pub(crate) resolved_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct AgentsManifest {
    #[serde(default)]
    pub(crate) skills: Vec<ManifestSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManifestSkill {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) r#ref: Option<String>,
}

/// Complete selected TOML entries only; callers must independently retain the input scope.
pub(crate) fn manifest_refs(manifest: &AgentsManifest) -> BTreeMap<String, Option<String>> {
    manifest
        .skills
        .iter()
        .map(|skill| (skill.name.clone(), skill.r#ref.clone()))
        .collect()
}

pub fn github_repo_from_source(source: &str) -> Option<String> {
    let slug = if let Some(url) = source.strip_prefix("git:") {
        let path = [
            "https://github.com/",
            "http://github.com/",
            "ssh://git@github.com/",
            "ssh://github.com/",
        ]
        .iter()
        .find_map(|prefix| url.strip_prefix(prefix))?;
        path.trim_end_matches('/')
            .strip_suffix(".git")
            .unwrap_or(path.trim_end_matches('/'))
    } else {
        source
    };
    let (owner, repo) = slug.split_once('/')?;
    let valid_owner = !owner.is_empty()
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    let valid_repo = !repo.is_empty()
        && !matches!(repo, "." | "..")
        && repo
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    (valid_owner && valid_repo).then(|| slug.to_owned())
}

/// Read `agents.lock` and `agents.toml` from `agents_dir` (normally
/// `~/.agents`) and join them by skill name. Either file missing yields an
/// empty `Vec`, not an error - most machines have no dotagents skills at all.
pub fn read_dotagents_ledger(agents_dir: &Path) -> Result<Vec<DotagentsSkill>, String> {
    let lock = read_agents_lock(agents_dir)?.unwrap_or_default();
    let manifest = read_agents_manifest(agents_dir)?.unwrap_or_default();
    Ok(join_dotagents_ledger(lock, manifest))
}

/// Read each dotagents input independently. `None` means the named input was
/// absent; callers that need ownership certainty must preserve errors rather
/// than replacing them with empty data.
pub fn read_agents_lock(agents_dir: &Path) -> Result<Option<AgentsLock>, String> {
    read_toml(&agents_dir.join("agents.lock"), parse_agents_lock)
}

pub fn read_agents_manifest(agents_dir: &Path) -> Result<Option<AgentsManifest>, String> {
    read_toml(&agents_dir.join("agents.toml"), parse_agents_manifest)
}

pub(crate) fn parse_agents_lock(content: &str, path: &Path) -> Result<AgentsLock, String> {
    parse_toml(content, path)
}

pub(crate) fn parse_agents_manifest(content: &str, path: &Path) -> Result<AgentsManifest, String> {
    let manifest: AgentsManifest = parse_toml(content, path)?;
    let mut names = std::collections::HashSet::new();
    for skill in &manifest.skills {
        if !names.insert(&skill.name) {
            return Err(format!(
                "Duplicate skill name {} in {}",
                skill.name,
                path.display()
            ));
        }
    }
    Ok(manifest)
}

pub fn join_dotagents_ledger(lock: AgentsLock, manifest: AgentsManifest) -> Vec<DotagentsSkill> {
    let declared_refs = manifest_refs(&manifest);

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
    skills
}

/// Reads and parses `path` as TOML into `T`, or `Ok(None)` when `path` doesn't
/// exist.
fn read_toml<T>(
    path: &Path,
    parse: impl FnOnce(&str, &Path) -> Result<T, String>,
) -> Result<Option<T>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to read {}: {error}", path.display())),
    };
    parse(&content, path).map(Some)
}

fn parse_toml<T: for<'de> Deserialize<'de>>(content: &str, path: &Path) -> Result<T, String> {
    toml::from_str(content).map_err(|error| format!("Failed to parse {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn detach_refuses_duplicate_selected_manifest_rows() {
        let lock = "version = 1\n[skills.sample]\nsource = 'owner/repo'\n";
        let manifest = "version = 1\n[[skills]]\nname = 'sample'\nref = 'main'\n";
        let intent = DotagentsDetachIntent::from_documents("sample", lock, manifest).unwrap();
        for branch in ["main", "other"] {
            let duplicate = format!("{manifest}[[skills]]\nname = 'sample'\nref = '{branch}'\n");
            assert!(DotagentsDetachIntent::from_documents("sample", lock, &duplicate).is_err());
            assert!(intent.propose_document_detach(lock, &duplicate).is_err());
        }
    }

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
