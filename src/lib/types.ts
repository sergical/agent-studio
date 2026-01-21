// ============================================================================
// Agent Studio - Comprehensive Type Definitions
// NOTE: Rust uses #[serde(flatten)] on base, so fields are flattened in JSON
// ============================================================================

// ============================================================================
// Tool Type (Claude Code vs OpenCode)
// ============================================================================

/**
 * Discriminator for which coding assistant tool the entity belongs to
 */
export type ToolType = 'claude' | 'opencode';

/**
 * Tool colors for UI display
 */
export const TOOL_COLORS: Record<ToolType, string> = {
  claude: '#F97316',    // Orange
  opencode: '#1E40AF',  // Dark Blue
};

/**
 * Tool display names
 */
export const TOOL_LABELS: Record<ToolType, string> = {
  claude: 'Claude Code',
  opencode: 'OpenCode',
};

// ============================================================================
// Base Entity Fields (flattened into all entities)
// ============================================================================

/**
 * Common fields present on all entities (flattened from Rust BaseEntity)
 */
export interface BaseEntityFields {
  id: string;
  name: string;
  path: string;
  scope: string;  // "global" or "project"
  project_path: string | null;
  is_symlink: boolean;
  symlink_target: string | null;
  content: string | null;
  last_modified: number;
  tool: ToolType;  // Which tool this entity belongs to
}

/**
 * Entity type discriminator
 */
export type EntityType = 
  | 'settings'
  | 'memory'
  | 'agent'
  | 'skill'
  | 'command'
  | 'hook'
  | 'plugin'
  | 'mcp';

// ============================================================================
// Settings Entity
// ============================================================================

export interface SettingsEntity extends BaseEntityFields {
  type: 'settings';
  variant: 'global' | 'project' | 'local';
  parsed: ClaudeCodeSettings | OpenCodeSettings | null;
}

export interface ClaudeCodeSettings {
  permissions?: {
    allow?: string[];
    deny?: string[];
    ask?: string[];
    additionalDirectories?: string[];
    defaultMode?: string;
    disableBypassPermissionsMode?: string;
  };
  env?: Record<string, string>;
  model?: string;
  hooks?: HooksConfig;
  attribution?: {
    commit?: string;
    pr?: string;
  };
  sandbox?: {
    enabled?: boolean;
    autoAllowBashIfSandboxed?: boolean;
    excludedCommands?: string[];
    allowUnsandboxedCommands?: boolean;
    network?: {
      allowUnixSockets?: string[];
      allowLocalBinding?: boolean;
      httpProxyPort?: number;
      socksProxyPort?: number;
    };
    enableWeakerNestedSandbox?: boolean;
  };
  [key: string]: unknown;
}

// ============================================================================
// OpenCode Settings (opencode.json)
// ============================================================================

export interface OpenCodeSettings {
  $schema?: string;
  theme?: string;
  model?: string;
  small_model?: string;
  default_agent?: string;
  autoupdate?: boolean | 'notify';
  share?: 'manual' | 'auto' | 'disabled';
  tui?: {
    scroll_speed?: number;
    scroll_acceleration?: { enabled?: boolean };
    diff_style?: 'auto' | 'stacked';
  };
  server?: {
    port?: number;
    hostname?: string;
    mdns?: boolean;
    cors?: string[];
  };
  tools?: Record<string, boolean>;
  permission?: OpenCodePermissions;
  mcp?: Record<string, OpenCodeMcpConfig>;
  agent?: Record<string, OpenCodeAgentConfig>;
  command?: Record<string, OpenCodeCommandConfig>;
  instructions?: string[];
  formatter?: Record<string, unknown>;
  keybinds?: Record<string, string>;
  compaction?: { auto?: boolean; prune?: boolean };
  watcher?: { ignore?: string[] };
  plugin?: string[];
  disabled_providers?: string[];
  enabled_providers?: string[];
  experimental?: Record<string, unknown>;
  provider?: Record<string, unknown>;
  [key: string]: unknown;
}

export interface OpenCodePermissions {
  edit?: 'ask' | 'allow' | 'deny';
  bash?: 'ask' | 'allow' | 'deny' | Record<string, 'ask' | 'allow' | 'deny'>;
  webfetch?: 'ask' | 'allow' | 'deny';
  skill?: Record<string, 'ask' | 'allow' | 'deny'>;
  task?: Record<string, 'ask' | 'allow' | 'deny'>;
}

export interface OpenCodeMcpConfig {
  type: 'local' | 'remote';
  command?: string[];
  url?: string;
  enabled?: boolean;
  environment?: Record<string, string>;
  headers?: Record<string, string>;
  timeout?: number;
  oauth?: false | { clientId?: string; clientSecret?: string; scope?: string };
}

export interface OpenCodeAgentConfig {
  description: string;
  mode?: 'primary' | 'subagent' | 'all';
  model?: string;
  prompt?: string;
  temperature?: number;
  maxSteps?: number;
  disable?: boolean;
  hidden?: boolean;
  tools?: Record<string, boolean>;
  permission?: OpenCodePermissions;
  [key: string]: unknown;
}

export interface OpenCodeCommandConfig {
  template: string;
  description?: string;
  agent?: string;
  subtask?: boolean;
  model?: string;
}

// ============================================================================
// Memory Entity (CLAUDE.md / AGENTS.md files)
// ============================================================================

export interface MemoryEntity extends BaseEntityFields {
  type: 'memory';
  variant: 'root' | 'dotclaude' | 'dotopencode' | 'global';
}

// ============================================================================
// Agent/Subagent Entity
// ============================================================================

export interface AgentEntity extends BaseEntityFields {
  type: 'agent';
  frontmatter: AgentFrontmatter | null;
}

export interface AgentFrontmatter {
  name?: string;
  description?: string;
  tools?: string | string[];
  disallowedTools?: string | string[];
  model?: 'sonnet' | 'opus' | 'haiku' | 'inherit' | string;
  permissionMode?: 'default' | 'acceptEdits' | 'dontAsk' | 'bypassPermissions' | 'plan';
  skills?: string[];
  hooks?: HooksConfig;
  [key: string]: unknown;
}

// ============================================================================
// Skill Entity
// ============================================================================

export interface SkillEntity extends BaseEntityFields {
  type: 'skill';
  skill_dir: string;
  frontmatter: SkillFrontmatter | null;
  supporting_files: string[];
}

export interface SkillFrontmatter {
  name?: string;
  description?: string;
  'allowed-tools'?: string | string[];
  model?: 'sonnet' | 'opus' | 'haiku' | 'inherit' | string;
  context?: 'fork';
  hooks?: HooksConfig;
  'user-invocable'?: boolean;
  'disable-model-invocation'?: boolean;
  license?: string;
  compatibility?: string;
  metadata?: Record<string, string>;
  [key: string]: unknown;
}

// ============================================================================
// Slash Command Entity
// ============================================================================

export interface CommandEntity extends BaseEntityFields {
  type: 'command';
  namespace: string | null;
  frontmatter: CommandFrontmatter | null;
}

export interface CommandFrontmatter {
  description?: string;
  'allowed-tools'?: string | string[];
  'argument-hint'?: string;
  model?: string;
  context?: 'fork';
  agent?: string;
  'disable-model-invocation'?: boolean;
  hooks?: HooksConfig;
  [key: string]: unknown;
}

// ============================================================================
// Hook Entity (extracted from settings)
// ============================================================================

export interface HookEntity {
  id: string;
  type: 'hook';
  event: HookEventType;
  matcher: string | null;
  hooks: HookDefinition[];
  source: 'global' | 'project' | 'local';
  source_path: string;
  tool: ToolType;  // Which tool this entity belongs to
}

export type HookEventType = 
  | 'PreToolUse'
  | 'PermissionRequest'
  | 'PostToolUse'
  | 'Notification'
  | 'UserPromptSubmit'
  | 'Stop'
  | 'SubagentStop'
  | 'PreCompact'
  | 'SessionStart'
  | 'SessionEnd'
  | 'SubagentStart';

export interface HookDefinition {
  type: 'command' | 'prompt';
  command?: string;
  prompt?: string;
  timeout?: number;
  once?: boolean;
}

export interface HookMatcher {
  matcher?: string;
  hooks: HookDefinition[];
}

export type HooksConfig = Partial<Record<HookEventType, HookMatcher[]>>;

// ============================================================================
// Plugin Entity
// ============================================================================

export interface PluginEntity extends BaseEntityFields {
  type: 'plugin';
  plugin_dir: string;
  manifest: PluginManifest | null;
  has_commands: boolean;
  has_agents: boolean;
  has_skills: boolean;
  has_hooks: boolean;
  has_mcp: boolean;
  has_lsp: boolean;
}

export interface PluginManifest {
  name: string;
  description: string;
  version: string;
  author?: {
    name: string;
    email?: string;
    url?: string;
  };
  homepage?: string;
  repository?: string;
  license?: string;
  keywords?: string[];
  mcpServers?: Record<string, McpServerConfig>;
  [key: string]: unknown;
}

// ============================================================================
// MCP Server Entity
// ============================================================================

export interface McpServerEntity {
  id: string;
  type: 'mcp';
  name: string;
  scope: 'user' | 'local' | 'project' | 'global';
  transport: 'stdio' | 'http' | 'sse' | 'unknown';
  config: McpServerConfig;
  source_path: string;
  is_from_plugin: boolean;
  plugin_name: string | null;
  tool: ToolType;  // Which tool this entity belongs to
}

export interface McpServerConfig {
  type?: 'stdio' | 'http' | 'sse' | 'local' | 'remote';
  command?: string | string[];  // Claude uses string, OpenCode uses string[]
  args?: string[];
  url?: string;
  env?: Record<string, string>;
  environment?: Record<string, string>;  // OpenCode uses 'environment'
  headers?: Record<string, string>;
  enabled?: boolean;  // OpenCode specific
  timeout?: number;   // OpenCode specific
}

// ============================================================================
// Analysis Types
// ============================================================================

export interface DuplicateGroup {
  name: string;
  entity_type: EntityType;
  entities: DuplicateEntity[];
}

export interface DuplicateEntity {
  id: string;
  path: string;
  scope: string;
  project_path: string | null;
  precedence: number;
}

export interface SymlinkInfo {
  path: string;
  target: string;
  target_exists: boolean;
  entity_type: string | null;
  entity_id: string | null;
}

// ============================================================================
// Health Check Types
// ============================================================================

export type HealthIssueSeverity = 'error' | 'warning' | 'info';

export interface HealthIssue {
  id: string;
  severity: HealthIssueSeverity;
  category: string;
  title: string;
  description: string;
  path?: string;
  entityType?: EntityType;
  entityId?: string;
  suggestion?: string;
}

// ============================================================================
// Project Info
// ============================================================================

/** Status of a file: whether it's a file, symlink, or missing */
export type FileStatus = 'file' | 'symlink' | 'missing';

/** Configuration state type for AGENTS.md / CLAUDE.md consistency */
export type ConfigStateType =
  | 'correct'         // AGENTS.md is file, CLAUDE.md is symlink → AGENTS.md
  | 'missing_symlink' // AGENTS.md exists, CLAUDE.md missing
  | 'needs_migration' // CLAUDE.md has content, AGENTS.md missing
  | 'conflict'        // Both files exist with content
  | 'empty';          // Neither file exists

/** Detailed configuration state for a project */
export interface ConfigState {
  agents_md_status: FileStatus;
  claude_md_status: FileStatus;
  claude_md_symlink_target: string | null;
  config_state: ConfigStateType;
  can_auto_fix: boolean;
}

export interface ProjectInfo {
  path: string;
  name: string;
  has_claude_dir: boolean;
  has_opencode_dir: boolean;  // OpenCode: .opencode/ directory
  has_mcp_json: boolean;
  has_claude_md: boolean;
  has_root_claude_md: boolean;
  has_agents_md: boolean;      // OpenCode: AGENTS.md in root
  has_opencode_json: boolean;  // OpenCode: opencode.json config
  entity_counts: EntityCounts;
  config_state: ConfigState | null;
}

export interface EntityCounts {
  settings: number;
  memory: number;
  agents: number;
  skills: number;
  commands: number;
  plugins: number;
  hooks: number;
  mcp: number;
}

// ============================================================================
// Discovery Result (from Rust backend)
// ============================================================================

export interface DiscoveryResult {
  global_config_path: string;
  projects: ProjectInfo[];
  settings: SettingsEntity[];
  memory: MemoryEntity[];
  agents: AgentEntity[];
  skills: SkillEntity[];
  commands: CommandEntity[];
  hooks: HookEntity[];
  plugins: PluginEntity[];
  mcp_servers: McpServerEntity[];
  duplicates: DuplicateGroup[];
  symlinks: SymlinkInfo[];
  discovered_at: number;
}

// ============================================================================
// UI State Types
// ============================================================================

export type ViewType = 
  | 'dashboard'
  | 'project'
  | 'health'
  | 'settings'
  | 'memory'
  | 'agents'
  | 'skills'
  | 'commands'
  | 'hooks'
  | 'plugins'
  | 'mcp';

export type FilterScope = 'all' | 'global' | 'project';

export interface Toast {
  id: string;
  type: 'success' | 'error' | 'info' | 'warning';
  title: string;
  message?: string;
  duration?: number;
}

// ============================================================================
// Command Palette Item Types
// ============================================================================

export interface CommandPaletteSection {
  id: string;
  title: string;
  entityType: EntityType;
  items: CommandPaletteItem[];
}

export interface CommandPaletteItem {
  id: string;
  name: string;
  description?: string;
  scope: 'global' | 'project';
  projectName?: string;
  entityType: EntityType;
  path: string;
  isSymlink?: boolean;
  isDuplicate?: boolean;
  precedence?: number;
  tool: ToolType;  // Which tool this entity belongs to
}

// ============================================================================
// Entity Templates (for creation)
// ============================================================================

export interface EntityTemplate {
  id: string;
  name: string;
  description: string;
  entityType: EntityType;
  content: string;
  frontmatterTemplate?: Record<string, unknown>;
}

// ============================================================================
// API Types (for Tauri commands)
// ============================================================================

export interface CreateEntityParams {
  entityType: EntityType;
  name: string;
  scope: 'global' | 'project';
  projectPath?: string;
  content?: string;
  templateId?: string;
  tool: ToolType;  // Which tool to create the entity for
}

export interface UpdateEntityParams {
  entityType: EntityType;
  id: string;
  content: string;
}

export interface DeleteEntityParams {
  entityType: EntityType;
  id: string;
  path: string;
}

// ============================================================================
// Utility Types
// ============================================================================

/** Union of all entity types with base fields */
export type FlatEntity = 
  | SettingsEntity 
  | MemoryEntity 
  | AgentEntity 
  | SkillEntity 
  | CommandEntity 
  | PluginEntity;

/** Union of all displayable entities */
export type DisplayableEntity = FlatEntity | HookEntity | McpServerEntity;

/** Type guard for checking if entity has base fields (flat entities) */
export function isFlatEntity(entity: DisplayableEntity): entity is FlatEntity {
  return 'path' in entity && 'scope' in entity && 'content' in entity;
}

/** Type guard for HookEntity */
export function isHookEntity(entity: DisplayableEntity): entity is HookEntity {
  return 'event' in entity && 'hooks' in entity && 'source' in entity;
}

/** Type guard for McpServerEntity */
export function isMcpServerEntity(entity: DisplayableEntity): entity is McpServerEntity {
  return 'transport' in entity && 'config' in entity && 'source_path' in entity;
}

/** Type guard for PluginEntity */
export function isPluginEntity(entity: DisplayableEntity): entity is PluginEntity {
  return 'plugin_dir' in entity && 'has_commands' in entity && 'has_agents' in entity;
}

/** Type guard for SkillEntity */
export function isSkillEntity(entity: DisplayableEntity): entity is SkillEntity {
  return 'type' in entity && entity.type === 'skill' && 'skill_dir' in entity;
}

/** Type guard for AgentEntity */
export function isAgentEntity(entity: DisplayableEntity): entity is AgentEntity {
  return 'type' in entity && entity.type === 'agent' && 'frontmatter' in entity;
}

/** Type guard for CommandEntity */
export function isCommandEntity(entity: DisplayableEntity): entity is CommandEntity {
  return 'type' in entity && entity.type === 'command' && 'namespace' in entity;
}

/** Get display name for entity type */
export function getEntityTypeLabel(type: EntityType): string {
  const labels: Record<EntityType, string> = {
    settings: 'Settings',
    memory: 'Memory',
    agent: 'Agent',
    skill: 'Skill',
    command: 'Command',
    hook: 'Hook',
    plugin: 'Plugin',
    mcp: 'MCP Server',
  };
  return labels[type];
}

/** Get plural display name for entity type */
export function getEntityTypePluralLabel(type: EntityType): string {
  const labels: Record<EntityType, string> = {
    settings: 'Settings',
    memory: 'Memory Files',
    agent: 'Agents',
    skill: 'Skills',
    command: 'Commands',
    hook: 'Hooks',
    plugin: 'Plugins',
    mcp: 'MCP Servers',
  };
  return labels[type];
}
