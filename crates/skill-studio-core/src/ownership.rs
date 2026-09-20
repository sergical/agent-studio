//! Ownership ledgers: the four files that decide which tool - if any - owns
//! a deployment's lifecycle ([`crate::identity::LifecycleOwnerKind`]).
//!
//! Ported from the desktop's `skill_ownership.rs` (ledger loading and
//! precedence), `dotagents_ledger.rs` (dotagents TOML shapes) and
//! `skill_fork_registry.rs` (the `forks`/`copies` buckets of
//! `skill-studio.json`), reading every byte through [`ScopeFs`] instead of
//! `std::fs` so the core never touches a path outside its scope. See
//! `docs/spec-core-primitives.md` section 13.4 for the precedence table.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::lock_file::{self, SkillLockFile};
use crate::ports::ScopeFs;

/// Largest ownership ledger file the core will read. Matches
/// [`lock_file::LOCK_FILE_MAX_BYTES`]; a file over this size is treated as
/// absent rather than partially parsed. Shared with
/// [`crate::tracked_projects`], which reads the same file.
pub(crate) const OWNERSHIP_LEDGER_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// `<home>/.agents/skill-studio.json` - Skill Studio's own bookkeeping file,
/// shared by [`read_home_registry`] and [`crate::tracked_projects`].
pub(crate) fn skill_studio_json_path(home: &Path) -> PathBuf {
    home.join(".agents").join("skill-studio.json")
}

/// One skill named in `agents.lock`, joined with whether `agents.toml` also
/// declares it by name. Mirrors the desktop's `DotagentsSkill`
/// (`dotagents_ledger.rs`), carrying only the fields `classify_owner` needs.
#[derive(Debug, Clone)]
pub(crate) struct DotagentsEntry {
    pub name: String,
    /// False for a skill pulled in by a wildcard install (`dotagents install
    /// --all`), which has an `agents.lock` row but no `[[skills]]` row in
    /// `agents.toml`.
    pub has_manifest_row: bool,
}

/// The skills.sh lock file and the dotagents ledger for one `.agents`
/// directory (the scope home's or one tracked project's).
#[derive(Debug, Clone)]
pub(crate) struct ScopeLedgers {
    pub lock: SkillLockFile,
    pub dotagents: Vec<DotagentsEntry>,
    /// Whether `agents.toml` or `agents.lock` exists at all, regardless of
    /// what it names. A dotagents install with no row for a given skill
    /// still makes that skill's owner ambiguous - see `classify_owner`'s
    /// shared-root carve-out - so presence is a separate fact from content.
    pub has_dotagents_files: bool,
    /// Skill names a project-scope `<project>/skills-lock.json` (schema
    /// version 1) names - a second, project-root source for skills.sh
    /// ownership alongside `lock` above, which only ever sees the shared
    /// `.skill-lock.json`. Always empty for the home scope, which has no
    /// such file.
    pub project_lock_skills: HashSet<String>,
}

/// Reads both ledgers under `agents_dir` (normally `<scope root>/.agents`),
/// plus `project_lock_path`'s skill names when the scope is a project (see
/// [`ScopeLedgers::project_lock_skills`]). A missing or unreadable file
/// yields an empty ledger rather than an error: most scopes have no
/// dotagents or skills.sh install at all, and a scan must still report
/// every other deployment.
pub(crate) fn read_scope_ledgers(
    fs: &dyn ScopeFs,
    agents_dir: &Path,
    project_lock_path: Option<&Path>,
) -> ScopeLedgers {
    let lock =
        lock_file::read_lock_file(fs, &agents_dir.join(".skill-lock.json")).unwrap_or_else(|_| {
            SkillLockFile {
                version: 3,
                skills: std::collections::HashMap::new(),
            }
        });
    let project_lock_skills = project_lock_path
        .map(|path| lock_file::read_project_lock_skill_names(fs, path))
        .unwrap_or_default();
    ScopeLedgers {
        lock,
        dotagents: read_dotagents_ledger(fs, agents_dir),
        has_dotagents_files: fs.symlink_metadata(&agents_dir.join("agents.toml")).is_ok()
            || fs.symlink_metadata(&agents_dir.join("agents.lock")).is_ok(),
        project_lock_skills,
    }
}

/// Reads `agents.lock`'s skill names, joined with whether each also has a
/// `[[skills]]` row in `agents.toml`. Matches the desktop's
/// `read_dotagents_ledger` (`dotagents_ledger.rs`), minus the fields
/// `classify_owner` doesn't need (`source`, `installed_commit`, ...).
fn read_dotagents_ledger(fs: &dyn ScopeFs, agents_dir: &Path) -> Vec<DotagentsEntry> {
    let manifest_names = dotagents_manifest_names(fs, agents_dir);
    dotagents_lock_names(fs, agents_dir)
        .into_iter()
        .map(|name| {
            let has_manifest_row = manifest_names.contains(&name);
            DotagentsEntry {
                name,
                has_manifest_row,
            }
        })
        .collect()
}

/// Reads `path` as a TOML document, or `None` when it is missing,
/// unreadable, over [`OWNERSHIP_LEDGER_MAX_BYTES`], or not valid TOML.
fn read_toml_document(fs: &dyn ScopeFs, path: &Path) -> Option<toml::Table> {
    let bytes = fs.read_capped(path, OWNERSHIP_LEDGER_MAX_BYTES).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    text.parse::<toml::Table>().ok()
}

/// `agents.lock`'s `[skills.<name>]` table keys - the set of skills
/// dotagents has actually resolved on disk.
fn dotagents_lock_names(fs: &dyn ScopeFs, agents_dir: &Path) -> Vec<String> {
    let Some(table) = read_toml_document(fs, &agents_dir.join("agents.lock")) else {
        return Vec::new();
    };
    table
        .get("skills")
        .and_then(toml::Value::as_table)
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default()
}

/// `agents.toml`'s `[[skills]]` array's `name` fields - the set of skills
/// the user declared by name, as opposed to pulling in via a wildcard.
fn dotagents_manifest_names(
    fs: &dyn ScopeFs,
    agents_dir: &Path,
) -> std::collections::HashSet<String> {
    let Some(table) = read_toml_document(fs, &agents_dir.join("agents.toml")) else {
        return std::collections::HashSet::new();
    };
    table
        .get("skills")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("name").and_then(toml::Value::as_str))
        .map(str::to_string)
        .collect()
}

/// One deployment created by Skill Studio's Copy installer, read back from
/// `skill-studio.json`'s `copies` map. Mirrors the desktop's
/// `CopyDeploymentRecord` (`skill_fork_registry.rs`), field for field.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CopyRecord {
    pub name: String,
    pub path: PathBuf,
    /// `"global"` or `"project"` - `InstallScope`'s wire form.
    pub scope: String,
    /// `"universal"` or `"per-harness"` - `SkillDestination`'s wire form.
    pub destination: String,
    #[serde(default)]
    pub project_path: Option<String>,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub disabled: bool,
}

/// One skill detached from its ledger via "Fork", read back from
/// `skill-studio.json`'s `forks` map. Mirrors the desktop's `ForkRecord`
/// (`skill_fork_registry.rs`), carrying only the fields the overlay in
/// `skill_refresh.rs`'s `apply_skill_snapshot_overlays` needs to find the
/// matching deployment.
#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct ForkRecord {
    /// Empty only for a legacy (pre registry-version-2) record, which must
    /// be matched by `skill_dir` alone.
    #[serde(default)]
    pub deployment_id: String,
    /// Empty means the default path, `<home>/.agents/skills/<name>`.
    #[serde(default)]
    pub skill_dir: PathBuf,
    /// The upstream repo this fork was cut from, for
    /// [`crate::skill_update_check::outdated`]'s currency check. Empty for a
    /// legacy record with no recorded upstream - such a fork stays
    /// `NotTracked` rather than being compared against nothing.
    #[serde(default)]
    pub repo: String,
    /// The upstream path within `repo`, paired with `repo` above.
    #[serde(default)]
    pub path: String,
    /// The commit the local copy was last synced from - the "installed"
    /// side of the fork's currency compare, pinned independently of
    /// whatever the dotagents/skills.sh ledger says for the same name.
    #[serde(default)]
    pub base_commit: String,
}

/// The `forks` and `copies` buckets of `~/.agents/skill-studio.json` -
/// Skill Studio's own bookkeeping file, which the desktop only ever reads or
/// writes at the scope home, never per project.
#[derive(Debug, Clone, Default)]
pub(crate) struct HomeRegistry {
    pub forks: BTreeMap<String, ForkRecord>,
    pub copies: BTreeMap<String, CopyRecord>,
}

/// Only the two buckets [`HomeRegistry`] carries; every other
/// `skill-studio.json` field (trials, parked skills, packs, ...) is out of
/// scope for ownership classification.
#[derive(Debug, Deserialize, Default)]
struct RawHomeRegistry {
    #[serde(default)]
    forks: BTreeMap<String, ForkRecord>,
    #[serde(default)]
    copies: BTreeMap<String, CopyRecord>,
}

/// Reads `<home>/.agents/skill-studio.json`. A missing, unreadable or
/// malformed file yields an empty registry - matching the desktop's
/// read-only callers (`read_fork_registry_or_default`), which downgrade
/// that same failure to "nothing recorded" rather than failing the scan.
pub(crate) fn read_home_registry(fs: &dyn ScopeFs, home: &Path) -> HomeRegistry {
    read_home_registry_result(fs, home).unwrap_or_default()
}

/// Like [`read_home_registry`], but a missing file is the only failure
/// downgraded to an empty registry; an unreadable or malformed file is
/// reported as `Err` instead of read as "no forks recorded" -
/// `skill_update_check::outdated`'s fork rule needs that distinction so a
/// broken registry surfaces as `Currency::Unknown` rather than the
/// `NotTracked` a fork with no registry row at all gets.
pub(crate) fn read_home_registry_result(
    fs: &dyn ScopeFs,
    home: &Path,
) -> Result<HomeRegistry, crate::error::CoreError> {
    let path = skill_studio_json_path(home);
    let bytes = match fs.read_capped(&path, OWNERSHIP_LEDGER_MAX_BYTES) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HomeRegistry::default()),
        Err(e) => return Err(crate::error::CoreError::io(path.clone(), e)),
    };
    let raw: RawHomeRegistry = serde_json::from_slice(&bytes).map_err(|e| {
        crate::error::CoreError::new(
            crate::error::ErrorCode::Io,
            format!("failed to parse skill-studio.json: {e}"),
        )
        .at(path.clone())
    })?;
    Ok(HomeRegistry {
        forks: raw.forks,
        copies: raw.copies,
    })
}
