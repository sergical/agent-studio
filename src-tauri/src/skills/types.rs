// ============================================================================
// Skills Module - Type Definitions
// Types for skills.sh integration and multi-agent support
// ============================================================================

use serde::{Deserialize, Serialize};

// ============================================================================
// Agent Target Definitions (41 agents)
// ============================================================================

/// Agent target identifier
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum AgentId {
    ClaudeCode,
    OpenCode,
    Cursor,
    Cline,
    Windsurf,
    RooCode,
    Codex,
    Amp,
    Zed,
    Void,
    Aider,
    PearAi,
    Continue,
    Copilot,
    Supermaven,
    Tabnine,
    Sourcegraph,
    Replit,
    Bolt,
    V0,
    Lovable,
    Devin,
    Goose,
    Aide,
    Trae,
    Melty,
    CodyAi,
    Blackbox,
    Codeium,
    Qodo,
    Coderabbit,
    Codium,
    Sourcery,
    AmazonQ,
    GeminiCode,
    JetbrainsAi,
    XcodeAi,
    Pieces,
    Mintlify,
    Swimm,
    Sweep,
}

impl AgentId {
    /// Get the CLI-compatible name (kebab-case) for this agent
    pub fn cli_name(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::OpenCode => "opencode",
            Self::Cursor => "cursor",
            Self::Cline => "cline",
            Self::Windsurf => "windsurf",
            Self::RooCode => "roo-code",
            Self::Codex => "codex",
            Self::Amp => "amp",
            Self::Zed => "zed",
            Self::Void => "void",
            Self::Aider => "aider",
            Self::PearAi => "pear-ai",
            Self::Continue => "continue",
            Self::Copilot => "copilot",
            Self::Supermaven => "supermaven",
            Self::Tabnine => "tabnine",
            Self::Sourcegraph => "sourcegraph",
            Self::Replit => "replit",
            Self::Bolt => "bolt",
            Self::V0 => "v0",
            Self::Lovable => "lovable",
            Self::Devin => "devin",
            Self::Goose => "goose",
            Self::Aide => "aide",
            Self::Trae => "trae",
            Self::Melty => "melty",
            Self::CodyAi => "cody-ai",
            Self::Blackbox => "blackbox",
            Self::Codeium => "codeium",
            Self::Qodo => "qodo",
            Self::Coderabbit => "coderabbit",
            Self::Codium => "codium",
            Self::Sourcery => "sourcery",
            Self::AmazonQ => "amazon-q",
            Self::GeminiCode => "gemini-code",
            Self::JetbrainsAi => "jetbrains-ai",
            Self::XcodeAi => "xcode-ai",
            Self::Pieces => "pieces",
            Self::Mintlify => "mintlify",
            Self::Swimm => "swimm",
            Self::Sweep => "sweep",
        }
    }

    /// Get the display name for this agent
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::OpenCode => "OpenCode",
            Self::Cursor => "Cursor",
            Self::Cline => "Cline",
            Self::Windsurf => "Windsurf",
            Self::RooCode => "Roo Code",
            Self::Codex => "Codex",
            Self::Amp => "Amp",
            Self::Zed => "Zed",
            Self::Void => "Void",
            Self::Aider => "Aider",
            Self::PearAi => "Pear AI",
            Self::Continue => "Continue",
            Self::Copilot => "GitHub Copilot",
            Self::Supermaven => "Supermaven",
            Self::Tabnine => "Tabnine",
            Self::Sourcegraph => "Sourcegraph",
            Self::Replit => "Replit",
            Self::Bolt => "Bolt",
            Self::V0 => "v0",
            Self::Lovable => "Lovable",
            Self::Devin => "Devin",
            Self::Goose => "Goose",
            Self::Aide => "Aide",
            Self::Trae => "Trae",
            Self::Melty => "Melty",
            Self::CodyAi => "Cody AI",
            Self::Blackbox => "Blackbox",
            Self::Codeium => "Codeium",
            Self::Qodo => "Qodo",
            Self::Coderabbit => "CodeRabbit",
            Self::Codium => "Codium",
            Self::Sourcery => "Sourcery",
            Self::AmazonQ => "Amazon Q",
            Self::GeminiCode => "Gemini Code",
            Self::JetbrainsAi => "JetBrains AI",
            Self::XcodeAi => "Xcode AI",
            Self::Pieces => "Pieces",
            Self::Mintlify => "Mintlify",
            Self::Swimm => "Swimm",
            Self::Sweep => "Sweep",
        }
    }

    /// Get the project path for this agent (relative to project root)
    pub fn project_path(&self) -> &'static str {
        match self {
            Self::ClaudeCode => ".claude/skills",
            Self::OpenCode => ".opencode/skills",
            Self::Cursor => ".cursor/skills",
            Self::Cline => ".cline/skills",
            Self::Windsurf => ".windsurf/skills",
            Self::RooCode => ".roo-code/skills",
            Self::Codex => ".codex/skills",
            Self::Amp => ".amp/skills",
            Self::Zed => ".zed/skills",
            Self::Void => ".void/skills",
            Self::Aider => ".aider/skills",
            Self::PearAi => ".pearai/skills",
            Self::Continue => ".continue/skills",
            Self::Copilot => ".copilot/skills",
            Self::Supermaven => ".supermaven/skills",
            Self::Tabnine => ".tabnine/skills",
            Self::Sourcegraph => ".sourcegraph/skills",
            Self::Replit => ".replit/skills",
            Self::Bolt => ".bolt/skills",
            Self::V0 => ".v0/skills",
            Self::Lovable => ".lovable/skills",
            Self::Devin => ".devin/skills",
            Self::Goose => ".goose/skills",
            Self::Aide => ".aide/skills",
            Self::Trae => ".trae/skills",
            Self::Melty => ".melty/skills",
            Self::CodyAi => ".cody/skills",
            Self::Blackbox => ".blackbox/skills",
            Self::Codeium => ".codeium/skills",
            Self::Qodo => ".qodo/skills",
            Self::Coderabbit => ".coderabbit/skills",
            Self::Codium => ".codium/skills",
            Self::Sourcery => ".sourcery/skills",
            Self::AmazonQ => ".amazonq/skills",
            Self::GeminiCode => ".gemini/skills",
            Self::JetbrainsAi => ".jetbrains-ai/skills",
            Self::XcodeAi => ".xcode-ai/skills",
            Self::Pieces => ".pieces/skills",
            Self::Mintlify => ".mintlify/skills",
            Self::Swimm => ".swimm/skills",
            Self::Sweep => ".sweep/skills",
        }
    }

    /// Get the global path for this agent (relative to home directory)
    pub fn global_path(&self) -> &'static str {
        match self {
            Self::ClaudeCode => ".claude/skills",
            Self::OpenCode => ".config/opencode/skills",
            Self::Cursor => ".cursor/skills",
            Self::Cline => ".cline/skills",
            Self::Windsurf => ".windsurf/skills",
            Self::RooCode => ".roo-code/skills",
            Self::Codex => ".codex/skills",
            Self::Amp => ".amp/skills",
            Self::Zed => ".zed/skills",
            Self::Void => ".void/skills",
            Self::Aider => ".aider/skills",
            Self::PearAi => ".pearai/skills",
            Self::Continue => ".continue/skills",
            Self::Copilot => ".copilot/skills",
            Self::Supermaven => ".supermaven/skills",
            Self::Tabnine => ".tabnine/skills",
            Self::Sourcegraph => ".sourcegraph/skills",
            Self::Replit => ".replit/skills",
            Self::Bolt => ".bolt/skills",
            Self::V0 => ".v0/skills",
            Self::Lovable => ".lovable/skills",
            Self::Devin => ".devin/skills",
            Self::Goose => ".goose/skills",
            Self::Aide => ".aide/skills",
            Self::Trae => ".trae/skills",
            Self::Melty => ".melty/skills",
            Self::CodyAi => ".cody/skills",
            Self::Blackbox => ".blackbox/skills",
            Self::Codeium => ".codeium/skills",
            Self::Qodo => ".qodo/skills",
            Self::Coderabbit => ".coderabbit/skills",
            Self::Codium => ".codium/skills",
            Self::Sourcery => ".sourcery/skills",
            Self::AmazonQ => ".amazonq/skills",
            Self::GeminiCode => ".gemini/skills",
            Self::JetbrainsAi => ".jetbrains-ai/skills",
            Self::XcodeAi => ".xcode-ai/skills",
            Self::Pieces => ".pieces/skills",
            Self::Mintlify => ".mintlify/skills",
            Self::Swimm => ".swimm/skills",
            Self::Sweep => ".sweep/skills",
        }
    }

    /// Get all agent IDs
    pub fn all() -> Vec<AgentId> {
        vec![
            Self::ClaudeCode,
            Self::OpenCode,
            Self::Cursor,
            Self::Cline,
            Self::Windsurf,
            Self::RooCode,
            Self::Codex,
            Self::Amp,
            Self::Zed,
            Self::Void,
            Self::Aider,
            Self::PearAi,
            Self::Continue,
            Self::Copilot,
            Self::Supermaven,
            Self::Tabnine,
            Self::Sourcegraph,
            Self::Replit,
            Self::Bolt,
            Self::V0,
            Self::Lovable,
            Self::Devin,
            Self::Goose,
            Self::Aide,
            Self::Trae,
            Self::Melty,
            Self::CodyAi,
            Self::Blackbox,
            Self::Codeium,
            Self::Qodo,
            Self::Coderabbit,
            Self::Codium,
            Self::Sourcery,
            Self::AmazonQ,
            Self::GeminiCode,
            Self::JetbrainsAi,
            Self::XcodeAi,
            Self::Pieces,
            Self::Mintlify,
            Self::Swimm,
            Self::Sweep,
        ]
    }
}

/// Agent target with paths resolved
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTarget {
    pub id: AgentId,
    pub name: String,
    pub project_path: String,
    pub global_path: String,
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
// Lock File Types (~/.agents/.skill-lock.json)
// ============================================================================

/// Installed skill entry in lock file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkillEntry {
    pub source: String,
    #[serde(rename = "sourceType")]
    pub source_type: String,
    #[serde(rename = "sourceUrl")]
    pub source_url: String,
    #[serde(rename = "skillPath", default)]
    pub skill_path: Option<String>,
    #[serde(rename = "skillFolderHash")]
    pub skill_folder_hash: String,
    #[serde(rename = "installedAt")]
    pub installed_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// Lock file structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillLockFile {
    pub version: u32,
    pub skills: std::collections::HashMap<String, InstalledSkillEntry>,
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
    pub skill_source: String,  // e.g., "getsentry/find-bugs" or skill name
    pub scope: InstallScope,
    pub project_path: Option<String>,
    pub agents: Vec<AgentId>,
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
