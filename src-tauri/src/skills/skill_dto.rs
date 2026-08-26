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
use super::provenance::SourceKind;
use super::skill_fork_registry::{AddMethod, OriginTool, TrialScope};

/// Which of the three disable mechanisms `Deployment.disabled` came from -
/// see `skill_harness_disable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisabledBy {
    CodexConfig,
    OpencodePermission,
    ClaudeLinkRemoved,
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
    // Deserialize "source" from the skills.sh API, serialize as "top_source"
    // for the frontend (see docs/agent-skill-conventions.md's search row).
    #[serde(rename(deserialize = "source"))]
    pub top_source: Option<String>,
    pub author: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Response from skills.sh search API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSearchResponse {
    pub skills: Vec<SkillSearchResult>,
    #[serde(rename(deserialize = "hasMore"), default)]
    pub has_more: bool,
}

/// Paginated response to return to frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedSkillsResponse {
    pub skills: Vec<SkillSearchResult>,
    pub has_more: bool,
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
    /// Display name of the agent (e.g. "Claude Code")
    pub agent: String,
    pub scope: String, // "global" | "project" | "plugin"
    pub path: String,
    pub is_symlink: bool,
    /// Set when this deployment is a skill shipped by a plugin.
    #[serde(default)]
    pub plugin: Option<PluginInfo>,
    /// Canonicalized symlink target, when `is_symlink` and the target resolves.
    #[serde(default)]
    pub symlink_target: Option<String>,
    /// True when `is_symlink` but the target doesn't exist.
    #[serde(default)]
    pub symlink_is_broken: bool,
    /// Set when `is_symlink` and resolving the target failed for a reason
    /// other than "doesn't exist" (permission denied, symlink loop, etc.).
    #[serde(default)]
    pub symlink_error: Option<String>,
    /// The project directory this deployment belongs to, for project-scoped
    /// deployments. `None` for global and plugin deployments.
    #[serde(default)]
    pub project_path: Option<String>,
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
    /// Codex's own `agents/openai.yaml` `policy.allow_implicit_invocation`
    /// value, read straight off disk - note-only, doesn't affect
    /// `InstalledSkill.invocation` (that's driven by SKILL.md frontmatter).
    #[serde(default)]
    pub codex_implicit_invocation: Option<bool>,
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
    #[serde(default)]
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

/// A trial's remaining-time projection, read-only for the frontend - see
/// `skill_fork_registry::TrialRecord`, which this is a projection of.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialInfo {
    pub expires_at: String,
    pub method: AddMethod,
    /// The trial's scope - needed so `keep_skill_trial`/expiry can key back
    /// into `trials` (`"global/<name>"` or `"project/<name>"`) correctly.
    pub scope: TrialScope,
    #[serde(default)]
    pub project_path: Option<String>,
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
    pub agents: Vec<super::agents::AgentId>,
    pub scope: InstallScope,
    pub project_path: Option<String>,
    pub trial: bool,
}

/// `add_skill`'s result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddSkillResult {
    pub name: String,
    pub tool: String,
    pub command: String,
    pub deployments_created: Vec<String>,
    /// Set when the install itself succeeded but a follow-up step (recording
    /// the 24 h trial) failed - the skill is on disk and usable, it just
    /// isn't tracked for auto-expiry. The sheet shows this as a warning
    /// toast rather than treating the whole request as failed.
    #[serde(default)]
    pub warning: Option<String>,
}

// ============================================================================
// Installation Types
// ============================================================================

/// Scope for skill installation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum InstallScope {
    Global,
    Project,
}

/// Installation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRequest {
    pub skill_source: String, // e.g., "getsentry/find-bugs" or skill name
    pub scope: InstallScope,
    pub project_path: Option<String>,
    pub agents: Vec<super::agents::AgentId>,
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
    pub percent: Option<u8>,
}
