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
  /** A secondary button, e.g. trial-expiry's "Restore" - see `ToastContainer`. */
  action?: { label: string; onClick: () => void };
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
 * Paginated response from Tauri backend
 */
export interface PaginatedSkillsResponse {
  skills: SkillSearchResult[];
  has_more: boolean;
}

/**
 * skills.sh v1 skill details, including the skill's markdown body.
 */
export interface SkillDetails {
  id: string;
  source: string;
  slug: string;
  installs: number;
  hash: string;
  skill_md: string | null;
}

// ============================================================================
// Lock File Types
// ============================================================================

/**
 * How a skill made it onto disk: "skills-sh" (present in the lock file),
 * "plugin" (shipped by an agent plugin, e.g. ~/.claude/plugins/*),
 * "dotagents" (symlinked in by getsentry/dotagents), "in-repo" (a plain
 * directory inside a git working tree), "manual" (a plain directory found on
 * disk with no other provenance signal), or "fork" (detached from its
 * dotagents/skills.sh ledger via Fork - see `ForkInfo`).
 */
export type SkillSourceKind = "skills-sh" | "plugin" | "dotagents" | "in-repo" | "manual" | "fork";

/** Badge label for each source_kind, shared by SkillBrowser and SkillDetailPanel. */
export const SOURCE_KIND_LABELS = {
  "skills-sh": "skills.sh",
  dotagents: "dotagents",
  plugin: "plugin",
  "in-repo": "in repo",
  manual: "manual",
  fork: "fork",
} as const satisfies Record<SkillSourceKind, string>;

/**
 * A forked skill's origin, set when `InstalledSkill.source_kind === "fork"`.
 * See `skill_fork_registry::ForkRecord` on the Rust side.
 */
export interface ForkInfo {
  origin_tool: "dotagents" | "skills-sh";
  origin_source: string;
  repo: string;
  base_commit: string;
  forked_at: string;
}

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
 * Which mechanism `Deployment.disabled` came from - see
 * `skill_harness_disable.rs`. The first three are native per-harness
 * switches; `studio-moved` is the universal fallback that renames the
 * deployment aside into a `.skill-studio-disabled/` holding directory.
 */
export type DisabledBy =
  | "codex-config"
  | "opencode-permission"
  | "claude-link-removed"
  | "studio-moved";

/**
 * A place a skill is deployed on disk for a specific agent.
 */
export interface Deployment {
  agent: string;
  scope: "global" | "project" | "plugin" | "parked";
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
  /**
   * Canonical path of this deployment's directory when it differs from
   * `path` - set when any ancestor is a symlink (e.g. a `.claude/skills`
   * root linked to `.agents/skills`), so the frontend can tell "same folder
   * through a linked root" from a separate copy.
   */
  resolved_path?: string;
  /** This deployment's own sha256 content hash, empty when unreadable. */
  content_hash: string;
  /** True when this deployment is disabled for its harness - see `DisabledBy`. */
  disabled: boolean;
  /** Which mechanism `disabled` came from, unset when not disabled. */
  disabled_by?: DisabledBy;
  /** For the shared-root deployment only: native shared-root reader agent ids disabled via their own mechanism ("codex", "open-code"). */
  disabled_readers?: string[];
  /** Codex's own `agents/openai.yaml` `policy.allow_implicit_invocation` value - note-only. */
  codex_implicit_invocation?: boolean;
  /**
   * True when this deployment's skills root is itself a symlink resolving
   * into the shared `.agents/skills` folder (a whole-dir link, not a
   * per-skill one) - per-skill disable needs a materialize step first.
   */
  shared_via_whole_dir_link?: boolean;
}

/**
 * Which invocation channels a skill allows - see
 * `frontmatter::invocation_policy` on the Rust side.
 */
export type InvocationPolicy = "both" | "user-only" | "model-only";

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
  /** Token count of just `"name: description"`, from the first deployment - the prompt cost the model actually pays per turn. */
  description_tokens: number;
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
  /** Set when `source_kind === "fork"`. */
  fork?: ForkInfo;
  /** Set when this skill is a "Try for 24 hours" install still within its window. */
  trial?: TrialInfo;
  /** True when parked (disabled globally) - see `skill_park.rs`. */
  parked: boolean;
  /** RFC3339 timestamp of when this skill was parked, set only when `parked`. */
  parked_at?: string;
  /** Which invocation channels this skill allows, from SKILL.md frontmatter. */
  invocation: InvocationPolicy;
}

/**
 * A trial's remaining-time projection - see
 * `skill_fork_registry::TrialRecord` on the Rust side.
 */
export interface TrialInfo {
  expires_at: string;
  method: AddMethod;
  /** The trial's scope - `keepSkillTrial` needs it to key back into `trials` correctly. */
  scope: InstallScope;
  project_path?: string;
}

/**
 * Hours left until `expiresAt`, rounded down, for the trial chip - see
 * `TrialInfo`. Negative once expired.
 */
export function trialHoursLeft(expiresAt: string): number {
  return Math.floor((new Date(expiresAt).getTime() - Date.now()) / (60 * 60 * 1000));
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
// Add-skill Types
// ============================================================================

/** How `addSkill` installed a skill - see `AddSkillSheet`. */
export type AddMethod = "dotagents" | "skills-sh" | "copy";

/**
 * `addSkill`'s request. `source` is produced verbatim by
 * `parseSkillSource` - see `skill-source-parse.ts`.
 */
export interface AddSkillRequest {
  source: import("./skill-source-parse").ParsedSkillSource;
  method: AddMethod;
  agents: AgentId[];
  scope: InstallScope;
  project_path?: string;
  trial: boolean;
}

/** `addSkill`'s result. */
export interface AddSkillResult {
  name: string;
  tool: string;
  command: string;
  deployments_created: string[];
  /** Set when the install succeeded but recording the 24 h trial failed. */
  warning?: string;
}

/**
 * `forkSkill`'s return shape - the fork record just written to
 * `~/.agents/skill-studio.json`. See `skill_fork_registry::ForkRecord`.
 */
export interface ForkRecord {
  forked_at: string;
  origin_tool: "dotagents" | "skills-sh";
  origin_source: string;
  repo: string;
  path: string;
  declared_ref?: string;
  base_commit: string;
}

/**
 * `pullForkUpstream`'s return shape - a three-way merge summary. See
 * `skill_fork::PullResult`.
 */
export interface PullResult {
  from_commit: string;
  to_commit: string;
  merged: string[];
  conflicts: string[];
  added: string[];
  removed: string[];
  unchanged: number;
  /** Set to "Already up to date" when nothing moved upstream; `null` otherwise. */
  message: string | null;
}

// ============================================================================
// Share Pack Types
// ============================================================================

/**
 * One skill to bundle into a pack: `name` is the skill's directory name,
 * `path` is the exact deployment directory to bundle from (a row's
 * `Deployment.path`) - see `skill_pack::PackMemberInput`.
 */
export interface PackMember {
  name: string;
  path: string;
}

/**
 * One share pack under `~/.agents/packs/<name>`, as recorded in
 * `~/.agents/skill-studio.json`. See `skill_pack::PackInfo`.
 */
export interface PackInfo {
  name: string;
  created_at: string;
  dir: string;
  /** `null`/`undefined` until `publishSkillPack` succeeds for the first time. */
  repo?: string;
  skills: string[];
}

/** `updateSkillPack`'s return shape - whether the rebuilt tree differed from the last commit. */
export interface UpdatePackResult {
  changed: boolean;
  pack: PackInfo;
}

/**
 * `importSkillPack`'s return shape: which names came from the repo's own
 * `skills/` tree (`--all`) versus a `[[skills]]` row pointing elsewhere, and
 * any per-row failures - a partial import still reports what worked.
 */
export interface ImportResult {
  bundled: string[];
  referenced: string[];
  errors: string[];
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
  /** Which OpenCode config format is present, `undefined` when neither exists. */
  opencode_config_kind?: "json" | "jsonc";
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

/**
 * One row of the event store's History (see docs/spec-event-store.md and
 * `SkillEventDto` in skills/skill_dto.rs). `kind` is one of the v1 event
 * kinds (`install`, `remove`, `unlink_harness`, `explode_shared_dir`, ...).
 */
export interface SkillEvent {
  id: string;
  ts: string;
  kind: string;
  skill: string;
  harness?: string;
  scope?: "global" | "project";
  project_path?: string;
  status: "pending" | "done" | "failed" | "interrupted";
  /** True when this event has an inverse, hasn't already been undone, and its status allows a restore. */
  restorable: boolean;
  reverted_by?: string;
  /** Absolute path to this event's backup directory, for a "Reveal in Finder" action. */
  backup_path?: string;
}
