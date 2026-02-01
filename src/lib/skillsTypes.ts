// ============================================================================
// Agent Studio - Skills Types
// Types for skills.sh integration and multi-agent support
// ============================================================================

// ============================================================================
// Agent Target Types (41 agents)
// ============================================================================

/**
 * Agent identifier matching Rust AgentId enum
 */
export type AgentId =
  | 'claude-code'
  | 'open-code'
  | 'cursor'
  | 'cline'
  | 'windsurf'
  | 'roo-code'
  | 'codex'
  | 'amp'
  | 'zed'
  | 'void'
  | 'aider'
  | 'pear-ai'
  | 'continue'
  | 'copilot'
  | 'supermaven'
  | 'tabnine'
  | 'sourcegraph'
  | 'replit'
  | 'bolt'
  | 'v0'
  | 'lovable'
  | 'devin'
  | 'goose'
  | 'aide'
  | 'trae'
  | 'melty'
  | 'cody-ai'
  | 'blackbox'
  | 'codeium'
  | 'qodo'
  | 'coderabbit'
  | 'codium'
  | 'sourcery'
  | 'amazon-q'
  | 'gemini-code'
  | 'jetbrains-ai'
  | 'xcode-ai'
  | 'pieces'
  | 'mintlify'
  | 'swimm'
  | 'sweep';

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
 * Common agents for quick selection
 */
export const COMMON_AGENTS: AgentId[] = [
  'claude-code',
  'cursor',
  'cline',
  'windsurf',
  'open-code',
];

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
 * Installed skill from lock file
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
}

// ============================================================================
// Installation Types
// ============================================================================

/**
 * Scope for skill installation
 */
export type InstallScope = 'global' | 'project';

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
  sortBy: 'installs' | 'name' | 'recent';
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
