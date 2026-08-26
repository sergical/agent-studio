// ============================================================================
// Skills Module - skill_fork_registry
// Reads and writes `~/.agents/skill-studio.json`, the one Skill-Studio-owned
// file inside `~/.agents` - the app never touches `agents.toml`,
// `agents.lock`, or `.skill-lock.json` itself, those belong to the owning
// CLI. Tracks which skills have been detached from their ledger ("forked")
// so local edits survive `dotagents sync` / `npx skills update`, plus a
// `trials` bucket left empty for a later step. A missing file yields a
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
use serde_json::Value;

/// Which CLI a forked skill was originally managed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OriginTool {
    Dotagents,
    SkillsSh,
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

/// `~/.agents/skill-studio.json`'s shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkRegistry {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub forks: BTreeMap<String, ForkRecord>,
    /// Reserved for a later step's trial-install bookkeeping; always
    /// round-tripped as-is so this step doesn't need to know its shape.
    #[serde(default = "default_trials")]
    pub trials: Value,
}

fn default_version() -> u32 {
    1
}

fn default_trials() -> Value {
    Value::Object(serde_json::Map::new())
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
            trials: default_trials(),
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
        assert_eq!(reloaded.trials, Value::Object(serde_json::Map::new()));
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
