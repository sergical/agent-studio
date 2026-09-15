// ============================================================================
// Skills Module - skill_fork_registry
// Reads and writes `~/.agents/skill-studio.json`, the one Skill-Studio-owned
// file inside `~/.agents` - the app never touches `agents.toml`,
// `agents.lock`, or `.skill-lock.json` itself, those belong to the owning
// CLI. Tracks which skills have been detached from their ledger ("forked")
// so local edits survive `dotagents sync` / `npx skills update`, plus a
// `trials` bucket for "Try for 24 hours" installs (see `skill_trial`), a
// `parked` bucket for skills disabled globally (see `skill_park`), and a
// `harness_disabled` bucket for the one per-harness disable that has no
// native config to read back from (Claude Code - see `skill_harness_disable`).
// A missing file yields a
// default (empty) registry; an unreadable or malformed one is an error for
// every caller, since silently treating it as empty would erase every
// recorded fork on the next write.
// ============================================================================

use crate::skill_document_write::DocumentWriteFailure;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::skill_deployment::InstallScope;
use crate::skill_deployment::SkillDestination;
use crate::skill_provenance::SourceKind;

fn path_is_empty(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

/// Which CLI a forked skill was originally managed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OriginTool {
    Dotagents,
    SkillsSh,
}

/// How `add_skill` installed a skill - shared by `AddSkillRequest.method` and
/// `TrialRecord.method`, since a trial's expiry step needs to know which tool
/// (if any) owns the skill it's about to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AddMethod {
    Dotagents,
    SkillsSh,
    Copy,
}

/// Which scope a trial (or an `add_skill` request) targeted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrialScope {
    Global,
    Project,
}

/// Durable state for trial expiry. `Expiring` prevents an interrupted CLI
/// removal from matching a later installation at the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TrialStatus {
    #[default]
    Active,
    Expiring,
    RecoveryRequired,
}

/// One "Try for 24 hours" install, tracked so `skill_trial`'s expiry loop
/// knows when to remove it and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrialRecord {
    /// Stable identity of the exact deployment this trial owns. Empty only
    /// for records written before registry version 2.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub deployment_id: String,
    pub started_at: String,
    pub expires_at: String,
    #[serde(default)]
    pub status: TrialStatus,
    pub method: AddMethod,
    pub scope: TrialScope,
    #[serde(default)]
    pub project_path: Option<String>,
    /// The exact directory `add_skill` created for this trial - expiry
    /// trashes and removes this path directly instead of recomputing it
    /// from `scope`/`project_path`, which was wrong for `skills-sh` trials
    /// (that method never writes the shared `.agents/skills` folder).
    #[serde(default)]
    pub skill_dir: PathBuf,
    /// Recursive content fingerprint recorded immediately after install.
    /// Empty only for legacy records, which expiry must not mutate.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub deployment_fingerprint: String,
    /// The per-skill Claude Code symlink `add_skill` created for this trial,
    /// if any - `None` when Claude Code wasn't selected or the whole-dir
    /// symlink already covered it.
    #[serde(default)]
    pub claude_link: Option<PathBuf>,
    /// Raw target of `claude_link` at install time. Missing only from legacy
    /// records, which expiry refuses when a Claude link is present.
    #[serde(default)]
    pub claude_link_target: Option<PathBuf>,
}

/// The `trials` map key for a given scope: `"global/<name>"` or
/// `"project/<name>"` - lets the same skill name be on trial globally and in
/// a project at the same time, and lets `keep_skill_trial`/expiry key back
/// into the map unambiguously.
pub fn trial_key(scope: TrialScope, name: &str) -> String {
    match scope {
        TrialScope::Global => format!("global/{name}"),
        TrialScope::Project => format!("project/{name}"),
    }
}

/// Registry key used by all new trial records.
pub fn deployment_trial_key(deployment_id: &str) -> String {
    format!("deployment/{deployment_id}")
}

/// The skill name embedded in a `trials` map key, e.g. `"global/find-bugs"`
/// -> `"find-bugs"`. Falls back to the whole key for anything that doesn't
/// look like one `trial_key` produced (there shouldn't be any).
pub fn name_from_trial_key(key: &str) -> &str {
    key.split_once('/').map(|(_, name)| name).unwrap_or(key)
}

/// One forked skill's provenance, enough to reinstall it from its origin
/// (`unfork_skill`) or to fetch its upstream at a specific commit
/// (`pull_fork_upstream`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkRecord {
    /// Global Universal deployment detached by this fork. Empty only for a
    /// legacy record, which callers must resolve by its exact local path.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub deployment_id: String,
    #[serde(default, skip_serializing_if = "path_is_empty")]
    pub skill_dir: PathBuf,
    pub forked_at: String,
    pub origin_tool: OriginTool,
    /// The exact source string the owning CLI would reinstall from -
    /// `agents.lock`'s `source` for dotagents, the lock file's `source` for
    /// skills.sh.
    pub origin_source: String,
    pub repo: String,
    pub path: String,
    /// The `ref` dotagents had declared for this skill, if any. `None` for
    /// skills.sh forks and unpinned dotagents forks.
    pub declared_ref: Option<String>,
    /// The commit the local copy was last synced from - the "base" of the
    /// three-way merge `pull_fork_upstream` runs.
    pub base_commit: String,
}

/// One skill parked (disabled globally) via `skill_park::park_skill` - see
/// that module for the mechanics. `source_kind` is the skill's `SourceKind`
/// at the time it was parked, so the snapshot can still label it correctly
/// even though a parked skill has no deployment for `classify_source_kind`
/// to look at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParkedRecord {
    /// Parked deployment identity and exact directory. Empty only for
    /// registry version 1 records.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub deployment_id: String,
    #[serde(default, skip_serializing_if = "path_is_empty")]
    pub skill_dir: PathBuf,
    pub parked_at: String,
    pub source_kind: SourceKind,
    /// The per-skill Claude Code symlink that was removed when parking, if
    /// any - `unpark_skill` recreates it at this exact path.
    #[serde(default)]
    pub claude_link: Option<PathBuf>,
}

/// One first-class agent's per-skill disable that has no native config to
/// read back, tracked here instead - currently only Claude Code (removing
/// its per-skill symlink), since Codex and OpenCode read their own disable
/// state straight from `~/.codex/config.toml` / `opencode.json`. See
/// `skill_harness_disable`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeLinkRemoved {
    /// Exact Claude Code deployment whose link was removed. Empty only for
    /// registry version 1 records.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub deployment_id: String,
    /// The symlink's original target, so re-enabling can recreate it exactly
    /// (relative, as `maybe_claude_code_symlink` creates it).
    pub link_target: PathBuf,
}

/// One skill bundled into a pack: `name` is its directory name, `path` is
/// the exact deployment directory it was bundled from - see
/// `skill_pack::resolve_members`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackMember {
    pub name: String,
    pub path: PathBuf,
}

/// One share pack created via `skill_pack::create_skill_pack`, keyed by pack
/// name in `ForkRegistry.packs`. `dir` and the member list are the app's own
/// bookkeeping; the pack's `agents.toml`/`README.md`/`skills/` tree under
/// `dir` is the actual dotagents-compatible payload - see `skill_pack`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackRecord {
    pub created_at: String,
    pub dir: PathBuf,
    /// `None` until `publish_skill_pack` succeeds for the first time.
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub members: Vec<PackMember>,
    /// The pre-`members` shape: a plain skill-name list with no deployment
    /// path. Never written by new code; `skill_pack::record_members` maps
    /// each name to `~/.agents/skills/<name>` once `home` is known, since
    /// serde can't do that at parse time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
}

/// One deployment created by Skill Studio's Copy installer. The deployment
/// ID is also the `copies` map key; the repeated identity fields make a
/// malformed or stale record fail closed during discovery and removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyDeploymentRecord {
    pub deployment_id: String,
    pub name: String,
    pub path: PathBuf,
    pub scope: InstallScope,
    pub destination: SkillDestination,
    pub slot: String,
    #[serde(default)]
    pub project_path: Option<String>,
    /// Discovery-compatible strong content hash recorded at install time.
    /// Empty only for legacy records, which destructive mutations refuse.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_hash: String,
    /// True when the exact copy is stored under `.skill-studio-disabled`.
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Serialize)]
pub(crate) enum RegistryOwnerRecord<'a> {
    Fork(&'a ForkRecord),
    Copy(&'a CopyDeploymentRecord),
}

impl RegistryOwnerRecord<'_> {
    pub(crate) fn revision(&self) -> Option<String> {
        let bytes = serde_json::to_vec(&("registry-owner-v1", self)).ok()?;
        Some(crate::skill_frontmatter_repair::content_fingerprint(&bytes))
    }
}

/// `~/.agents/skill-studio.json`'s shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkRegistry {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub forks: BTreeMap<String, ForkRecord>,
    /// "Try for 24 hours" installs, keyed by skill name - see `skill_trial`.
    #[serde(default)]
    pub trials: BTreeMap<String, TrialRecord>,
    /// Skills parked (disabled globally) via `skill_park`, keyed by name.
    #[serde(default)]
    pub parked: BTreeMap<String, ParkedRecord>,
    /// Per-harness disables that need a Skill-Studio-owned record rather than
    /// being read back from the harness's own config, keyed by skill name
    /// then by harness `cli_name` (currently only `"claude-code"`). See
    /// `skill_harness_disable`.
    #[serde(default)]
    pub harness_disabled: BTreeMap<String, BTreeMap<String, ClaudeLinkRemoved>>,
    /// Share packs created via `skill_pack`, keyed by pack name.
    #[serde(default)]
    pub packs: BTreeMap<String, PackRecord>,
    /// Exact deployments created by the Copy installer, keyed by deployment
    /// ID. Absent in registry versions 1 and 2; those installs remain manual
    /// because Copy ownership is never inferred from directory topology.
    #[serde(default)]
    pub copies: BTreeMap<String, CopyDeploymentRecord>,
    /// skills.sh /api/v1 bearer token; absent until the user configures one
    /// (the developer override - see `api::resolve_skills_sh_access`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_sh_api_key: Option<String>,
    /// The local Skill Studio server's base URL (no `/api/v1` suffix), used
    /// for discovery instead of skills.sh directly when `skills_sh_api_key`
    /// is absent. Absent means the default `http://127.0.0.1:8787`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// The macOS application name "Open in editor" hands a path to, without
    /// the `.app` suffix - `"Cursor"`, `"Visual Studio Code"`. Absent means
    /// the system default for the file's type. See `skill_editor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_editor: Option<String>,
    /// Normalized GitHub `owner/repo` and `git:<url>` identities the user
    /// has explicitly trusted for a later dotagents Add Skill retry. Empty
    /// by default: `kentcdodds/kcd-skills` still needs confirmation.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub trusted_dotagents_sources: BTreeSet<String>,
}

pub const CURRENT_REGISTRY_VERSION: u32 = 4;

fn default_version() -> u32 {
    CURRENT_REGISTRY_VERSION
}

// `#[derive(Default)]` would use `u32`/`Value`'s own `Default` (0 / Null)
// instead of the `#[serde(default = "...")]` functions above, so a freshly
// created registry would round-trip differently than one that was never
// read from disk. Implement it by hand to keep the two in sync.
impl Default for ForkRegistry {
    fn default() -> Self {
        ForkRegistry {
            version: default_version(),
            forks: BTreeMap::new(),
            trials: BTreeMap::new(),
            parked: BTreeMap::new(),
            harness_disabled: BTreeMap::new(),
            packs: BTreeMap::new(),
            copies: BTreeMap::new(),
            skills_sh_api_key: None,
            server_url: None,
            preferred_editor: None,
            trusted_dotagents_sources: BTreeSet::new(),
        }
    }
}

/// `~/.agents/skill-studio.json`.
pub fn fork_registry_path(home: &Path) -> PathBuf {
    home.join(".agents").join("skill-studio.json")
}

/// `<app data>/skill-studio/forks/<name>/base` - the last-synced snapshot of
/// a forked skill, used as the "base" side of `pull_fork_upstream`'s
/// three-way merge.
pub fn fork_snapshot_dir(app_data: &Path, name: &str) -> PathBuf {
    app_data
        .join("skill-studio")
        .join("forks")
        .join(name)
        .join("base")
}

/// Read the registry: a missing file yields a fresh default one, but an
/// unreadable or malformed file is an `Err` - a mutating command (fork/pull/
/// unfork/remove) must not treat a broken file as empty, since writing that
/// back out would silently erase every recorded fork.
pub fn read_fork_registry(home: &Path) -> Result<ForkRegistry, String> {
    Ok(read_fork_registry_optional(home)?.unwrap_or_default())
}

pub(crate) fn read_fork_registry_optional(home: &Path) -> Result<Option<ForkRegistry>, String> {
    let path = fork_registry_path(home);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    parse_fork_registry(&content, &path).map(Some)
}

pub(crate) fn parse_fork_registry(content: &str, path: &Path) -> Result<ForkRegistry, String> {
    serde_json::from_str(content).map_err(|_| {
        format!(
            "{} is malformed; repair the registry before continuing",
            path.display()
        )
    })
}

/// A trial record selected by a caller-authorized desktop Keep or removal operation.
#[cfg(unix)]
pub enum TrialRemoval<'a> {
    Deployment {
        deployment_id: &'a str,
        name: &'a str,
        scope: TrialScope,
        skill_dir: &'a Path,
    },
    RecoveryRequired {
        deployment_id: &'a str,
    },
}

/// Removes selected trial metadata under an exclusive scoped registry lease.
/// The missing-registry check uses ambient `try_exists`; existing registry reads
/// and publication use the scope. Skill files are never opened for writing.
/// Unrecognized registry fields survive.
#[cfg(unix)]
pub fn remove_selected_trial(home: &Path, selection: TrialRemoval<'_>) -> Result<bool, String> {
    use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
    use crate::skill_document_target::SkillRegistryTarget;
    use crate::skill_scope::SkillReadScope;

    let path = fork_registry_path(home);
    if !path.try_exists().map_err(|error| error.to_string())? {
        return Ok(false);
    }
    let parent = path.parent().ok_or("Registry has no parent")?;
    let scope = SkillReadScope::bind(&[parent.to_path_buf()]).map_err(|error| error.to_string())?;
    let guard = CoordinationPlan::new(
        vec![DirectoryEffect::entry(
            path.clone(),
            CoordinationMode::Exclusive,
        )],
        Some(std::time::Duration::from_secs(5)),
    )
    .map_err(|error| error.to_string())?
    .acquire()
    .map_err(|error| error.to_string())?;
    let mut lease = guard
        .finalize_write(&scope, std::slice::from_ref(&path))
        .map_err(|error| error.to_string())?;
    let original = lease
        .read(&path, 8 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    let registry = parse_fork_registry(
        std::str::from_utf8(&original).map_err(|_| "Fork registry is not UTF-8")?,
        &path,
    )?;
    let keys = match selection {
        TrialRemoval::Deployment {
            deployment_id,
            name,
            scope,
            skill_dir,
        } => {
            let exact = deployment_trial_key(deployment_id);
            let legacy = trial_key(scope, name);
            let mut keys = vec![exact];
            if registry
                .trials
                .get(&legacy)
                .is_some_and(|trial| trial.skill_dir == skill_dir)
            {
                keys.push(legacy);
            }
            keys
        }
        TrialRemoval::RecoveryRequired { deployment_id } => {
            let key = deployment_trial_key(deployment_id);
            if !registry
                .trials
                .get(&key)
                .is_some_and(|trial| trial.status == TrialStatus::RecoveryRequired)
            {
                return Ok(false);
            }
            vec![key]
        }
    };
    let mut document: serde_json::Value =
        serde_json::from_slice(&original).map_err(|error| error.to_string())?;
    let Some(trials) = document
        .get_mut("trials")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return Ok(false);
    };
    let mut changed = false;
    for key in keys {
        changed |= trials.remove(&key).is_some();
    }
    if !changed {
        return Ok(false);
    }
    let proposed = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
    SkillRegistryTarget::bind(parent)?
        .replace(&mut lease, &original, &proposed)
        .map_err(|error| error.to_string())?;
    lease.revalidate().map_err(|error| error.to_string())?;
    Ok(true)
}

/// Publishes the trial expiry changes between two registry snapshots. Only
/// selected `trials` and `copies` entries are changed; the current document is
/// read under the scoped lease so unrelated edits and unrecognized JSON fields
/// survive. A selected known record must still match the expected snapshot.
#[cfg(unix)]
pub enum TrialExpiryPublication {
    Published {
        rollback: TrialExpiryRollback,
    },
    PublishedWithDurabilityError {
        error: String,
        rollback: TrialExpiryRollback,
    },
}

/// Raw selected-record preimage retained only until a failed expiry operation
/// either rolls back or leaves its durable state for a later retry.
#[cfg(unix)]
pub struct TrialExpiryRollback {
    original: Vec<u8>,
    before_trials: BTreeMap<String, TrialRecord>,
    after_trials: BTreeMap<String, TrialRecord>,
    before_copies: BTreeMap<String, CopyDeploymentRecord>,
    after_copies: BTreeMap<String, CopyDeploymentRecord>,
}

#[cfg(unix)]
fn trial_expiry_rollback(
    original: Vec<u8>,
    before: &ForkRegistry,
    after: &ForkRegistry,
) -> TrialExpiryRollback {
    TrialExpiryRollback {
        original,
        before_trials: before.trials.clone(),
        after_trials: after.trials.clone(),
        before_copies: before.copies.clone(),
        after_copies: after.copies.clone(),
    }
}

#[cfg(unix)]
pub fn publish_trial_expiry(
    home: &Path,
    expected: &ForkRegistry,
    proposed: &ForkRegistry,
) -> Result<TrialExpiryPublication, String> {
    use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
    use crate::skill_scope::SkillReadScope;

    let (changed_trials, changed_copies) = changed_trial_expiry_records(expected, proposed)?;
    if changed_trials.is_empty() && changed_copies.is_empty() {
        return Ok(TrialExpiryPublication::Published {
            rollback: trial_expiry_rollback(Vec::new(), expected, proposed),
        });
    }

    let path = fork_registry_path(home);
    if !path.try_exists().map_err(|error| error.to_string())? {
        return Err(
            "Fork registry is absent; refusing expiry publication with expected records".into(),
        );
    }
    let parent = path.parent().ok_or("Registry has no parent")?;
    let scope = SkillReadScope::bind(&[parent.to_path_buf()]).map_err(|error| error.to_string())?;
    let guard = CoordinationPlan::new(
        vec![DirectoryEffect::entry(
            path.clone(),
            CoordinationMode::Exclusive,
        )],
        Some(std::time::Duration::from_secs(5)),
    )
    .map_err(|error| error.to_string())?
    .acquire()
    .map_err(|error| error.to_string())?;
    let mut lease = guard
        .finalize_write(&scope, std::slice::from_ref(&path))
        .map_err(|error| error.to_string())?;
    publish_trial_expiry_with_lease(home, expected, proposed, &mut lease)
}

/// Publishes a selected trial expiry transition while the caller retains the
/// registry lease. This permits a larger operation to hold the event store and
/// registry in one fixed order without reacquiring either lock.
#[cfg(unix)]
pub fn publish_trial_expiry_with_lease(
    home: &Path,
    expected: &ForkRegistry,
    proposed: &ForkRegistry,
    lease: &mut crate::skill_coordination::FinalizedWriteLease<'_>,
) -> Result<TrialExpiryPublication, String> {
    use crate::skill_document_target::SkillRegistryTarget;

    let (changed_trials, changed_copies) = changed_trial_expiry_records(expected, proposed)?;
    if changed_trials.is_empty() && changed_copies.is_empty() {
        return Ok(TrialExpiryPublication::Published {
            rollback: trial_expiry_rollback(Vec::new(), expected, proposed),
        });
    }
    let path = fork_registry_path(home);
    let parent = path.parent().ok_or("Registry has no parent")?;
    let original = lease.read_retained(&path, 8 * 1024 * 1024)?;
    let current = parse_fork_registry(
        std::str::from_utf8(&original).map_err(|_| "Fork registry is not UTF-8")?,
        &path,
    )?;
    ensure_selected_records_match(&current.trials, &expected.trials, &changed_trials, "trial")?;
    ensure_selected_records_match(&current.copies, &expected.copies, &changed_copies, "copy")?;
    let mut document: serde_json::Value =
        serde_json::from_slice(&original).map_err(|error| error.to_string())?;
    apply_selected_records(&mut document, "trials", &proposed.trials, &changed_trials)?;
    apply_selected_records(&mut document, "copies", &proposed.copies, &changed_copies)?;
    let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
    let rollback = trial_expiry_rollback(original.clone(), expected, proposed);
    match SkillRegistryTarget::bind(parent)?.replace_retained(lease, &original, &bytes) {
        Ok(()) => {}
        Err(DocumentWriteFailure::AfterReplace(error)) => {
            return Ok(TrialExpiryPublication::PublishedWithDurabilityError { error, rollback });
        }
        Err(DocumentWriteFailure::BeforeReplace(error)) => return Err(error),
    }
    if let Err(error) = lease.revalidate() {
        return Ok(TrialExpiryPublication::PublishedWithDurabilityError {
            error: error.to_string(),
            rollback,
        });
    }
    Ok(TrialExpiryPublication::Published { rollback })
}

/// Reverses one published expiry transition under the same selected-record
/// guard. The preimage restores unknown fields on selected records while the
/// latest document supplies every unrelated field.
#[cfg(unix)]
pub fn rollback_trial_expiry(
    home: &Path,
    expected: &ForkRegistry,
    proposed: &ForkRegistry,
    rollback: TrialExpiryRollback,
) -> Result<TrialExpiryPublication, String> {
    use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
    use crate::skill_scope::SkillReadScope;
    let path = fork_registry_path(home);
    let parent = path.parent().ok_or("Registry has no parent")?;
    let scope = SkillReadScope::bind(&[parent.to_path_buf()]).map_err(|error| error.to_string())?;
    let guard = CoordinationPlan::new(
        vec![DirectoryEffect::entry(
            path.clone(),
            CoordinationMode::Exclusive,
        )],
        Some(std::time::Duration::from_secs(5)),
    )
    .map_err(|error| error.to_string())?
    .acquire()
    .map_err(|error| error.to_string())?;
    let mut lease = guard
        .finalize_write(&scope, std::slice::from_ref(&path))
        .map_err(|error| error.to_string())?;
    rollback_trial_expiry_with_lease(home, expected, proposed, rollback, &mut lease)
}

/// Reverses a selected trial expiry transition while retaining the caller's
/// registry lease. It uses the original JSON preimage for selected records and
/// keeps current unknown and unrelated fields intact.
#[cfg(unix)]
pub fn rollback_trial_expiry_with_lease(
    home: &Path,
    expected: &ForkRegistry,
    proposed: &ForkRegistry,
    rollback: TrialExpiryRollback,
    lease: &mut crate::skill_coordination::FinalizedWriteLease<'_>,
) -> Result<TrialExpiryPublication, String> {
    use crate::skill_document_target::SkillRegistryTarget;

    if expected.trials != rollback.after_trials
        || expected.copies != rollback.after_copies
        || proposed.trials != rollback.before_trials
        || proposed.copies != rollback.before_copies
    {
        return Err("Trial expiry rollback does not match its publication receipt".into());
    }
    let mut unchanged = proposed.clone();
    unchanged.trials = expected.trials.clone();
    unchanged.copies = expected.copies.clone();
    if serde_json::to_value(&unchanged).map_err(|error| error.to_string())?
        != serde_json::to_value(expected).map_err(|error| error.to_string())?
    {
        return Err("Trial expiry rollback may only change trial or copy records".into());
    }
    let changed_trials = changed_registry_keys(&expected.trials, &proposed.trials);
    let changed_copies = changed_registry_keys(&expected.copies, &proposed.copies);
    let path = fork_registry_path(home);
    let parent = path.parent().ok_or("Registry has no parent")?;
    let current_bytes = lease.read_retained(&path, 8 * 1024 * 1024)?;
    let current = parse_fork_registry(
        std::str::from_utf8(&current_bytes).map_err(|_| "Fork registry is not UTF-8")?,
        &path,
    )?;
    ensure_selected_records_match(&current.trials, &expected.trials, &changed_trials, "trial")?;
    ensure_selected_records_match(&current.copies, &expected.copies, &changed_copies, "copy")?;
    let preimage: serde_json::Value =
        serde_json::from_slice(&rollback.original).map_err(|error| error.to_string())?;
    let mut document: serde_json::Value =
        serde_json::from_slice(&current_bytes).map_err(|error| error.to_string())?;
    apply_selected_preimages(
        &mut document,
        &preimage,
        "trials",
        &proposed.trials,
        &changed_trials,
    )?;
    apply_selected_preimages(
        &mut document,
        &preimage,
        "copies",
        &proposed.copies,
        &changed_copies,
    )?;
    let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
    let next_rollback = trial_expiry_rollback(current_bytes.clone(), expected, proposed);
    match SkillRegistryTarget::bind(parent)?.replace_retained(lease, &current_bytes, &bytes) {
        Ok(()) => {}
        Err(DocumentWriteFailure::AfterReplace(error)) => {
            return Ok(TrialExpiryPublication::PublishedWithDurabilityError {
                error,
                rollback: next_rollback,
            });
        }
        Err(DocumentWriteFailure::BeforeReplace(error)) => return Err(error),
    }
    if let Err(error) = lease.revalidate() {
        return Ok(TrialExpiryPublication::PublishedWithDurabilityError {
            error: error.to_string(),
            rollback: next_rollback,
        });
    }
    Ok(TrialExpiryPublication::Published {
        rollback: next_rollback,
    })
}

#[cfg(unix)]
fn changed_trial_expiry_records(
    expected: &ForkRegistry,
    proposed: &ForkRegistry,
) -> Result<(Vec<String>, Vec<String>), String> {
    let mut unchanged = proposed.clone();
    unchanged.trials = expected.trials.clone();
    unchanged.copies = expected.copies.clone();
    if serde_json::to_value(&unchanged).map_err(|error| error.to_string())?
        != serde_json::to_value(expected).map_err(|error| error.to_string())?
    {
        return Err("Trial expiry may only change trial or copy records".to_string());
    }
    Ok((
        changed_registry_keys(&expected.trials, &proposed.trials),
        changed_registry_keys(&expected.copies, &proposed.copies),
    ))
}

#[cfg(unix)]
fn changed_registry_keys<T: PartialEq>(
    expected: &BTreeMap<String, T>,
    proposed: &BTreeMap<String, T>,
) -> Vec<String> {
    expected
        .keys()
        .chain(proposed.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| expected.get(*key) != proposed.get(*key))
        .cloned()
        .collect()
}

#[cfg(unix)]
fn ensure_selected_records_match<T: PartialEq>(
    current: &BTreeMap<String, T>,
    expected: &BTreeMap<String, T>,
    keys: &[String],
    kind: &str,
) -> Result<(), String> {
    for key in keys {
        if current.get(key) != expected.get(key) {
            return Err(format!(
                "Selected {kind} record changed during expiry: {key}"
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn apply_selected_records<T: Serialize>(
    document: &mut serde_json::Value,
    bucket: &str,
    proposed: &BTreeMap<String, T>,
    keys: &[String],
) -> Result<(), String> {
    let root = document
        .as_object_mut()
        .ok_or("Fork registry must be a JSON object")?;
    let records = root
        .entry(bucket.to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| format!("Fork registry {bucket} must be a JSON object"))?;
    for key in keys {
        match proposed.get(key) {
            None => {
                records.remove(key);
            }
            Some(record) => {
                let update = serde_json::to_value(record).map_err(|error| error.to_string())?;
                match (
                    records
                        .get_mut(key)
                        .and_then(serde_json::Value::as_object_mut),
                    update.as_object(),
                ) {
                    (Some(existing), Some(update)) => {
                        for (field, value) in update {
                            existing.insert(field.clone(), value.clone());
                        }
                    }
                    _ => {
                        records.insert(key.clone(), update);
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn apply_selected_preimages<T: Serialize>(
    document: &mut serde_json::Value,
    preimage: &serde_json::Value,
    bucket: &str,
    proposed: &BTreeMap<String, T>,
    keys: &[String],
) -> Result<(), String> {
    let root = document
        .as_object_mut()
        .ok_or("Fork registry must be a JSON object")?;
    let records = root
        .entry(bucket.to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| format!("Fork registry {bucket} must be a JSON object"))?;
    let source = preimage.get(bucket).and_then(serde_json::Value::as_object);
    for key in keys {
        match proposed.get(key) {
            None => {
                records.remove(key);
            }
            Some(record) => {
                let value = source
                    .and_then(|records| records.get(key))
                    .cloned()
                    .unwrap_or(serde_json::to_value(record).map_err(|error| error.to_string())?);
                records.insert(key.clone(), value);
            }
        }
    }
    Ok(())
}

/// A byte-exact compatibility snapshot. This uses ambient paths and does not
/// replace the scoped cross-process lease required by the shared write service.
pub struct ForkRegistrySnapshot {
    home: PathBuf,
    content: Option<Vec<u8>>,
    registry: ForkRegistry,
}

impl ForkRegistrySnapshot {
    pub fn read(home: &Path) -> Result<Self, String> {
        let path = fork_registry_path(home);
        let content = read_registry_bytes(&path)?;
        let registry = match &content {
            Some(bytes) => parse_fork_registry(
                std::str::from_utf8(bytes).map_err(|_| "Fork registry is not UTF-8")?,
                &path,
            )?,
            None => ForkRegistry::default(),
        };
        Ok(Self {
            home: home.to_path_buf(),
            content,
            registry,
        })
    }

    pub fn registry(&self) -> &ForkRegistry {
        &self.registry
    }

    /// Refuses observed drift immediately before rename. This is not atomic
    /// compare-and-swap against uncoordinated external writers.
    pub fn replace(self, registry: &ForkRegistry) -> Result<(), DocumentWriteFailure> {
        write_registry(&self.home, registry, Some(&self.content), || {})
    }
}

fn read_registry_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Failed to read {}: {error}", path.display())),
    }
}

/// Counter appended to the write's temp file name, so concurrent writers
/// never pick the same temp path.
static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `registry` atomically (temp file + rename), creating `~/.agents` if
/// it doesn't already exist.
pub fn write_fork_registry(home: &Path, registry: &ForkRegistry) -> Result<(), String> {
    write_registry(home, registry, None, || {}).map_err(|error| error.to_string())
}

fn write_registry(
    home: &Path,
    registry: &ForkRegistry,
    expected: Option<&Option<Vec<u8>>>,
    before_commit: impl FnOnce(),
) -> Result<(), DocumentWriteFailure> {
    let path = fork_registry_path(home);
    let parent = path
        .parent()
        .ok_or_else(|| DocumentWriteFailure::BeforeReplace("Registry has no parent".into()))?;
    #[cfg(unix)]
    let _coordination = {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        CoordinationPlan::new(
            vec![DirectoryEffect::entry(&path, CoordinationMode::Exclusive)],
            None,
        )
        .and_then(|plan| plan.acquire())
        .map_err(|error| DocumentWriteFailure::BeforeReplace(error.to_string()))?
    };
    std::fs::create_dir_all(parent)
        .map_err(|error| DocumentWriteFailure::BeforeReplace(error.to_string()))?;
    let json = serde_json::to_vec_pretty(registry)
        .map_err(|error| DocumentWriteFailure::BeforeReplace(error.to_string()))?;
    let unique = WRITE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let tmp_path = path.with_extension(format!("json.tmp.{}.{unique}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .map_err(|error| DocumentWriteFailure::BeforeReplace(error.to_string()))?;
    let staged = (|| -> Result<(), String> {
        file.write_all(&json).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        before_commit();
        if let Some(expected) = expected {
            if &read_registry_bytes(&path)? != expected {
                return Err(
                    "Fork registry changed since it was read; reload before writing".into(),
                );
            }
        }
        std::fs::rename(&tmp_path, &path).map_err(|error| error.to_string())
    })();
    if let Err(error) = staged {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(DocumentWriteFailure::BeforeReplace(error));
    }
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| DocumentWriteFailure::AfterReplace(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn registry_publication_holds_shared_coordination_until_completion() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        for existing in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            if existing {
                write_fork_registry(temp.path(), &ForkRegistry::default()).unwrap();
            }
            let path = fork_registry_path(temp.path());
            let acquire = || {
                CoordinationPlan::new_fixture(
                    vec![DirectoryEffect::entry(&path, CoordinationMode::Exclusive)],
                    temp.path(),
                    Some(std::time::Duration::from_millis(30)),
                )
                .unwrap()
                .acquire()
            };
            write_registry(temp.path(), &ForkRegistry::default(), None, || {
                assert!(
                    acquire().is_err(),
                    "competing writer entered registry publication"
                );
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "skill_fork_registry::tests::registry_coordination_child",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("SKILL_STUDIO_REGISTRY_LOCK_TEST_HOME", temp.path())
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            })
            .unwrap();
            drop(acquire().expect("completed writer must release its locks"));
            assert_eq!(read_fork_registry(temp.path()).unwrap().version, 4);
        }
    }

    #[cfg(unix)]
    #[test]
    fn retained_trial_expiry_lease_rolls_back_and_preserves_unknown_json() {
        use crate::skill_coordination::{CoordinationMode, CoordinationPlan, DirectoryEffect};
        use crate::skill_scope::SkillReadScope;

        let temp = tempfile::tempdir().unwrap();
        let mut expected = ForkRegistry::default();
        expected.trials.insert(
            "deployment/test".into(),
            TrialRecord {
                deployment_id: "test".into(),
                started_at: "2026-01-01T00:00:00Z".into(),
                expires_at: "2026-01-02T00:00:00Z".into(),
                status: TrialStatus::Active,
                method: AddMethod::Dotagents,
                scope: TrialScope::Global,
                project_path: None,
                skill_dir: temp.path().join("skill"),
                deployment_fingerprint: "fingerprint".into(),
                claude_link: None,
                claude_link_target: None,
            },
        );
        write_fork_registry(temp.path(), &expected).unwrap();
        let path = fork_registry_path(temp.path());
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        raw["unrecognized_top_level"] = serde_json::json!({"kept": true});
        raw["trials"]["deployment/test"]["future_trial_field"] = serde_json::json!({"kept": true});
        std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
        let expected = read_fork_registry(temp.path()).unwrap();
        let mut proposed = expected.clone();
        proposed.trials.clear();
        let parent = path.parent().unwrap();
        let scope = SkillReadScope::bind(&[parent.to_path_buf()]).unwrap();
        let guard = CoordinationPlan::new(
            vec![DirectoryEffect::entry(&path, CoordinationMode::Exclusive)],
            Some(std::time::Duration::from_secs(5)),
        )
        .unwrap()
        .acquire()
        .unwrap();
        let mut lease = guard
            .finalize_write(&scope, std::slice::from_ref(&path))
            .unwrap();

        let rollback =
            match publish_trial_expiry_with_lease(temp.path(), &expected, &proposed, &mut lease)
                .unwrap()
            {
                TrialExpiryPublication::Published { rollback } => rollback,
                TrialExpiryPublication::PublishedWithDurabilityError { .. } => {
                    panic!("fixture publication must be durable")
                }
            };
        let restored = rollback_trial_expiry_with_lease(
            temp.path(),
            &proposed,
            &expected,
            rollback,
            &mut lease,
        )
        .unwrap();
        assert!(matches!(restored, TrialExpiryPublication::Published { .. }));
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["unrecognized_top_level"]["kept"], true);
        assert_eq!(
            raw["trials"]["deployment/test"]["future_trial_field"]["kept"],
            true
        );
        assert_eq!(
            read_fork_registry(temp.path()).unwrap().trials,
            expected.trials
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "run by the registry coordination parent fixture"]
    fn registry_coordination_child() {
        use crate::skill_coordination::{
            CoordinationFailure, CoordinationMode, CoordinationPlan, DirectoryEffect,
        };
        let home = PathBuf::from(std::env::var_os("SKILL_STUDIO_REGISTRY_LOCK_TEST_HOME").unwrap());
        let result = CoordinationPlan::new(
            vec![DirectoryEffect::entry(
                fork_registry_path(&home),
                CoordinationMode::Exclusive,
            )],
            Some(std::time::Duration::from_millis(50)),
        )
        .unwrap()
        .acquire();
        assert!(matches!(result, Err(CoordinationFailure::Busy)));
    }

    #[test]
    fn snapshot_replacement_refuses_changed_removed_and_new_registry() {
        for change in ["updated", "removed", "created"] {
            let temp = tempfile::tempdir().unwrap();
            if change != "created" {
                write_fork_registry(temp.path(), &ForkRegistry::default()).unwrap();
            }
            let snapshot = ForkRegistrySnapshot::read(temp.path()).unwrap();
            let replacement = snapshot.registry().clone();
            let path = fork_registry_path(temp.path());
            if change == "removed" {
                std::fs::remove_file(&path).unwrap();
            } else {
                let newer = ForkRegistry {
                    preferred_editor: Some("external".into()),
                    ..ForkRegistry::default()
                };
                write_fork_registry(temp.path(), &newer).unwrap();
            }
            let current = read_registry_bytes(&path).unwrap();
            assert!(matches!(
                snapshot.replace(&replacement),
                Err(DocumentWriteFailure::BeforeReplace(_))
            ));
            assert_eq!(read_registry_bytes(&path).unwrap(), current);
            assert!(std::fs::read_dir(temp.path().join(".agents"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp.")));
        }
    }

    #[test]
    fn snapshot_replacement_checks_after_staging_and_publishes_valid_updates() {
        let temp = tempfile::tempdir().unwrap();
        write_fork_registry(temp.path(), &ForkRegistry::default()).unwrap();
        let snapshot = ForkRegistrySnapshot::read(temp.path()).unwrap();
        let updated = ForkRegistry {
            preferred_editor: Some("updated".into()),
            ..snapshot.registry().clone()
        };
        let path = fork_registry_path(temp.path());
        let external = b"{\"version\":4,\"preferred_editor\":\"external\"}";
        let result = write_registry(&snapshot.home, &updated, Some(&snapshot.content), || {
            std::fs::write(&path, external).unwrap();
        });
        assert!(matches!(
            result,
            Err(DocumentWriteFailure::BeforeReplace(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), external);
        ForkRegistrySnapshot::read(temp.path())
            .unwrap()
            .replace(&updated)
            .unwrap();
        assert_eq!(
            read_fork_registry(temp.path())
                .unwrap()
                .preferred_editor
                .as_deref(),
            Some("updated")
        );
        std::fs::write(&path, "malformed").unwrap();
        assert!(ForkRegistrySnapshot::read(temp.path()).is_err());
    }

    #[test]
    fn missing_file_yields_default_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = read_fork_registry(tmp.path()).unwrap();
        assert_eq!(reg.version, 4);
        assert!(reg.forks.is_empty());
    }

    #[test]
    fn round_trips_through_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = ForkRegistry::default();
        reg.forks.insert(
            "find-bugs".to_string(),
            ForkRecord {
                deployment_id: "dep:v1/global/universal/universal/find-bugs/-/x".to_string(),
                skill_dir: tmp.path().join(".agents/skills/find-bugs"),
                forked_at: "2026-01-01T00:00:00Z".to_string(),
                origin_tool: OriginTool::Dotagents,
                origin_source: "getsentry/find-bugs".to_string(),
                repo: "getsentry/find-bugs".to_string(),
                path: "skills/find-bugs".to_string(),
                declared_ref: None,
                base_commit: "a".repeat(40),
            },
        );
        write_fork_registry(tmp.path(), &reg).unwrap();

        let reloaded = read_fork_registry(tmp.path()).unwrap();
        assert_eq!(reloaded.forks.len(), 1);
        assert_eq!(
            reloaded.forks["find-bugs"].origin_tool,
            OriginTool::Dotagents
        );
        assert!(reloaded.trials.is_empty());
    }

    #[test]
    fn corrupt_file_is_an_error_for_the_mutation_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".agents")).unwrap();
        std::fs::write(tmp.path().join(".agents/skill-studio.json"), "not json").unwrap();
        let err = read_fork_registry(tmp.path()).unwrap_err();
        assert!(err.contains("malformed"));
    }

    #[test]
    fn version_two_registry_reads_with_no_inferred_copy_ownership() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".agents")).unwrap();
        std::fs::write(
            tmp.path().join(".agents/skill-studio.json"),
            r#"{"version":2,"forks":{},"trials":{},"parked":{},"harness_disabled":{},"packs":{}}"#,
        )
        .unwrap();

        let registry = read_fork_registry(tmp.path()).unwrap();
        assert_eq!(registry.version, 2);
        assert!(registry.copies.is_empty());
    }

    #[test]
    fn corrupt_file_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".agents")).unwrap();
        std::fs::write(tmp.path().join(".agents/skill-studio.json"), "not json").unwrap();
        assert!(read_fork_registry(tmp.path()).is_err());
    }

    #[test]
    fn trial_key_distinguishes_global_and_project_scope() {
        let global_key = trial_key(TrialScope::Global, "find-bugs");
        let project_key = trial_key(TrialScope::Project, "find-bugs");
        assert_eq!(global_key, "global/find-bugs");
        assert_eq!(project_key, "project/find-bugs");
        assert_ne!(global_key, project_key);
        assert_eq!(name_from_trial_key(&global_key), "find-bugs");
        assert_eq!(name_from_trial_key(&project_key), "find-bugs");
    }

    #[test]
    fn no_leftover_temp_files_after_write() {
        let tmp = tempfile::tempdir().unwrap();
        write_fork_registry(tmp.path(), &ForkRegistry::default()).unwrap();
        let leftover = std::fs::read_dir(tmp.path().join(".agents"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(leftover, 0);
    }
}
