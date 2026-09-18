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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use skill_studio_core::discovery_sources::DiscoverySources;
use skill_studio_core::tracked_projects::TrackedProjects;

use super::skill_deployment::SkillDestination;
use super::skill_dto::InstallScope;
use super::SourceKind;

fn path_is_empty(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

/// Which CLI a forked skill was originally managed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OriginTool {
    Dotagents,
    SkillsSh,
}

/// How `add_skill` installed a skill - shared by `AddSkillRequest.method` and
/// `TrialRecord.method`, since a trial's expiry step needs to know which tool
/// (if any) owns the skill it's about to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AddMethod {
    Dotagents,
    SkillsSh,
    Copy,
}

/// Which scope a trial (or an `add_skill` request) targeted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TrialScope {
    Global,
    Project,
}

/// Durable state for trial expiry. `Expiring` prevents an interrupted CLI
/// removal from matching a later installation at the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
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
    key.split_once('/').map_or(key, |(_, name)| name)
}

/// One forked skill's provenance, enough to reinstall it from its origin
/// (`unfork_skill`) or to fetch its upstream at a specific commit
/// (`pull_fork_upstream`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
/// its per-skill symlink), since Codex and `OpenCode` read their own disable
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

/// `~/.agents/skill-studio.json`'s shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkRegistry {
    #[serde(default = "default_version")]
    pub version: u32,
    /// Write counter bumped by [`write_fork_registry`] on every write, via
    /// the core's [`skill_studio_core::registry::write_registry_document`].
    /// Distinct from `version`, which marks a schema migration and is set
    /// by hand - see `crates/skill-studio-core/src/registry.rs`. Defaults to
    /// 0 for a file written before this field existed.
    #[serde(default)]
    pub write_version: u64,
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
    /// Folders the user added by hand or stopped tracking - the core's
    /// [`TrackedProjects`], saved here so the CLI, the MCP server, and every
    /// version of the desktop app discover the same projects.
    #[serde(default, skip_serializing_if = "TrackedProjects::is_empty")]
    pub projects: TrackedProjects,
    /// Per-harness project discovery switches - see
    /// `skill_studio_core::discovery_sources::DiscoverySources`. Saved here so
    /// the desktop app, the CLI, and the MCP server honour the same choice.
    #[serde(default, skip_serializing_if = "DiscoverySources::is_empty")]
    pub discovery: DiscoverySources,
    /// Opt-in error reporting (Settings' "Error reporting" toggle) - see
    /// `error_reporting`. Off by default: a panic or a command failure is
    /// sanitized and sent to Sentry only once this is `true`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub error_reporting_enabled: bool,
    /// The first-run screen's saved choice - see `harness_first_run`.
    /// Absent means the screen has never been completed, so the app shows
    /// it again on the next launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harnesses: Option<super::harness_first_run::HarnessesChoice>,
    /// Every top-level key this build doesn't know about. Keeps a write from
    /// erasing a field a newer or older build added - the file is shared
    /// with the CLI and with whichever app version last wrote it.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

impl skill_studio_core::registry::RegistryDocument for ForkRegistry {
    fn write_version(&self) -> u64 {
        self.write_version
    }

    fn set_write_version(&mut self, version: u64) {
        self.write_version = version;
    }
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
            write_version: 0,
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
            projects: TrackedProjects::default(),
            discovery: DiscoverySources::default(),
            error_reporting_enabled: false,
            harnesses: None,
            unknown: serde_json::Map::new(),
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
        "~/.agents/skill-studio.json is malformed; fix or move it, then try again".to_string()
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

/// Where `FileLease` keeps its advisory lock files for this registry -
/// `core_runtime::data_root()`'s `leases` subdirectory, the same lease root
/// the CLI, MCP, and desktop's park/unpark commands already share.
fn registry_lease_root() -> PathBuf {
    super::core_runtime::data_root().join("leases")
}

/// Write `registry` atomically under the exclusive lease over `home`,
/// bumping `write_version` by one - see
/// `skill_studio_core::registry::write_registry_document`. Creates
/// `~/.agents` if it doesn't already exist.
pub fn write_fork_registry(home: &Path, registry: &ForkRegistry) -> Result<(), String> {
    // The core's scope normalization canonicalizes `home`, which requires
    // it to exist already - callers historically relied on this function
    // creating a never-before-seen home (e.g. a fresh project scope) via
    // `create_dir_all` on the registry's parent, so do that first here too.
    std::fs::create_dir_all(home)
        .map_err(|e| format!("Failed to create {}: {e}", home.display()))?;
    let path = fork_registry_path(home);
    let fs = skill_studio_host::RealFs::new();
    let leases = skill_studio_host::FileLease::new(registry_lease_root());
    let mut document = registry.clone();
    skill_studio_core::registry::write_registry_document(&leases, &fs, home, &path, &mut document)
        .map_err(|e| e.to_string())
}

/// `write_fork_registry`, for a caller that already holds `home`'s
/// `WriteLease` - a command that took its lease before touching several
/// lease-guarded things, for instance. Writes under that held lease instead
/// of taking a second, conflicting one: advisory locks don't nest within
/// one process, so a nested `write_fork_registry` would report the caller's
/// own lease as busy instead of writing.
pub fn write_fork_registry_locked(
    guard: &super::write_lease::WriteLeaseGuard,
    home: &Path,
    registry: &ForkRegistry,
) -> Result<(), String> {
    std::fs::create_dir_all(home)
        .map_err(|e| format!("Failed to create {}: {e}", home.display()))?;
    let path = fork_registry_path(home);
    let fs = skill_studio_host::RealFs::new();
    let mut document = registry.clone();
    skill_studio_core::registry::write_registry_document_locked(
        guard.as_exclusive_guard(),
        &fs,
        home,
        &path,
        &mut document,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// (F1) Flow: a Copy install runs through the core's `ops::install`
    /// directly against `home`, the way `apps/cli`'s `add` subcommand and
    /// the MCP server's install tool both will - neither goes through the
    /// desktop's own `add_skill`. Expectation: the `copies` entry it writes
    /// deserializes into this file's own `CopyDeploymentRecord` with a
    /// `deployment_id` in the desktop's `dep:v1/...` shape, so the desktop's
    /// removal/discovery code (keyed by that field) recognizes a
    /// core-installed skill without a schema migration.
    /// Failure: a missing/malformed `deployment_id`, or a `copies` entry
    /// that doesn't deserialize into `CopyDeploymentRecord` at all - either
    /// means the core and the desktop have silently drifted onto two
    /// different `copies` shapes.
    #[test]
    fn a_core_copy_install_writes_a_registry_the_desktop_reads_back_or_names_the_missing_field() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let data_root = home.join(".skill-studio");

        let ports = skill_studio_core::ports::Ports {
            fs: std::sync::Arc::new(skill_studio_host::RealFs::new()),
            clock: std::sync::Arc::new(skill_studio_core::testing::FakeClock::at(0)),
            ids: std::sync::Arc::new(skill_studio_core::testing::FakeIds::default()),
            leases: std::sync::Arc::new(skill_studio_host::FileLease::new(
                data_root.join("leases"),
            )),
            // `install` records a journal event row before its first write,
            // which `NoHistory` refuses - use a real sqlite-backed store, the
            // same as the core's own `ops_install.rs` tests.
            history: std::sync::Arc::new(skill_studio_host::SqliteHistoryOpener::new(
                home.join(".history").join("events.sqlite3"),
            )),
            sink: std::sync::Arc::new(skill_studio_core::testing::RecordingSink::default()),
            spawner: None,
            discovery: None,
            tools: None,
            catalog: std::sync::Arc::new(skill_studio_core::harness::HarnessCatalog::builtin()),
        };
        let rt = skill_studio_core::ports::Runtime::new(
            &skill_studio_core::scope::RuntimeScope::fixture(home),
            ports,
        )
        .unwrap();

        let req = skill_studio_core::dto::InstallRequest {
            skill: skill_studio_core::identity::SkillName("find-bugs".to_string()),
            method: skill_studio_core::dto::InstallMethod::Copy,
            scope: skill_studio_core::identity::RootScope::Global,
            harnesses: Vec::new(),
            files: vec![skill_studio_core::dto::InstallFile {
                relative_path: PathBuf::from("SKILL.md"),
                contents: b"---\nname: find-bugs\ndescription: finds bugs\n---\nBody.\n".to_vec(),
            }],
            source: None,
            trust_identity: None,
            trust_confirmed: false,
            save_as_preference: false,
        };
        skill_studio_core::ops::install(&rt, &skill_studio_core::testing::golden::ctx(), &req)
            .unwrap();

        let reg = read_fork_registry(home).unwrap();
        assert_eq!(reg.copies.len(), 1, "expected exactly one copies entry");
        let record = reg.copies.get("find-bugs").expect(
            "the copies map must be keyed by the deployment id the core just wrote a record under, \
             or by the skill name - neither key was found",
        );
        assert!(
            record
                .deployment_id
                .starts_with("dep:v1/global/universal/universal/find-bugs/"),
            "unexpected deployment_id shape: {}",
            record.deployment_id
        );
        assert_eq!(record.scope, InstallScope::Global);
        assert_eq!(record.destination, SkillDestination::Universal);
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
    fn an_unknown_top_level_key_survives_read_then_write() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".agents")).unwrap();
        std::fs::write(
            tmp.path().join(".agents/skill-studio.json"),
            r#"{"version":4,"a_future_field":{"nested":true}}"#,
        )
        .unwrap();

        let reg = read_fork_registry(tmp.path()).unwrap();
        assert_eq!(
            reg.unknown.get("a_future_field"),
            Some(&serde_json::json!({"nested": true}))
        );
        write_fork_registry(tmp.path(), &reg).unwrap();

        let reloaded = read_fork_registry(tmp.path()).unwrap();
        assert_eq!(
            reloaded.unknown.get("a_future_field"),
            Some(&serde_json::json!({"nested": true}))
        );
    }

    #[test]
    fn write_fork_registry_bumps_write_version_by_one_or_names_the_stuck_value() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = ForkRegistry::default();
        assert_eq!(
            reg.write_version, 0,
            "a fresh registry starts at write_version 0"
        );

        write_fork_registry(tmp.path(), &reg).unwrap();
        let after_first = read_fork_registry(tmp.path()).unwrap();
        assert_eq!(
            after_first.write_version, 1,
            "write_fork_registry did not bump write_version on its first write"
        );

        write_fork_registry(tmp.path(), &after_first).unwrap();
        let after_second = read_fork_registry(tmp.path()).unwrap();
        assert_eq!(
            after_second.write_version, 2,
            "write_fork_registry did not bump write_version on a second write"
        );
    }

    #[test]
    fn projects_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = ForkRegistry::default();
        reg.projects.added.push(tmp.path().join("proj"));
        write_fork_registry(tmp.path(), &reg).unwrap();

        let reloaded = read_fork_registry(tmp.path()).unwrap();
        assert_eq!(reloaded.projects.added, [tmp.path().join("proj")]);
    }

    #[test]
    fn an_empty_projects_list_is_not_written() {
        let tmp = tempfile::tempdir().unwrap();
        write_fork_registry(tmp.path(), &ForkRegistry::default()).unwrap();

        let content =
            std::fs::read_to_string(tmp.path().join(".agents/skill-studio.json")).unwrap();
        assert!(!content.contains("\"projects\""));
    }

    #[test]
    fn discovery_switches_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = ForkRegistry::default();
        reg.discovery.set("codex", false);
        write_fork_registry(tmp.path(), &reg).unwrap();

        let content =
            std::fs::read_to_string(tmp.path().join(".agents/skill-studio.json")).unwrap();
        assert!(content.contains(r#""discovery": {"#));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&content).unwrap()["discovery"],
            serde_json::json!({ "codex": false })
        );

        let reloaded = read_fork_registry(tmp.path()).unwrap();
        assert_eq!(reloaded.discovery, reg.discovery);
    }

    #[test]
    fn default_discovery_is_not_written() {
        let tmp = tempfile::tempdir().unwrap();
        write_fork_registry(tmp.path(), &ForkRegistry::default()).unwrap();

        let content =
            std::fs::read_to_string(tmp.path().join(".agents/skill-studio.json")).unwrap();
        assert!(!content.contains("\"discovery\""));
    }

    #[test]
    fn no_leftover_temp_files_after_write() {
        let tmp = tempfile::tempdir().unwrap();
        write_fork_registry(tmp.path(), &ForkRegistry::default()).unwrap();
        let leftover = std::fs::read_dir(tmp.path().join(".agents"))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(leftover, 0);
    }
}
