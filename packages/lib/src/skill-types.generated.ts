// ============================================================================
// GENERATED FILE - do not edit by hand.
// Produced by `npm run types:generate` (apps/desktop/scripts/generate-types.mjs)
// from the Rust wire types in apps/desktop/src-tauri/src/skills/*.rs, via the
// `schema` binary and json-schema-to-typescript. Re-run that command after
// changing a #[derive(JsonSchema)] struct or enum; `npm run check` fails if
// this file drifts from the Rust source of truth.
// ============================================================================

/**
 * Which mechanism `Deployment.disabled` came from - see
 * `skill_harness_disable`. The first three are native per-harness switches;
 * `StudioMoved` is the universal fallback that renames the deployment's
 * directory aside into a `.skill-studio-disabled/` holding directory in its
 * skills root, for harnesses with no native switch.
 */
export type DisabledBy =
  | ("codex-config" | "opencode-permission" | "claude-link-removed" | "studio-moved")
  | "claude-plugin-disabled";
/**
 * Which CLI a forked skill was originally managed by.
 */
export type OriginTool = "dotagents" | "skills-sh";
/**
 * How `add_skill` installed a skill - shared by `AddSkillRequest.method` and
 * `TrialRecord.method`, since a trial's expiry step needs to know which tool
 * (if any) owns the skill it's about to remove.
 */
export type AddMethod = "dotagents" | "skills-sh" | "copy";
/**
 * Durable state for trial expiry. `Expiring` prevents an interrupted CLI
 * removal from matching a later installation at the same path.
 */
export type TrialStatus = "active" | "expiring" | "recovery-required";
/**
 * One of the four first-class agents a skill run can target.
 */
export type HarnessId = "claude-code" | "codex" | "open-code" | "pi";
/**
 * Which `OpenCode` config format is present, so a caller can tell the user
 * to hand-edit a `.jsonc` file rather than silently showing no disables.
 */
export type OpencodeConfigKind = "json" | "jsonc";
/**
 * Agent target identifier
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
export type ParsedSkillSourceKind = "github" | "git" | "local";
/**
 * Where a skill is installed relative to harness folders. Universal owns
 * `.agents/skills`; Per harness owns an independent copy in one harness dir
 * and never writes `.agents/skills`.
 */
export type SkillDestination = "universal" | "per-harness";
/**
 * Scope for skill installation
 */
export type InstallScope = "global" | "project";
export type FrontmatterRepairApplyMode = "apply-fix" | "fix-installed-copy" | "fork-and-fix";
/**
 * Pack import either completes immediately or pauses for explicit trust.
 */
export type PackImportPreflightResult =
  | {
      result: ImportResult;
      status: "imported";
    }
  | {
      identities: string[];
      confirmation_token: string;
      status: "needs-trust";
    };
/**
 * How a skill use started.
 */
export type SkillTrigger = "user" | "agent" | "file_read";
/**
 * Where a [`ProjectFolder`] came from - decides which action the row offers.
 */
export type ProjectFolderSource = "discovered" | "added";
/**
 * Kebab-case harness identifier, for example `claude-code` or `open-code`.
 *
 * Invariant: the string is the serde wire name used by the desktop app
 * today. `open-code` is canonical; `opencode` is only a CLI binary name and
 * is never stored in an `AgentId`.
 */
export type AgentIdentity = string;
/**
 * One repair `fix_skill` applied.
 */
export type FixApplied = {
  /**
   * Deployment written.
   */
  deployment_id: string;
  /**
   * History event.
   */
  event_id: string;
  kind: "frontmatter_repair";
};

/**
 * Everything the frontend needs about installed skills, discovered
 * projects, and invocation history, built together in one background pass.
 */
export interface SkillSnapshot {
  /**
   * Process-local publication order. Zero is reserved for snapshots read
   * from older serialized data that predates revisions.
   */
  revision: number;
  skills: InstalledSkill[];
  projects: string[];
  invocations: SkillInvocationStats[];
  heatmap: InvocationHeatmap;
  scanned_at: string;
  /**
   * The newest "Test" run outcome per skill, read cheaply from
   * `skill_run_history::read_last_test_index` - not affected by the
   * invocations-only rebuild path, only refreshed on a full rebuild.
   */
  last_test_by_skill: {
    [k: string]: SkillRunSummary;
  };
  update_check: UpdateCheckSummary;
  /**
   * Which `OpenCode` config format is present, if any - `None` when
   * `OpenCode` isn't configured, `Some(Jsonc)` when Skill Studio can only
   * read (not write) its per-skill disables. See
   * `skill_studio_core::opencode_config::detect_config_kind`.
   */
  opencode_config_kind: OpencodeConfigKind | null;
  /**
   * True when the core scan's read budget was exceeded before every root
   * could be reached - `skills`/`projects` may be missing entries from
   * the roots named in `scan_observations`. See `core_scan_installed_skills`.
   */
  scan_partial: boolean;
  /**
   * Human-readable notes about roots the scan could not reach, each
   * prefixed by a display of the root it is about.
   */
  scan_observations: string[];
  /**
   * Path prefixes this run could not read: whole roots, or single skill
   * directories whose SKILL.md was unreadable. `rebuild_snapshot_now`
   * folds a partial scan's carried-over deployments against them, and the
   * partial-scan banner counts them as locations.
   */
  unread_roots: string[];
}
/**
 * Installed skill with parsed data
 */
export interface InstalledSkill {
  name: string;
  source: string;
  source_type: string;
  source_url: string | null;
  skill_path: string | null;
  installed_at: string;
  updated_at: string | null;
  has_update: boolean;
  /**
   * Exact lifecycle owners whose persisted update state is newer than the
   * installed commit. Aggregate update badges derive from this list.
   */
  update_owner_ids: string[];
  /**
   * Update metadata keyed by the exact lifecycle owner. New clients use
   * this instead of pairing an action with aggregate commit metadata.
   */
  update_owners: OwnerUpdateInfo[];
  /**
   * The upstream commit `has_update` compares against, from the same
   * `skill_update_check` state - for the detail header's "Update
   * available · abc1234 · 3d ago" line. `None` unless `has_update`.
   */
  update_commit: string | null;
  /**
   * The committer date of `update_commit`, for the same line.
   */
  update_commit_at: string | null;
  /**
   * How this skill was installed - see `skill_studio_core::identity::SourceKind`.
   */
  source_kind: "dotagents" | "plugin" | "skills-sh" | "in-repo" | "manual" | "fork";
  /**
   * Every place this skill was found deployed on disk, one entry per
   * agent/scope. Empty when the skill is known only from the lock file.
   */
  deployments: Deployment[];
  /**
   * True when the skill directory ships behavior specs/evals
   * (a spec.md file or an evals/ directory), the getsentry/skillet pattern.
   */
  has_spec: boolean;
  /**
   * The `description` field from SKILL.md frontmatter, when present.
   */
  description: string | null;
  /**
   * Violations of the agentskills.io SKILL.md spec found for this skill.
   * Empty means the skill is spec-compliant.
   */
  spec_violations: string[];
  /**
   * Token count of SKILL.md's text (`cl100k_base`), from the first deployment.
   */
  skill_md_tokens: number;
  /**
   * Token count of just `"{name}: {description}"`, from the first
   * deployment - the prompt cost the model actually pays per turn.
   */
  description_tokens: number;
  /**
   * Total size in bytes of the skill folder, from the first deployment.
   */
  folder_bytes: number;
  /**
   * Number of files in the skill folder, from the first deployment.
   */
  file_count: number;
  /**
   * sha256 over the sorted (relative path, bytes) pairs of the skill
   * folder, from the first deployment.
   */
  content_hash: string;
  /**
   * Every distinct `content_hash` seen across this skill's deployments,
   * so the UI can flag duplicates whose content has diverged.
   */
  content_hashes: string[];
  /**
   * RFC3339 timestamp of the newest file mtime in the skill folder, from
   * the first deployment.
   */
  modified_at?: string | null;
  /**
   * Every top-level SKILL.md frontmatter key, stringified, from the first
   * deployment.
   */
  frontmatter_fields: {
    [k: string]: string;
  };
  /**
   * True when the folder walk for the first deployment hit the
   * 2,000-file / 64 MiB cap and stopped early.
   */
  folder_truncated: boolean;
  /**
   * Set when `source_kind` is `Fork` - see `skill_fork_registry`.
   */
  fork: ForkInfo | null;
  /**
   * Set when this skill is a "Try for 24 hours" install still within its
   * window - see `skill_fork_registry::TrialRecord` and `skill_trial`.
   */
  trial: TrialInfo | null;
  /**
   * Every active trial keyed by its exact deployment. `trial` remains for
   * old clients and is populated only when there is one active trial.
   */
  trials: TrialInfo[];
  /**
   * True when this skill is parked (disabled globally) - see
   * `skill_park`. Parked skills are excluded from coverage/dashboard
   * totals and shown in their own sidebar group instead.
   */
  parked: boolean;
  /**
   * RFC3339 timestamp of when this skill was parked, set only when `parked`.
   */
  parked_at: string | null;
  /**
   * Which invocation channels this skill allows - see
   * `frontmatter::invocation_policy`.
   */
  invocation: "both" | "user-only" | "model-only";
}
/**
 * Persisted update state for one exact lifecycle owner.
 */
export interface OwnerUpdateInfo {
  owner_id: string;
  latest_commit: string | null;
  latest_commit_at: string | null;
}
/**
 * Where a skill is deployed on disk for a specific agent
 */
export interface Deployment {
  /**
   * Stable id (`dep:v1/...`) for exact mutations. Empty only on
   * lock-file-only records that have no on-disk path.
   */
  id: string;
  /**
   * Universal (`.agents/skills`) or Per harness. Compatibility still
   * serializes the scanner label `shared` on `agent`.
   */
  destination: "universal" | "per-harness";
  /**
   * The owner allowed to change a deployment. Read-only kinds use `None`.
   */
  owner_kind:
    | "skills-sh"
    | "dotagents"
    | "copy"
    | "fork"
    | "plugin"
    | "in-repo"
    | "manual"
    | "wildcard-dotagents"
    | "ambiguous";
  /**
   * `owner:v1/...` when a matching ledger owns this deployment.
   */
  owner_id?: string | null;
  /**
   * Whether Skill Studio may mutate this deployment through an owner adapter.
   */
  mutability: "mutable" | "read-only";
  /**
   * How this deployment relates to a Universal folder of the same skill.
   */
  backing:
    | {
        kind: "canonical";
      }
    | {
        deployment_id: string;
        kind: "linked-to";
      }
    | {
        kind: "independent";
      };
  /**
   * Display name of the agent (e.g. "Claude Code"). Universal roots
   * still use the compatibility label `shared`.
   */
  agent: string;
  scope: string;
  path: string;
  is_symlink: boolean;
  /**
   * Set when this deployment is a skill shipped by a plugin.
   */
  plugin?: PluginInfo | null;
  /**
   * Canonicalized symlink target, when `is_symlink` and the target resolves.
   */
  symlink_target?: string | null;
  /**
   * True when `is_symlink` but the target doesn't exist.
   */
  symlink_is_broken: boolean;
  /**
   * Set when `is_symlink` and resolving the target failed for a reason
   * other than "doesn't exist" (permission denied, symlink loop, etc.).
   */
  symlink_error?: string | null;
  /**
   * The project directory this deployment belongs to, for project-scoped
   * deployments. `None` for global and plugin deployments.
   */
  project_path?: string | null;
  /**
   * Canonical path of this deployment's directory when it differs from
   * `path` - set when any ancestor is a symlink (e.g. a `.claude/skills`
   * root linked to `.agents/skills`), so the frontend can tell "same
   * folder through a linked root" from a separate copy.
   */
  resolved_path?: string | null;
  /**
   * This deployment's own sha256 content hash, empty when unreadable
   * (e.g. a broken symlink). Lets the UI point at which specific copies
   * of a duplicated skill differ, not just the skill as a whole.
   */
  content_hash: string;
  /**
   * True when this specific deployment is disabled for its harness (as
   * opposed to parked, which removes the skill from every harness at
   * once) - see `skill_harness_disable`.
   */
  disabled: boolean;
  /**
   * Which mechanism `disabled` came from, `None` when not disabled.
   */
  disabled_by: DisabledBy | null;
  /**
   * For a shared-root deployment (`agent == "shared"`) only: agent ids among
   * the native shared-root readers whose own mechanism disables this skill
   * (Codex config / `OpenCode` permission deny) - `"codex"`, `"open-code"`.
   * Always empty for other deployments.
   */
  disabled_readers?: string[];
  /**
   * Codex's own `agents/openai.yaml` `policy.allow_implicit_invocation`
   * value, read straight off disk - note-only, doesn't affect
   * `InstalledSkill.invocation` (that's driven by SKILL.md frontmatter).
   */
  codex_implicit_invocation: boolean | null;
  /**
   * True when this deployment's skills root is itself a symlink resolving
   * into the shared `.agents/skills` folder (e.g. `~/.claude/skills ->
   * ../.agents/skills`) - a whole-dir link, not a per-skill one. Per-skill
   * disable is impossible here without first converting the root to
   * per-skill links - see `skill_materialize::explode_shared_dir`.
   */
  shared_via_whole_dir_link: boolean;
  /**
   * Violations of the agentskills.io SKILL.md spec found for this
   * specific deployment's SKILL.md - as opposed to
   * `InstalledSkill.spec_violations`, which is the deduped union across
   * every deployment of the same name. Lets the UI blame the one copy
   * that's actually broken instead of every deployment sharing the name.
   */
  spec_violations: string[];
  /**
   * Which invocation channels this deployment's own SKILL.md allows - see
   * `frontmatter::invocation_policy`. Defaults to `Both`, same as
   * `InstalledSkill.invocation`, for deployments serialized before this
   * field existed (fixtures, cached snapshots).
   */
  invocation: "both" | "user-only" | "model-only";
}
/**
 * A plugin that shipped a skill, per the agent-plugins.org convention
 * (Claude Code / Codex plugin caches, or any directory with a `plugin.json`
 * manifest and a `skills/` subdirectory).
 */
export interface PluginInfo {
  name: string;
  version: string | null;
  /**
   * Which agent's plugin system this came from, e.g. "Claude Code", "Codex".
   */
  harness: string;
  /**
   * Marketplace directory name.
   */
  marketplace: string;
  /**
   * `"<plugin>@<marketplace>"`, the id the harness's plugin CLI expects.
   */
  id: string;
}
/**
 * Fork provenance shown on a forked skill's detail header - see
 * `skill_fork_registry::ForkRecord`, which this is a read-only projection
 * of for the frontend.
 */
export interface ForkInfo {
  origin_tool: OriginTool;
  origin_source: string;
  repo: string;
  base_commit: string;
  forked_at: string;
}
/**
 * A trial's remaining-time projection, read-only for the frontend - see
 * `skill_fork_registry::TrialRecord`, which this is a projection of.
 */
export interface TrialInfo {
  deployment_id: string;
  expires_at: string;
  method: AddMethod;
  status: TrialStatus;
  /**
   * The trial's scope - needed so `keep_skill_trial`/expiry can key back
   * into `trials` (`"global/<name>"` or `"project/<name>"`) correctly.
   */
  scope: "global" | "project";
  project_path: string | null;
}
/**
 * Per-skill use summary sent to the frontend.
 */
export interface SkillInvocationStats {
  /**
   * The skill's name.
   */
  skill: string;
  /**
   * Total counted uses across every cached transcript.
   */
  total: number;
  /**
   * Counted uses in the last 24 hours.
   */
  last_24_hours: number;
  /**
   * Counted uses in the last 7 days.
   */
  last_7_days: number;
  /**
   * Counted uses in the last 14 days.
   */
  last_14_days: number;
  /**
   * Counted uses in the last 30 days.
   */
  last_30_days: number;
  /**
   * The most recent use's timestamp, RFC 3339.
   */
  last_used: string | null;
  /**
   * Use counts by full project path, over the last 30 days only.
   */
  by_project_30_days: {
    [k: string]: number;
  };
  /**
   * Per-day use counts, "YYYY-MM-DD" (UTC), over the last 365 days.
   */
  by_day: {
    [k: string]: number;
  };
  /**
   * Use counts by harness id, over the last 30 days only.
   */
  by_harness_30_days: {
    [k: string]: number;
  };
  by_trigger_30_days: SkillTriggerCounts;
  /**
   * Hourly use buckets, grouped by (hour, harness, trigger, project),
   * over the last 365 days.
   */
  by_hour: SkillUseHour[];
}
/**
 * Use counts by trigger, over the last 30 days only.
 */
export interface SkillTriggerCounts {
  /**
   * Uses the user typed.
   */
  user: number;
  /**
   * Uses the model called as a tool.
   */
  agent: number;
  /**
   * Uses the model triggered by reading `SKILL.md` directly.
   */
  file_read: number;
}
/**
 * One hour's worth of uses for one (harness, trigger, project) combination,
 * for the Activity page's hourly heatmap and day details.
 */
export interface SkillUseHour {
  /**
   * Whole hours since the Unix epoch, UTC.
   */
  hour: number;
  /**
   * Which harness recorded these uses.
   */
  harness: string;
  /**
   * How these uses started.
   */
  trigger: "user" | "agent" | "file_read";
  /**
   * The project directory these uses happened in, if recorded.
   */
  project_path: string | null;
  /**
   * Counted uses in this hour for this (harness, trigger, project).
   */
  count: number;
}
/**
 * Per-day use counts for the heatmap (date "YYYY-MM-DD" -> count).
 */
export interface InvocationHeatmap {
  /**
   * Counted uses per day.
   */
  days: {
    [k: string]: number;
  };
}
/**
 * The cheap per-skill index `build_snapshot` reads for every skill's
 * dashboard/list row, written alongside every full record.
 */
export interface SkillRunSummary {
  at: string;
  harness: HarnessId;
  passed: boolean | null;
}
/**
 * The latest background update-check result - see `skill_update_check`.
 */
export interface UpdateCheckSummary {
  checked_at: string | null;
  gh_status: string;
  message: string | null;
  updates_available: number;
}
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
 * What the Add Skill sheet needs before it can pick sensible defaults.
 */
export interface AddMethodDefaults {
  /**
   * Whether `npx` (what every dotagents command shells out to) resolves
   * on `PATH` - dotagents can't run at all without it.
   */
  dotagents_installed: boolean;
  /**
   * Whether `~/.agents/.skill-lock.json` exists - skills.sh has been used
   * to install at least one skill on this machine before.
   */
  has_skill_lock: boolean;
  /**
   * Every first-class agent whose own config directory exists on this
   * machine, in `AgentId`'s declaration order - see `harness_config_dirs`.
   */
  installed_harnesses: AgentId[];
  /**
   * Whether `~/.claude/skills` is a symlink into the shared folder -
   * true when Claude Code already reads `.agents/skills` on its own,
   * false when it's a real directory (or doesn't exist yet).
   */
  claude_reads_shared_folder: boolean;
}
/**
 * Paginated response to return to frontend
 */
export interface PaginatedSkillsResponse {
  skills: SkillSearchResult[];
  has_more: boolean;
}
/**
 * Search result from skills.sh API
 */
export interface SkillSearchResult {
  id: string;
  name: string;
  description: string | null;
  installs: number;
  top_source: string | null;
  author: string | null;
  tags: string[] | null;
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
  /**
   * SKILL.md (or AGENTS.md fallback) contents, when the payload has one.
   */
  skill_md: string | null;
}
/**
 * How discovery requests reach skills.sh - see `api::resolve_skills_sh_access`.
 * `"direct"` means a developer-override key is configured (`server_url` is
 * `None`); `"server"` means requests go through the local Skill Studio
 * server at `server_url`.
 */
export interface SkillsShAccessInfo {
  mode: string;
  server_url: string | null;
}
/**
 * Installation result
 */
export interface InstallResult {
  success: boolean;
  skill_name: string;
  installed_path: string | null;
  error: string | null;
  /**
   * Which CLI `update_skill` ran, for a toast that names it - "dotagents"
   * or "skills-sh". `None` for install/remove results, which never set it.
   */
  tool: string | null;
  /**
   * The exact argv `update_skill` ran, joined with spaces, for the same
   * toast.
   */
  command: string | null;
}
/**
 * Exact deployment plus the harness whose visibility will change. Universal
 * deployments are valid for readers that discover that scope directly.
 */
export interface HarnessVisibilityTarget {
  deployment_id: string;
  reader_agent: AgentId;
}
/**
 * `add_skill`'s request - see `AddSkillSheet`.
 */
export interface AddSkillRequest {
  source: ParsedSkillSource;
  method: AddMethod;
  destination: SkillDestination;
  agents: AgentId[];
  /**
   * Harnesses to switch off for this skill right after a successful
   * install: readers of the Universal folder the install itself cannot
   * avoid reaching. Unused for Per harness Copy.
   */
  disabled_harnesses: AgentId[];
  scope: InstallScope;
  project_path: string | null;
  trial: boolean;
}
/**
 * A parsed "Source" field from the add-skill sheet - see
 * `src/lib/skill-source-parse.ts`'s `parseSkillSource`, which produces this
 * exact shape on the frontend. `#[serde(rename_all = "camelCase")]` so the
 * two sides agree on field names without either translating the other.
 */
export interface ParsedSkillSource {
  kind: ParsedSkillSourceKind;
  repo: string | null;
  path: string | null;
  ref: string | null;
  skillName: string | null;
  url: string | null;
  localPath: string | null;
}
/**
 * `add_skills`' request: one source, and the skill folders picked out of it
 * by the Add-skill sheet's picker (see `github_skill_listing`). Every other
 * field means exactly what it does on `AddSkillRequest`.
 */
export interface AddSkillsRequest {
  source: ParsedSkillSource;
  skills: GithubSkillEntry[];
  method: AddMethod;
  destination: SkillDestination;
  agents: AgentId[];
  /**
   * Harnesses to switch off for this skill right after a successful
   * install: readers of the Universal folder the install itself cannot
   * avoid reaching. Unused for Per harness Copy.
   */
  disabled_harnesses: AgentId[];
  scope: InstallScope;
  project_path: string | null;
  trial: boolean;
}
/**
 * One skill folder inside a repo: `path` is repo-relative and `""` for a
 * `SKILL.md` at the repo root.
 */
export interface GithubSkillEntry {
  name: string;
  path: string;
}
/**
 * One skill's outcome in an `add_skills` batch. A failure never stops the
 * rest of the batch, so exactly one of `result`/`error` is set per entry.
 */
export interface AddSkillOutcome {
  name: string;
  result: AddSkillResult | null;
  error: string | null;
}
/**
 * `add_skill`'s result.
 */
export interface AddSkillResult {
  name: string;
  tool: string;
  command: string;
  deployments_created: string[];
  /**
   * Set when the install itself succeeded but a follow-up step (recording
   * the 24 h trial, or turning the skill off for a `disabled_harnesses`
   * entry) failed - the skill is on disk and usable, it just
   * isn't tracked for auto-expiry. The sheet shows this as a warning
   * toast rather than treating the whole request as failed.
   */
  warning: string | null;
}
/**
 * `list_github_skills`'s result. `commit` is the tree's own sha, which the
 * copy install pins to; `truncated` is GitHub's own flag for a tree too
 * large to return in one response.
 */
export interface GithubSkillListing {
  repo: string;
  git_ref: string;
  commit: string | null;
  skills: GithubSkillEntry[];
  truncated: boolean;
}
/**
 * One forked skill's provenance, enough to reinstall it from its origin
 * (`unfork_skill`) or to fetch its upstream at a specific commit
 * (`pull_fork_upstream`).
 */
export interface ForkRecord {
  /**
   * Global Universal deployment detached by this fork. Empty only for a
   * legacy record, which callers must resolve by its exact local path.
   */
  deployment_id?: string;
  skill_dir?: string;
  forked_at: string;
  origin_tool: OriginTool;
  /**
   * The exact source string the owning CLI would reinstall from -
   * `agents.lock`'s `source` for dotagents, the lock file's `source` for
   * skills.sh.
   */
  origin_source: string;
  repo: string;
  path: string;
  /**
   * The `ref` dotagents had declared for this skill, if any. `None` for
   * skills.sh forks and unpinned dotagents forks.
   */
  declared_ref: string | null;
  /**
   * The commit the local copy was last synced from - the "base"
   * `pull_fork_upstream` diffs against to tell an edited file from an
   * untouched one, writing conflict markers (never merging) where both
   * sides changed.
   */
  base_commit: string;
}
/**
 * What one `pull_fork_upstream` call did.
 */
export interface PullResult {
  from_commit: string;
  to_commit: string;
  merged: string[];
  conflicts: string[];
  added: string[];
  removed: string[];
  unchanged: number;
  /**
   * Set to "Already up to date" when `to_commit == from_commit`; `None`
   * otherwise.
   */
  message: string | null;
}
export interface FrontmatterRepairPreview {
  deployment_id: string;
  path: string;
  scope: string;
  reason: string;
  expected_content_fingerprint: string;
  proposal_id: string;
  original_content: string;
  proposed_content: string;
  allowed_apply_modes: FrontmatterRepairApplyMode[];
}
/**
 * One skill pack, as sent to the frontend.
 */
export interface PackInfo {
  name: string;
  created_at: string;
  dir: string;
  repo: string | null;
  skills: string[];
}
/**
 * Result of `update_skill_pack`: whether the rebuilt tree actually differed
 * from the pack's last commit.
 */
export interface UpdatePackResult {
  changed: boolean;
  pack: PackInfo;
}
/**
 * Result of `import_skill_pack`: which names came from the repo's own
 * `skills/` tree (`--all`) versus a `[[skills]]` row pointing elsewhere,
 * and any per-row failures (a partial import still reports what worked).
 */
export interface ImportResult {
  bundled: string[];
  referenced: string[];
  errors: string[];
}
/**
 * One row of the event log, projected for the Activity view's History
 * section - see `event_store::EventRow` and `event_commands::list_skill_events`.
 */
export interface SkillEventDto {
  id: string;
  ts: string;
  kind: string;
  skill: string;
  harness: string | null;
  scope: string | null;
  project_path: string | null;
  status: string;
  /**
   * True when this event has an inverse, hasn't already been undone, and
   * its status is one a restore makes sense for.
   */
  restorable: boolean;
  /**
   * False when force restore could cross an independent Copy boundary.
   */
  force_restorable: boolean;
  reverted_by: string | null;
  /**
   * Absolute path to this event's backup directory, for a "Reveal in
   * Finder" action - `None` when the event backed up nothing.
   */
  backup_path?: string | null;
}
/**
 * One skill bundled into a pack: `name` is its directory name, `path` is
 * the exact deployment directory it was bundled from - see
 * `skill_pack::resolve_members`.
 */
export interface PackMember {
  name: string;
  path: string;
}
/**
 * The complete pack import request. Trust confirmation must repeat this
 * value so a token cannot authorize a changed target or source.
 */
export interface PackImportRequest {
  source: string;
  agents: AgentId[];
  method: string;
  destination: SkillDestination;
  scope: InstallScope;
  project_path: string | null;
}
/**
 * One recorded skill use from a harness's own session history.
 */
export interface SkillInvocation {
  /**
   * The skill's name, as recorded by the harness (may carry a
   * `prefix:base` plugin qualifier).
   */
  skill: string;
  /**
   * Which harness recorded this use - an [`AgentId`](crate::identity::AgentId)
   * wire name, e.g. [`AgentId::CLAUDE_CODE`](crate::identity::AgentId::CLAUDE_CODE).
   */
  harness: string;
  /**
   * How the use started.
   */
  trigger: "user" | "agent" | "file_read";
  /**
   * When the use happened.
   */
  at: string;
  /**
   * The project directory the use happened in, if the harness recorded one.
   */
  project_path: string | null;
  /**
   * The harness's own session id, used to dedupe file reads. Not every
   * harness records one.
   */
  session?: string | null;
}
/**
 * Update or remove one deployment, or every deployment of one owner.
 */
export interface LifecycleTarget {
  deployment_id?: string | null;
  owner_id?: string | null;
}
/**
 * Folders added or excluded from project discovery, as recorded under the
 * `projects` key of `~/.agents/skill-studio.json`.
 */
export interface TrackedProjects {
  /**
   * Folders discovery should cover even when it would not find them on
   * its own.
   */
  added: string[];
  /**
   * Folders discovery should skip even when it would otherwise find
   * them.
   */
  excluded: string[];
}
/**
 * One discovery harness's on/off switch, as Settings shows it. `enabled:
 * false` means project discovery no longer reads that harness's own project
 * history (Codex's `config.toml`, Claude Code's transcripts, ...) when
 * looking for folders to add.
 */
export interface DiscoverySourceSetting {
  harness: string;
  enabled: boolean;
}
/**
 * One row of the Settings "Project folders" card.
 */
export interface ProjectFolder {
  /**
   * The path as the caller or the file gave it, same as
   * `SkillSnapshot::projects` - deployments' `project_path` compares
   * against this exact string.
   */
  path: string;
  source: ProjectFolderSource;
  /**
   * True when the path no longer exists on disk - shown as "Folder not
   * found" instead of being dropped, since a folder the user added by
   * hand shouldn't disappear from the list without a trace.
   */
  missing: boolean;
  /**
   * `None` for a plain folder; for a `*`-suffixed pattern, how many of
   * its currently matching folders are in the resolved set. A pattern
   * gets one row of its own instead of one row per matched folder.
   */
  matches: number | null;
}
/**
 * One command's health over the rollup window (see [`crate::health::health_rollup`]):
 * how often it ran, how often it failed, and how long it took.
 */
export interface CommandHealth {
  /**
   * The command name, as recorded in `timing.jsonl`.
   */
  command: string;
  /**
   * Calls within the window.
   */
  count: number;
  /**
   * Calls within the window whose outcome was `"error"`.
   */
  failures: number;
  /**
   * Median elapsed milliseconds, nearest-rank.
   */
  p50_ms: number;
  /**
   * 95th-percentile elapsed milliseconds, nearest-rank.
   */
  p95_ms: number;
  /**
   * The most recent failing call's error text, if any failed.
   */
  last_error: string | null;
}
/**
 * The method and harnesses the last successful install saved, or the
 * environment default when nothing has been saved yet.
 */
export interface InstallPreferences {
  /**
   * The saved or defaulted method.
   */
  method: "copy" | "dotagents" | "skills_sh";
  /**
   * The saved or defaulted harnesses.
   */
  harnesses: AgentIdentity[];
  /**
   * `false` when `method`/`harnesses` are an environment default rather
   * than a saved preference (no install has completed on this scope
   * yet).
   */
  saved: boolean;
}
/**
 * Result of the `harnesses` operation: one detection row per first-class
 * harness.
 */
export interface HarnessReport {
  /**
   * Rows, one per [`builtin_adapters`] harness, in that order.
   */
  harnesses: HarnessDetection[];
}
/**
 * Runtime detection facts for one harness: proven, not guessed.
 */
export interface HarnessDetection {
  /**
   * Kebab-case harness identifier, for example `claude-code` or `open-code`.
   *
   * Invariant: the string is the serde wire name used by the desktop app
   * today. `open-code` is canonical; `opencode` is only a CLI binary name and
   * is never stored in an `AgentId`.
   */
  id: string;
  /**
   * Display name shown to a person.
   */
  display_name: string;
  /**
   * Derived state.
   */
  state: "not_found" | "data_only" | "installed" | "configured" | "used";
  /**
   * Resolved executable path, when found on `PATH`.
   */
  executable: string | null;
  version: DetectedString;
  install_method: DetectedString1;
  /**
   * The vendor config file exists under the home root.
   */
  configured: boolean;
  /**
   * A session or transcript record exists under the home root.
   */
  used: boolean;
}
/**
 * Version, proven by `<bin> --version`.
 */
export interface DetectedString {
  /**
   * The value, when proven.
   */
  value: string | null;
  evidence: Evidence;
}
/**
 * Where the value came from, or why it is `Unknown`.
 */
export interface Evidence {
  /**
   * URL or document reference.
   */
  source: string;
  /**
   * Confidence level.
   */
  confidence: "verified-from-docs" | "inferred" | "unknown";
}
/**
 * Install method, inferred from the resolved executable path.
 */
export interface DetectedString1 {
  /**
   * The value, when proven.
   */
  value: string | null;
  evidence: Evidence;
}
/**
 * The first-run screen's saved choice, round-tripped through the registry.
 * Kept small and documented per unit 3.2's issue: unit 4.4 reads `kept` to
 * decide which harnesses the rest of the app still shows.
 */
export interface HarnessesChoice {
  /**
   * Catalog ids (`AgentId::as_str()`, e.g. `"claude-code"`) of the rows
   * the user kept on the first-run screen.
   */
  kept: string[];
  /**
   * Whether the user opted in to searching harness history (Codex
   * `config.toml` trust rows, Claude Code transcripts, ...) for project
   * folders, mirroring the per-harness discovery switch in
   * `docs/action-map/settings-and-projects.md`.
   */
  search_project_folders: boolean;
  /**
   * RFC 3339 timestamp of the save, for a support report; not read by any
   * decision in the app.
   */
  saved_at: string;
}
/**
 * Result of `fix_skill`.
 */
export interface FixSkillOutcome {
  /**
   * Skill the fix ran for.
   */
  skill: string;
  /**
   * Repairs written.
   */
  applied: FixApplied[];
  /**
   * Issues named but not repaired, with their paths.
   */
  unrepaired: UnrepairedIssue[];
  /**
   * Conflicts found; the app opens both paths in the user's editor.
   */
  conflicts: ConflictSummary[];
}
/**
 * One issue `fix_skill` found but could not repair, named with its path so
 * the caller can show it rather than a generic toast.
 */
export interface UnrepairedIssue {
  /**
   * Path of the offending file or folder, when the issue names one.
   */
  path: string;
  /**
   * Message for a person.
   */
  message: string;
  /**
   * Stable category a caller can branch on instead of `message`.
   */
  kind: "link" | "frontmatter" | "conflict" | "other";
}
/**
 * One pair of differing copies: never merged, named for the caller to open
 * side by side in the user's editor.
 */
export interface ConflictSummary {
  /**
   * Skill the conflict belongs to.
   */
  skill: string;
  /**
   * One-line summary for a person.
   */
  message: string;
  /**
   * First copy's path.
   */
  path_a: string;
  /**
   * Second copy's path.
   */
  path_b: string;
}
/**
 * Result of `remove`.
 */
export interface RemoveOutcome {
  /**
   * The `remove` event.
   */
  event_id: string;
  /**
   * The deployment that was removed.
   */
  deployment_id: string;
  /**
   * The skill that was removed.
   */
  skill: string;
  /**
   * The removed folder's `TreeHash`, taken before the first write.
   */
  tree_hash_before: string;
  /**
   * Where the folder now lives under quarantine, for `Copy`/`Fork`
   * (never deleted - see `docs/action-map/primitives-and-call-stack.md`'s
   * Remove row). `None` for `Dotagents`/`SkillsSh`, whose own CLI deletes
   * the bytes directly, the same as it does for every other build's
   * remove today.
   */
  quarantine_path: string | null;
}
