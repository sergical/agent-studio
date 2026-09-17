// ============================================================================
// Skills Module - skill_dto
// Serialized shapes sent to the frontend over Tauri IPC: skills.sh API
// responses, installed-skill records, and installation request/response
// types. Lock file structs live in skill_lock_file.rs in the core crate, agent identifiers in
// skill_agents.rs, SKILL.md frontmatter in skill_document.rs, and source provenance
// in provenance.rs.
// ============================================================================

use serde::{Deserialize, Serialize};

use super::github_skill_listing::GithubSkillEntry;
use super::skill_deployment::SkillDestination;
use super::skill_fork_registry::AddMethod;

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
    /// False when force restore could cross an independent Copy boundary.
    pub force_restorable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversal_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_action: Option<String>,
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
// skill_lock_file.rs in the core crate, next to the code that reads them.

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

pub use skill_studio_core::skill_deployment::InstallScope;
pub use skill_studio_core::skill_inventory::{
    Deployment, DisabledBy, ForkInfo, InstalledSkill, OwnerUpdateInfo, TrialInfo,
};
pub use skill_studio_core::skill_plugins::PluginInfo;
