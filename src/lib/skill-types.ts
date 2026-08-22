// ============================================================================
// Agent Studio - skill-types
// Types for skills.sh integration, multi-agent support, and shared UI state
// ============================================================================

/**
 * Toast notification shown in the corner of the app
 */
export interface Toast {
  id: string;
  type: "success" | "error" | "info" | "warning";
  title: string;
  message?: string;
  duration?: number;
}

// ============================================================================
// Agent Target Types (41 agents)
// ============================================================================

/**
 * Agent identifier matching Rust AgentId enum
 */
export type AgentId =
  | "claude-code"
  | "open-code"
  | "pi"
  | "cursor"
  | "cline"
  | "windsurf"
  | "roo-code"
  | "codex"
  | "amp"
  | "zed"
  | "void"
  | "aider"
  | "pear-ai"
  | "continue"
  | "copilot"
  | "supermaven"
  | "tabnine"
  | "sourcegraph"
  | "replit"
  | "bolt"
  | "v0"
  | "lovable"
  | "devin"
  | "goose"
  | "aide"
  | "trae"
  | "melty"
  | "cody-ai"
  | "blackbox"
  | "codeium"
  | "qodo"
  | "coderabbit"
  | "codium"
  | "sourcery"
  | "amazon-q"
  | "gemini-code"
  | "jetbrains-ai"
  | "xcode-ai"
  | "pieces"
  | "mintlify"
  | "swimm"
  | "sweep";

/**
 * Agent target with paths resolved
 */
export interface AgentTarget {
  id: AgentId;
  name: string;
  project_path: string;
  global_path: string;
}

/**
 * First-class agents featured for quick selection in the agent-target
 * picker, and the only agents whose skill directories are scanned for
 * native provenance detection (see skills/scan.rs).
 */
export const COMMON_AGENTS: AgentId[] = ["claude-code", "codex", "open-code", "pi"];

// ============================================================================
// Skills.sh API Types
// ============================================================================

/**
 * Search result from skills.sh API
 */
export interface SkillSearchResult {
  id: string;
  name: string;
  description?: string;
  installs: number;
  top_source?: string;
  author?: string;
  tags?: string[];
}

/**
 * Response from skills.sh search API
 */
export interface SkillSearchResponse {
  skills: SkillSearchResult[];
  hasMore: boolean;
}

/**
 * Paginated response from Tauri backend
 */
export interface PaginatedSkillsResponse {
  skills: SkillSearchResult[];
  has_more: boolean;
}

// ============================================================================
// Lock File Types
// ============================================================================

/**
 * How a skill made it onto disk: "skills-sh" (present in the lock file),
 * "plugin" (shipped by an agent plugin, e.g. ~/.claude/plugins/*),
 * "dotagents" (symlinked in by getsentry/dotagents), or "manual" (a plain
 * directory found on disk with no other provenance signal).
 */
export type SkillSourceKind = "skills-sh" | "plugin" | "dotagents" | "manual";

/** Badge label for each source_kind, shared by SkillBrowser and SkillDetailPanel. */
export const SOURCE_KIND_LABELS = {
  "skills-sh": "skills.sh",
  dotagents: "dotagents",
  plugin: "plugin",
  manual: "manual",
} as const satisfies Record<SkillSourceKind, string>;

/**
 * A plugin that shipped a skill, per the agent-plugins.org convention
 * (Claude Code / Codex plugin caches, or any directory with a `plugin.json`
 * manifest and a `skills/` subdirectory).
 */
export interface PluginInfo {
  name: string;
  version?: string;
  harness: string;
}

/**
 * A place a skill is deployed on disk for a specific agent.
 */
export interface Deployment {
  agent: string;
  scope: "global" | "project" | "plugin";
  path: string;
  is_symlink: boolean;
  plugin?: PluginInfo;
}

/**
 * Installed skill, merged from the lock file and a scan of the four
 * first-class agents' skill directories (see skills/scan.rs)
 */
export interface InstalledSkill {
  name: string;
  source: string;
  source_type: string;
  source_url?: string;
  skill_path?: string;
  installed_at: string;
  updated_at?: string;
  has_update: boolean;
  source_kind: SkillSourceKind;
  deployments: Deployment[];
  has_spec: boolean;
  /** The `description` field from SKILL.md frontmatter, when present. */
  description?: string;
  /** Violations of the agentskills.io SKILL.md spec. Empty means compliant. */
  spec_violations: string[];
}

// ============================================================================
// Installation Types
// ============================================================================

/**
 * Scope for skill installation
 */
export type InstallScope = "global" | "project";

/**
 * Installation request
 */
export interface InstallRequest {
  skill_source: string;
  scope: InstallScope;
  project_path?: string;
  agents: AgentId[];
}

/**
 * Installation result
 */
export interface InstallResult {
  success: boolean;
  skill_name: string;
  installed_path?: string;
  error?: string;
}

// ============================================================================
// UI State Types
// ============================================================================

/**
 * Skill store filter state
 */
export interface SkillStoreFilters {
  query: string;
  showInstalled: boolean;
  showAvailable: boolean;
  sortBy: "installs" | "name" | "recent";
}

/**
 * Skill with combined search and installed info
 */
export interface SkillWithStatus extends SkillSearchResult {
  is_installed: boolean;
  installed_info?: InstalledSkill;
}

/**
 * Installation progress state
 */
export interface InstallProgressState {
  isInstalling: boolean;
  skillName: string;
  stage: string;
  message: string;
  percent?: number;
  error?: string;
}
