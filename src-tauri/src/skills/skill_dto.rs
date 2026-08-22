// ============================================================================
// Skills Module - skill_dto
// Serialized shapes sent to the frontend over Tauri IPC: skills.sh API
// responses, installed-skill records, and installation request/response
// types. Lock file structs live in lock_file.rs, agent identifiers in
// agents.rs, SKILL.md frontmatter in frontmatter.rs, and source provenance
// in provenance.rs.
// ============================================================================

use serde::{Deserialize, Serialize};

use super::provenance::SourceKind;

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
    // Deserialize "topSource" from API, serialize as "top_source" for frontend
    #[serde(rename(deserialize = "topSource"))]
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
}

/// Installation progress update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallProgress {
    pub stage: String,
    pub message: String,
    pub percent: Option<u8>,
}
