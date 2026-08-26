// ============================================================================
// Skill Studio - skill-types
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
// Agent Target Types (42 agents)
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
  | "sweep"
  | "grok-build";

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
 * picker. These are the install targets `npx skills --agent <id>` accepts,
 * not every harness the scanner reads: Grok Build is scanned for coverage
 * and health but is not an `npx skills` install target, so it is excluded
 * here (see AgentId::GrokBuild rejection in skills/commands.rs).
 */
export const COMMON_AGENTS: AgentId[] = ["claude-code", "codex", "open-code", "pi", "cursor"];

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
  plugin?: PluginInfo | null;
  /** Canonicalized symlink target, when `is_symlink` and the target resolves. */
  symlink_target?: string;
  /** True when `is_symlink` but the target doesn't exist. */
  symlink_is_broken: boolean;
  /** Set when `is_symlink` and resolving the target failed for a reason other than "doesn't exist". */
  symlink_error?: string;
  /** The project directory this deployment belongs to, for project-scoped deployments. */
  project_path?: string;
  /** This deployment's own sha256 content hash, empty when unreadable. */
  content_hash: string;
}

/**
 * Installed skill, merged from the lock file and a scan of the four
 * first-class agents' skill directories (see skills/skill_discovery.rs)
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
  /** The upstream commit `has_update` compares against, when `has_update` is true. */
  update_commit?: string;
  /** The committer date of `update_commit`. */
  update_commit_at?: string;
  source_kind: SkillSourceKind;
  deployments: Deployment[];
  has_spec: boolean;
  /** The `description` field from SKILL.md frontmatter, when present. */
  description?: string;
  /** Violations of the agentskills.io SKILL.md spec. Empty means compliant. */
  spec_violations: string[];
  /** Token count of SKILL.md's text (cl100k_base), from the first deployment. */
  skill_md_tokens: number;
  /** Total size in bytes of the skill folder, from the first deployment. */
  folder_bytes: number;
  /** Number of files in the skill folder, from the first deployment. */
  file_count: number;
  /** sha256 over the skill folder's contents, from the first deployment. */
  content_hash: string;
  /** Every distinct content_hash seen across this skill's deployments. */
  content_hashes: string[];
  /** RFC3339 timestamp of the newest file mtime, from the first deployment. */
  modified_at?: string;
  /** Every top-level SKILL.md frontmatter key, stringified, from the first deployment. */
  frontmatter_fields: Record<string, string>;
  /** True when the folder walk for the first deployment hit the 2,000-file / 64 MiB cap. */
  folder_truncated: boolean;
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
  /** Which CLI `updateSkill` ran - "dotagents" or "skills-sh" - for a toast that names it. */
  tool?: "dotagents" | "skills-sh";
  /** The exact argv `updateSkill` ran, joined with spaces, for the same toast. */
  command?: string;
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

// ============================================================================
// Invocation / Background Refresh Types
// ============================================================================

/**
 * One recorded skill invocation, parsed from a Claude Code transcript
 * (see skills/skill_invocations.rs).
 */
export interface SkillInvocation {
  skill: string;
  agent: string;
  at: string;
  project_path?: string;
}

/**
 * Per-skill invocation summary.
 */
export interface SkillInvocationStats {
  skill: string;
  total: number;
  last_24_hours: number;
  last_7_days: number;
  last_14_days: number;
  last_30_days: number;
  last_used?: string;
  /** Invocation counts by full project path, over the last 30 days only. */
  by_project_30_days: Record<string, number>;
  /** Per-day invocation counts, "YYYY-MM-DD" (UTC), over the last 365 days. */
  by_day: Record<string, number>;
}

/**
 * Per-day invocation counts for the heatmap (date "YYYY-MM-DD" -> count).
 */
export interface InvocationHeatmap {
  days: Record<string, number>;
}

/**
 * Everything the background refresh thread computes in one pass: installed
 * skills, discovered projects, and invocation history. See
 * skills/skill_refresh.rs.
 */
export interface SkillSnapshot {
  skills: InstalledSkill[];
  projects: string[];
  invocations: SkillInvocationStats[];
  heatmap: InvocationHeatmap;
  scanned_at: string;
  /** The newest "Test" run outcome per skill name - see `skill-run-history-types.ts`. */
  last_test_by_skill: Record<string, import("./skill-run-history-types").SkillRunSummary>;
  /** The latest background update-check result - see `skill_update_check.rs`. */
  update_check: UpdateCheckSummary;
}

/**
 * `SkillSnapshot.update_check` and `checkSkillUpdatesNow`'s return shape: a
 * flattened view of the backend's update-check store, plus a ready-to-display
 * count of skills with an update available.
 */
export interface UpdateCheckSummary {
  checked_at: string | null;
  gh_status: "ok" | "missing" | "not-logged-in" | "failed";
  message: string | null;
  updates_available: number;
}
