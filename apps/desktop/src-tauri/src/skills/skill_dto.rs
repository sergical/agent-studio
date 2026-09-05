// ============================================================================
// Skills Module - skill_dto
// Serialized shapes sent to the frontend over Tauri IPC: skills.sh API
// responses, installed-skill records, and installation request/response
// types. Lock file structs live in lock_file.rs, agent identifiers in
// agents.rs, SKILL.md frontmatter in frontmatter.rs, and source provenance
// in provenance.rs.
// ============================================================================

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::frontmatter::InvocationPolicy;
use super::github_skill_listing::GithubSkillEntry;
use super::provenance::SourceKind;
use super::skill_deployment::{BackingRelationship, DeploymentMutability, SkillDestination};
use super::skill_fork_registry::{AddMethod, OriginTool, TrialScope};
use super::skill_ownership::LifecycleOwnerKind;

/// Which mechanism `Deployment.disabled` came from - see
/// `skill_harness_disable`. The first three are native per-harness switches;
/// `StudioMoved` is the universal fallback that renames the deployment's
/// directory aside into a `.skill-studio-disabled/` holding directory in its
/// skills root, for harnesses with no native switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisabledBy {
    CodexConfig,
    OpencodePermission,
    ClaudeLinkRemoved,
    StudioMoved,
}

// ============================================================================
// Skills.sh API Types
// ============================================================================

/// Search result from skills.sh API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSearchResult {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub installs: u32,
    pub top_source: Option<String>,
    pub author: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Paginated response to return to frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedSkillsResponse {
    pub skills: Vec<SkillSearchResult>,
    pub has_more: bool,
}

/// skills.sh v1 skill details, including the skill's markdown body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDetails {
    pub id: String,
    pub source: String,
    pub slug: String,
    pub installs: u32,
    pub hash: String,
    /// SKILL.md (or AGENTS.md fallback) contents, when the payload has one.
    pub skill_md: Option<String>,
}

/// How discovery requests reach skills.sh - see `api::resolve_skills_sh_access`.
/// `"direct"` means a developer-override key is configured (`server_url` is
/// `None`); `"server"` means requests go through the local Skill Studio
/// server at `server_url`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsShAccessInfo {
    pub mode: String,
    pub server_url: Option<String>,
}

// ============================================================================
// Event Store Types
// ============================================================================

/// One row of the event log, projected for the Activity view's History
/// section - see `event_store::EventRow` and `event_commands::list_skill_events`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEventDto {
    pub id: String,
    pub ts: String,
    pub kind: String,
    pub skill: String,
    pub harness: Option<String>,
    pub scope: Option<String>,
    pub project_path: Option<String>,
    pub status: String,
    /// True when this event has an inverse, hasn't already been undone, and
    /// its status is one a restore makes sense for.
    pub restorable: bool,
    pub reverted_by: Option<String>,
    /// Absolute path to this event's backup directory, for a "Reveal in
    /// Finder" action - `None` when the event backed up nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
}

// ============================================================================
// Installed Skill Types
// ============================================================================
// InstalledSkillEntry and SkillLockFile (the raw lock-file shapes) live in
// lock_file.rs, next to the code that reads and writes them.

/// A plugin that shipped a skill, per the agent-plugins.org convention
/// (Claude Code / Codex plugin caches, or any directory with a `plugin.json`
/// manifest and a `skills/` subdirectory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: Option<String>,
    /// Which agent's plugin system this came from, e.g. "Claude Code", "Codex".
    pub harness: String,
}

/// Where a skill is deployed on disk for a specific agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    /// Stable id (`dep:v1/...`) for exact mutations. Empty only on
    /// lock-file-only records that have no on-disk path.
    #[serde(default)]
    pub id: String,
    /// Universal (`.agents/skills`) or Per harness. Compatibility still
    /// serializes the scanner label `shared` on `agent`.
    #[serde(default = "default_destination")]
    pub destination: SkillDestination,
    #[serde(default = "default_owner_kind")]
    pub owner_kind: LifecycleOwnerKind,
    /// `owner:v1/...` when a matching ledger owns this deployment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub mutability: DeploymentMutability,
    #[serde(default = "default_backing")]
    pub backing: BackingRelationship,
    /// Display name of the agent (e.g. "Claude Code"). Universal roots
    /// still use the compatibility label `shared`.
    pub agent: String,
    pub scope: String, // "global" | "project" | "plugin"
    pub path: String,
    pub is_symlink: bool,
    /// Set when this deployment is a skill shipped by a plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginInfo>,
    /// Canonicalized symlink target, when `is_symlink` and the target resolves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
    /// True when `is_symlink` but the target doesn't exist.
    #[serde(default)]
    pub symlink_is_broken: bool,
    /// Set when `is_symlink` and resolving the target failed for a reason
    /// other than "doesn't exist" (permission denied, symlink loop, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_error: Option<String>,
    /// The project directory this deployment belongs to, for project-scoped
    /// deployments. `None` for global and plugin deployments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    /// Canonical path of this deployment's directory when it differs from
    /// `path` - set when any ancestor is a symlink (e.g. a `.claude/skills`
    /// root linked to `.agents/skills`), so the frontend can tell "same
    /// folder through a linked root" from a separate copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_path: Option<String>,
    /// This deployment's own sha256 content hash, empty when unreadable
    /// (e.g. a broken symlink). Lets the UI point at which specific copies
    /// of a duplicated skill differ, not just the skill as a whole.
    #[serde(default)]
    pub content_hash: String,
    /// True when this specific deployment is disabled for its harness (as
    /// opposed to parked, which removes the skill from every harness at
    /// once) - see `skill_harness_disable`.
    #[serde(default)]
    pub disabled: bool,
    /// Which mechanism `disabled` came from, `None` when not disabled.
    #[serde(default)]
    pub disabled_by: Option<DisabledBy>,
    /// For a shared-root deployment (`agent == "shared"`) only: agent ids among
    /// the native shared-root readers whose own mechanism disables this skill
    /// (Codex config / OpenCode permission deny) - `"codex"`, `"open-code"`.
    /// Always empty for other deployments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_readers: Vec<String>,
    /// Codex's own `agents/openai.yaml` `policy.allow_implicit_invocation`
    /// value, read straight off disk - note-only, doesn't affect
    /// `InstalledSkill.invocation` (that's driven by SKILL.md frontmatter).
    #[serde(default)]
    pub codex_implicit_invocation: Option<bool>,
    /// True when this deployment's skills root is itself a symlink resolving
    /// into the shared `.agents/skills` folder (e.g. `~/.claude/skills ->
    /// ../.agents/skills`) - a whole-dir link, not a per-skill one. Per-skill
    /// disable is impossible here without first converting the root to
    /// per-skill links - see `skill_materialize::explode_shared_dir`.
    #[serde(default)]
    pub shared_via_whole_dir_link: bool,
    /// Violations of the agentskills.io SKILL.md spec found for this
    /// specific deployment's SKILL.md - as opposed to
    /// `InstalledSkill.spec_violations`, which is the deduped union across
    /// every deployment of the same name. Lets the UI blame the one copy
    /// that's actually broken instead of every deployment sharing the name.
    #[serde(default)]
    pub spec_violations: Vec<String>,
    /// Which invocation channels this deployment's own SKILL.md allows - see
    /// `frontmatter::invocation_policy`. Defaults to `Both`, same as
    /// `InstalledSkill.invocation`, for deployments serialized before this
    /// field existed (fixtures, cached snapshots).
    #[serde(default = "default_invocation")]
    pub invocation: InvocationPolicy,
}

impl Default for Deployment {
    fn default() -> Self {
        Self {
            id: String::new(),
            destination: default_destination(),
            owner_kind: default_owner_kind(),
            owner_id: None,
            mutability: DeploymentMutability::default(),
            backing: default_backing(),
            agent: String::new(),
            scope: String::new(),
            path: String::new(),
            is_symlink: false,
            plugin: None,
            symlink_target: None,
            resolved_path: None,
            symlink_is_broken: false,
            symlink_error: None,
            project_path: None,
            content_hash: String::new(),
            disabled: false,
            disabled_by: None,
            disabled_readers: Vec::new(),
            codex_implicit_invocation: None,
            shared_via_whole_dir_link: false,
            spec_violations: Vec::new(),
            invocation: default_invocation(),
        }
    }
}

/// Fork provenance shown on a forked skill's detail header - see
/// `skill_fork_registry::ForkRecord`, which this is a read-only projection
/// of for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkInfo {
    pub origin_tool: OriginTool,
    pub origin_source: String,
    pub repo: String,
    pub base_commit: String,
    pub forked_at: String,
}

/// Installed skill with parsed data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkill {
    pub name: String,
    pub source: String,
    pub source_type: String,
    pub source_url: Option<String>,
    pub skill_path: Option<String>,
    pub installed_at: String,
    pub updated_at: Option<String>,
    pub has_update: bool,
    /// Exact lifecycle owners whose persisted update state is newer than the
    /// installed commit. Aggregate update badges derive from this list.
    #[serde(default)]
    pub update_owner_ids: Vec<String>,
    /// Update metadata keyed by the exact lifecycle owner. New clients use
    /// this instead of pairing an action with aggregate commit metadata.
    #[serde(default)]
    pub update_owners: Vec<OwnerUpdateInfo>,
    /// The upstream commit `has_update` compares against, from the same
    /// `skill_update_check` state - for the detail header's "Update
    /// available · abc1234 · 3d ago" line. `None` unless `has_update`.
    #[serde(default)]
    pub update_commit: Option<String>,
    /// The committer date of `update_commit`, for the same line.
    #[serde(default)]
    pub update_commit_at: Option<String>,
    /// How this skill was installed - see `provenance::SourceKind`.
    pub source_kind: SourceKind,
    /// Every place this skill was found deployed on disk, one entry per
    /// agent/scope. Empty when the skill is known only from the lock file.
    #[serde(default)]
    pub deployments: Vec<Deployment>,
    /// True when the skill directory ships behavior specs/evals
    /// (a spec.md file or an evals/ directory), the getsentry/skillet pattern.
    #[serde(default)]
    pub has_spec: bool,
    /// The `description` field from SKILL.md frontmatter, when present.
    #[serde(default)]
    pub description: Option<String>,
    /// Violations of the agentskills.io SKILL.md spec found for this skill.
    /// Empty means the skill is spec-compliant.
    #[serde(default)]
    pub spec_violations: Vec<String>,
    /// Token count of SKILL.md's text (cl100k_base), from the first deployment.
    #[serde(default)]
    pub skill_md_tokens: u32,
    /// Token count of just `"{name}: {description}"`, from the first
    /// deployment - the prompt cost the model actually pays per turn.
    #[serde(default)]
    pub description_tokens: u32,
    /// Total size in bytes of the skill folder, from the first deployment.
    #[serde(default)]
    pub folder_bytes: u64,
    /// Number of files in the skill folder, from the first deployment.
    #[serde(default)]
    pub file_count: u32,
    /// sha256 over the sorted (relative path, bytes) pairs of the skill
    /// folder, from the first deployment.
    #[serde(default)]
    pub content_hash: String,
    /// Every distinct `content_hash` seen across this skill's deployments,
    /// so the UI can flag duplicates whose content has diverged.
    #[serde(default)]
    pub content_hashes: Vec<String>,
    /// RFC3339 timestamp of the newest file mtime in the skill folder, from
    /// the first deployment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
    /// Every top-level SKILL.md frontmatter key, stringified, from the first
    /// deployment.
    #[serde(default)]
    pub frontmatter_fields: BTreeMap<String, String>,
    /// True when the folder walk for the first deployment hit the
    /// 2,000-file / 64 MiB cap and stopped early.
    #[serde(default)]
    pub folder_truncated: bool,
    /// Set when `source_kind` is `Fork` - see `skill_fork_registry`.
    #[serde(default)]
    pub fork: Option<ForkInfo>,
    /// Set when this skill is a "Try for 24 hours" install still within its
    /// window - see `skill_fork_registry::TrialRecord` and `skill_trial`.
    #[serde(default)]
    pub trial: Option<TrialInfo>,
    /// Every active trial keyed by its exact deployment. `trial` remains for
    /// old clients and is populated only when there is one active trial.
    #[serde(default)]
    pub trials: Vec<TrialInfo>,
    /// True when this skill is parked (disabled globally) - see
    /// `skill_park`. Parked skills are excluded from coverage/dashboard
    /// totals and shown in their own sidebar group instead.
    #[serde(default)]
    pub parked: bool,
    /// RFC3339 timestamp of when this skill was parked, set only when `parked`.
    #[serde(default)]
    pub parked_at: Option<String>,
    /// Which invocation channels this skill allows - see
    /// `frontmatter::invocation_policy`.
    #[serde(default = "default_invocation")]
    pub invocation: InvocationPolicy,
}

fn default_invocation() -> InvocationPolicy {
    InvocationPolicy::Both
}

fn default_destination() -> SkillDestination {
    SkillDestination::PerHarness
}

fn default_owner_kind() -> LifecycleOwnerKind {
    LifecycleOwnerKind::Manual
}

fn default_backing() -> BackingRelationship {
    BackingRelationship::Independent
}

/// A trial's remaining-time projection, read-only for the frontend - see
/// `skill_fork_registry::TrialRecord`, which this is a projection of.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialInfo {
    #[serde(default)]
    pub deployment_id: String,
    pub expires_at: String,
    pub method: AddMethod,
    pub status: super::skill_fork_registry::TrialStatus,
    /// The trial's scope - needed so `keep_skill_trial`/expiry can key back
    /// into `trials` (`"global/<name>"` or `"project/<name>"`) correctly.
    pub scope: TrialScope,
    #[serde(default)]
    pub project_path: Option<String>,
}

/// Persisted update state for one exact lifecycle owner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerUpdateInfo {
    pub owner_id: String,
    #[serde(default)]
    pub latest_commit: Option<String>,
    #[serde(default)]
    pub latest_commit_at: Option<String>,
}

// ============================================================================
// Add-skill Types
// ============================================================================

/// A parsed "Source" field from the add-skill sheet - see
/// `src/lib/skill-source-parse.ts`'s `parseSkillSource`, which produces this
/// exact shape on the frontend. `#[serde(rename_all = "camelCase")]` so the
/// two sides agree on field names without either translating the other.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedSkillSource {
    pub kind: ParsedSkillSourceKind,
    pub repo: Option<String>,
    pub path: Option<String>,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub skill_name: Option<String>,
    pub url: Option<String>,
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParsedSkillSourceKind {
    Github,
    Git,
    Local,
}

/// `add_skill`'s request - see `AddSkillSheet`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddSkillRequest {
    pub source: ParsedSkillSource,
    pub method: AddMethod,
    pub destination: SkillDestination,
    pub agents: Vec<super::agents::AgentId>,
    /// Harnesses to switch off for this skill right after a successful
    /// install: readers of the Universal folder the install itself cannot
    /// avoid reaching. Unused for Per harness Copy.
    #[serde(default)]
    pub disabled_harnesses: Vec<super::agents::AgentId>,
    pub scope: InstallScope,
    pub project_path: Option<String>,
    pub trial: bool,
}

/// `add_skills`' request: one source, and the skill folders picked out of it
/// by the Add-skill sheet's picker (see `github_skill_listing`). Every other
/// field means exactly what it does on `AddSkillRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddSkillsRequest {
    pub source: ParsedSkillSource,
    pub skills: Vec<GithubSkillEntry>,
    pub method: AddMethod,
    pub destination: SkillDestination,
    pub agents: Vec<super::agents::AgentId>,
    /// Harnesses to switch off for this skill right after a successful
    /// install: readers of the Universal folder the install itself cannot
    /// avoid reaching. Unused for Per harness Copy.
    #[serde(default)]
    pub disabled_harnesses: Vec<super::agents::AgentId>,
    pub scope: InstallScope,
    pub project_path: Option<String>,
    pub trial: bool,
}

/// One skill's outcome in an `add_skills` batch. A failure never stops the
/// rest of the batch, so exactly one of `result`/`error` is set per entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddSkillOutcome {
    pub name: String,
    pub result: Option<AddSkillResult>,
    pub error: Option<String>,
}

/// `add_skill`'s result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddSkillResult {
    pub name: String,
    pub tool: String,
    pub command: String,
    pub deployments_created: Vec<String>,
    /// Set when the install itself succeeded but a follow-up step (recording
    /// the 24 h trial, or turning the skill off for a `disabled_harnesses`
    /// entry) failed - the skill is on disk and usable, it just
    /// isn't tracked for auto-expiry. The sheet shows this as a warning
    /// toast rather than treating the whole request as failed.
    #[serde(default)]
    pub warning: Option<String>,
}

// ============================================================================
// Installation Types
// ============================================================================

/// Scope for skill installation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstallScope {
    Global,
    Project,
}

/// Update or remove one deployment, or every deployment of one owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
}

/// Exact deployment plus the harness whose visibility will change. Universal
/// deployments are valid for readers that discover that scope directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessVisibilityTarget {
    pub deployment_id: String,
    pub reader_agent: super::agents::AgentId,
}

/// Installation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallResult {
    pub success: bool,
    pub skill_name: String,
    pub installed_path: Option<String>,
    pub error: Option<String>,
    /// Which CLI `update_skill` ran, for a toast that names it - "dotagents"
    /// or "skills-sh". `None` for install/remove results, which never set it.
    #[serde(default)]
    pub tool: Option<String>,
    /// The exact argv `update_skill` ran, joined with spaces, for the same
    /// toast.
    #[serde(default)]
    pub command: Option<String>,
}

/// Installation progress update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallProgress {
    pub stage: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
}
