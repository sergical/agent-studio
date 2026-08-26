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
// every mutating command (fork/pull/unfork/remove), since silently treating
// it as empty would erase every recorded fork on the next write. Read-only
// callers (snapshot/candidate building) use `read_fork_registry_or_default`
// instead, which downgrades that same error to a logged warning.
// ============================================================================

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::provenance::SourceKind;

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

/// One "Try for 24 hours" install, tracked so `skill_trial`'s expiry loop
/// knows when to remove it and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrialRecord {
    pub started_at: String,
    pub expires_at: String,
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
    /// The per-skill Claude Code symlink `add_skill` created for this trial,
    /// if any - `None` when Claude Code wasn't selected or the whole-dir
    /// symlink already covered it.
    #[serde(default)]
    pub claude_link: Option<PathBuf>,
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
    /// The symlink's original target, so re-enabling can recreate it exactly
    /// (relative, as `maybe_claude_code_symlink` creates it).
    pub link_target: PathBuf,
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
}

fn default_version() -> u32 {
    1
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
    let path = fork_registry_path(home);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ForkRegistry::default()),
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    serde_json::from_str(&content).map_err(|_| {
        "~/.agents/skill-studio.json is malformed; fix or move it before forking".to_string()
    })
}

/// `read_fork_registry`, but for read-only snapshot/candidate building: an
/// unreadable or malformed registry is logged and treated as empty instead
/// of failing an entire background rebuild.
pub fn read_fork_registry_or_default(home: &Path) -> ForkRegistry {
    read_fork_registry(home).unwrap_or_else(|e| {
        eprintln!("skill fork registry: {e}");
        ForkRegistry::default()
    })
}

/// Counter appended to the write's temp file name, so concurrent writers
/// never pick the same temp path.
static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `registry` atomically (temp file + rename), creating `~/.agents` if
/// it doesn't already exist.
pub fn write_fork_registry(home: &Path, registry: &ForkRegistry) -> Result<(), String> {
    let path = fork_registry_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(registry)
        .map_err(|e| format!("Failed to serialize fork registry: {e}"))?;
    let unique = WRITE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let tmp_path = path.with_extension(format!("json.tmp.{}.{unique}", std::process::id()));
    std::fs::write(&tmp_path, json)
        .map_err(|e| format!("Failed to write {}: {e}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to rename {}: {e}", tmp_path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_default_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = read_fork_registry(tmp.path()).unwrap();
        assert_eq!(reg.version, 1);
        assert!(reg.forks.is_empty());
    }

    #[test]
    fn round_trips_through_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = ForkRegistry::default();
        reg.forks.insert(
            "find-bugs".to_string(),
            ForkRecord {
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
    fn corrupt_file_is_logged_and_treated_as_empty_for_the_read_only_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".agents")).unwrap();
        std::fs::write(tmp.path().join(".agents/skill-studio.json"), "not json").unwrap();
        let reg = read_fork_registry_or_default(tmp.path());
        assert!(reg.forks.is_empty());
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
