// ============================================================================
// GENERATED FILE - do not edit by hand.
// Produced by `npm run types:generate` (apps/desktop/scripts/generate-types.mjs)
// from the Rust wire types in apps/desktop/src-tauri/src/skills/*.rs, via the
// `schema` binary and json-schema-to-typescript. Re-run that command after
// changing a #[derive(JsonSchema)] struct or enum; `npm run check` fails if
// this file drifts from the Rust source of truth.
// ============================================================================

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
/**
 * Where a skill is installed relative to harness folders. Universal owns
 * `.agents/skills`; Per harness owns an independent copy in one harness dir
 * and never writes `.agents/skills`.
 */
export type SkillDestination = "universal" | "per-harness";
/**
 * How `add_skill` installed a skill - shared by `AddSkillRequest.method` and
 * `TrialRecord.method`, since a trial's expiry step needs to know which tool
 * (if any) owns the skill it's about to remove.
 */
export type AddMethod = "dotagents" | "skills-sh" | "copy";
/**
 * Scope for skill installation
 */
export type InstallScope = "global" | "project";
export type ParsedSkillSourceKind = "github" | "git" | "local";
/**
 * Which CLI a forked skill was originally managed by.
 */
export type OriginTool = "dotagents" | "skills-sh";
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
      confirmation_token: string;
      identities: string[];
      status: "needs-trust";
    };
/**
 * One of the four first-class agents a skill run can target.
 */
export type HarnessId = "claude-code" | "codex" | "open-code" | "pi";
/**
 * Which OpenCode config format is present, so the frontend can tell the user
 * to hand-edit a `.jsonc` file rather than silently showing no disables.
 */
export type OpencodeConfigKind = "json" | "jsonc";
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
 * Durable state for trial expiry. `Expiring` prevents an interrupted CLI
 * removal from matching a later installation at the same path.
 */
export type TrialStatus = "active" | "expiring" | "recovery-required";

/**
 * What the Add Skill sheet needs before it can pick sensible defaults.
 */
export interface AddMethodDefaults {
  /**
   * Whether `~/.claude/skills` is a symlink into the shared folder -
   * true when Claude Code already reads `.agents/skills` on its own,
   * false when it's a real directory (or doesn't exist yet).
   */
  claude_reads_shared_folder: boolean;
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
}
/**
 * One skill's outcome in an `add_skills` batch. A failure never stops the
 * rest of the batch, so exactly one of `result`/`error` is set per entry.
 */
export interface AddSkillOutcome {
  error: string | null;
  name: string;
  result: AddSkillResult | null;
}
/**
 * `add_skill`'s result.
 */
export interface AddSkillResult {
  command: string;
  deployments_created: string[];
  name: string;
  tool: string;
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
 * `add_skill`'s request - see `AddSkillSheet`.
 */
export interface AddSkillRequest {
  agents: AgentId[];
  destination: SkillDestination;
  /**
   * Harnesses to switch off for this skill right after a successful
   * install: readers of the Universal folder the install itself cannot
   * avoid reaching. Unused for Per harness Copy.
   */
  disabled_harnesses: AgentId[];
  method: AddMethod;
  project_path: string | null;
  scope: InstallScope;
  source: ParsedSkillSource;
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
  localPath: string | null;
  path: string | null;
  ref: string | null;
  repo: string | null;
  skillName: string | null;
  url: string | null;
}
/**
 * `add_skills`' request: one source, and the skill folders picked out of it
 * by the Add-skill sheet's picker (see `github_skill_listing`). Every other
 * field means exactly what it does on `AddSkillRequest`.
 */
export interface AddSkillsRequest {
  agents: AgentId[];
  destination: SkillDestination;
  /**
   * Harnesses to switch off for this skill right after a successful
   * install: readers of the Universal folder the install itself cannot
   * avoid reaching. Unused for Per harness Copy.
   */
  disabled_harnesses: AgentId[];
  method: AddMethod;
  project_path: string | null;
  scope: InstallScope;
  skills: GithubSkillEntry[];
  source: ParsedSkillSource;
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
 * Agent target with paths resolved
 */
export interface AgentTarget {
  global_path: string;
  id: AgentId;
  name: string;
  project_path: string;
}
/**
 * One forked skill's provenance, enough to reinstall it from its origin
 * (`unfork_skill`) or to fetch its upstream at a specific commit
 * (`pull_fork_upstream`).
 */
export interface ForkRecord {
  /**
   * The commit the local copy was last synced from - the "base" of the
   * three-way merge `pull_fork_upstream` runs.
   */
  base_commit: string;
  /**
   * The `ref` dotagents had declared for this skill, if any. `None` for
   * skills.sh forks and unpinned dotagents forks.
   */
  declared_ref: string | null;
  /**
   * Global Universal deployment detached by this fork. Empty only for a
   * legacy record, which callers must resolve by its exact local path.
   */
  deployment_id?: string;
  forked_at: string;
  /**
   * The exact source string the owning CLI would reinstall from -
   * `agents.lock`'s `source` for dotagents, the lock file's `source` for
   * skills.sh.
   */
  origin_source: string;
  origin_tool: OriginTool;
  path: string;
  repo: string;
  skill_dir?: string;
}
export interface FrontmatterRepairPreview {
  allowed_apply_modes: FrontmatterRepairApplyMode[];
  deployment_id: string;
  expected_content_fingerprint: string;
  original_content: string;
  path: string;
  proposal_id: string;
  proposed_content: string;
  reason: string;
  scope: string;
}
/**
 * `list_github_skills`'s result. `commit` is the tree's own sha, which the
 * copy install pins to; `truncated` is GitHub's own flag for a tree too
 * large to return in one response.
 */
export interface GithubSkillListing {
  commit: string | null;
  git_ref: string;
  repo: string;
  skills: GithubSkillEntry[];
  truncated: boolean;
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
 * Result of `import_skill_pack`: which names came from the repo's own
 * `skills/` tree (`--all`) versus a `[[skills]]` row pointing elsewhere,
 * and any per-row failures (a partial import still reports what worked).
 */
export interface ImportResult {
  bundled: string[];
  errors: string[];
  referenced: string[];
}
/**
 * Installation result
 */
export interface InstallResult {
  /**
   * The exact argv `update_skill` ran, joined with spaces, for the same
   * toast.
   */
  command: string | null;
  error: string | null;
  installed_path: string | null;
  skill_name: string;
  success: boolean;
  /**
   * Which CLI `update_skill` ran, for a toast that names it - "dotagents"
   * or "skills-sh". `None` for install/remove results, which never set it.
   */
  tool: string | null;
}
/**
 * Per-day invocation counts for the heatmap (date "YYYY-MM-DD" -> count).
 */
export interface InvocationHeatmap {
  days: {
    [k: string]: number;
  };
}
/**
 * Update or remove one deployment, or every deployment of one owner.
 */
export interface LifecycleTarget {
  deployment_id?: string | null;
  owner_id?: string | null;
}
/**
 * The complete pack import request. Trust confirmation must repeat this
 * value so a token cannot authorize a changed target or source.
 */
export interface PackImportRequest {
  agents: AgentId[];
  destination: SkillDestination;
  method: string;
  project_path: string | null;
  scope: InstallScope;
  source: string;
}
/**
 * One skill pack, as sent to the frontend.
 */
export interface PackInfo {
  created_at: string;
  dir: string;
  name: string;
  repo: string | null;
  skills: string[];
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
 * Paginated response to return to frontend
 */
export interface PaginatedSkillsResponse {
  has_more: boolean;
  skills: SkillSearchResult[];
}
/**
 * Search result from skills.sh API
 */
export interface SkillSearchResult {
  author: string | null;
  description: string | null;
  id: string;
  installs: number;
  name: string;
  tags: string[] | null;
  top_source: string | null;
}
/**
 * What one `pull_fork_upstream` call did.
 */
export interface PullResult {
  added: string[];
  conflicts: string[];
  from_commit: string;
  merged: string[];
  /**
   * Set to "Already up to date" when `to_commit == from_commit`; `None`
   * otherwise.
   */
  message: string | null;
  removed: string[];
  to_commit: string;
  unchanged: number;
}
/**
 * skills.sh v1 skill details, including the skill's markdown body.
 */
export interface SkillDetails {
  hash: string;
  id: string;
  installs: number;
  /**
   * SKILL.md (or AGENTS.md fallback) contents, when the payload has one.
   */
  skill_md: string | null;
  slug: string;
  source: string;
}
/**
 * One row of the event log, projected for the Activity view's History
 * section - see `event_store::EventRow` and `event_commands::list_skill_events`.
 */
export interface SkillEventDto {
  /**
   * Absolute path to this event's backup directory, for a "Reveal in
   * Finder" action - `None` when the event backed up nothing.
   */
  backup_path?: string | null;
  /**
   * False when force restore could cross an independent Copy boundary.
   */
  force_restorable: boolean;
  harness: string | null;
  id: string;
  kind: string;
  project_path: string | null;
  /**
   * True when this event has an inverse, hasn't already been undone, and
   * its status is one a restore makes sense for.
   */
  restorable: boolean;
  reverted_by: string | null;
  scope: string | null;
  skill: string;
  status: string;
  ts: string;
}
/**
 * One recorded skill invocation from an agent transcript.
 */
export interface SkillInvocation {
  /**
   * Which agent recorded this invocation, e.g. "Claude Code".
   */
  agent: string;
  at: string;
  project_path: string | null;
  skill: string;
}
/**
 * Everything the frontend needs about installed skills, discovered
 * projects, and invocation history, built together in one background pass.
 */
export interface SkillSnapshot {
  heatmap: InvocationHeatmap;
  invocations: SkillInvocationStats[];
  /**
   * The newest "Test" run outcome per skill, read cheaply from
   * `skill_run_history::read_last_test_index` - not affected by the
   * invocations-only rebuild path, only refreshed on a full rebuild.
   */
  last_test_by_skill: {
    [k: string]: SkillRunSummary;
  };
  /**
   * Which OpenCode config format is present, if any - `None` when
   * OpenCode isn't configured, `Some(Jsonc)` when Skill Studio can only
   * read (not write) its per-skill disables. See
   * `opencode_skill_permission::detect_config_kind`.
   */
  opencode_config_kind: OpencodeConfigKind | null;
  projects: string[];
  /**
   * Process-local publication order. Zero is reserved for snapshots read
   * from older serialized data that predates revisions.
   */
  revision: number;
  /**
   * Human-readable notes about roots the scan could not reach, each
   * prefixed by a display of the root it is about.
   */
  scan_observations: string[];
  /**
   * True when the core scan's read budget was exceeded before every root
   * could be reached - `skills`/`projects` may be missing entries from
   * the roots named in `scan_observations`. See `core_scan_installed_skills`.
   */
  scan_partial: boolean;
  scanned_at: string;
  skills: InstalledSkill[];
  update_check: UpdateCheckSummary;
}
/**
 * Per-skill invocation summary sent to the frontend.
 */
export interface SkillInvocationStats {
  /**
   * Per-day invocation counts, "YYYY-MM-DD" (UTC), over the last 365 days.
   */
  by_day: {
    [k: string]: number;
  };
  /**
   * Invocation counts by full project path, over the last 30 days only.
   */
  by_project_30_days: {
    [k: string]: number;
  };
  last_14_days: number;
  last_24_hours: number;
  last_30_days: number;
  last_7_days: number;
  last_used: string | null;
  skill: string;
  total: number;
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
 * Installed skill with parsed data
 */
export interface InstalledSkill {
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
   * Every place this skill was found deployed on disk, one entry per
   * agent/scope. Empty when the skill is known only from the lock file.
   */
  deployments: Deployment[];
  /**
   * The `description` field from SKILL.md frontmatter, when present.
   */
  description: string | null;
  /**
   * Token count of just `"{name}: {description}"`, from the first
   * deployment - the prompt cost the model actually pays per turn.
   */
  description_tokens: number;
  /**
   * Number of files in the skill folder, from the first deployment.
   */
  file_count: number;
  /**
   * Total size in bytes of the skill folder, from the first deployment.
   */
  folder_bytes: number;
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
   * Every top-level SKILL.md frontmatter key, stringified, from the first
   * deployment.
   */
  frontmatter_fields: {
    [k: string]: string;
  };
  /**
   * True when the skill directory ships behavior specs/evals
   * (a spec.md file or an evals/ directory), the getsentry/skillet pattern.
   */
  has_spec: boolean;
  has_update: boolean;
  installed_at: string;
  /**
   * Which invocation channels this skill allows - see
   * `frontmatter::invocation_policy`.
   */
  invocation: "both" | "user-only" | "model-only";
  /**
   * RFC3339 timestamp of the newest file mtime in the skill folder, from
   * the first deployment.
   */
  modified_at?: string | null;
  name: string;
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
   * Token count of SKILL.md's text (cl100k_base), from the first deployment.
   */
  skill_md_tokens: number;
  skill_path: string | null;
  source: string;
  /**
   * How this skill was installed - see `skill_studio_core::identity::SourceKind`.
   */
  source_kind: "dotagents" | "plugin" | "skills-sh" | "in-repo" | "manual" | "fork";
  source_type: string;
  source_url: string | null;
  /**
   * Violations of the agentskills.io SKILL.md spec found for this skill.
   * Empty means the skill is spec-compliant.
   */
  spec_violations: string[];
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
   * Exact lifecycle owners whose persisted update state is newer than the
   * installed commit. Aggregate update badges derive from this list.
   */
  update_owner_ids: string[];
  /**
   * Update metadata keyed by the exact lifecycle owner. New clients use
   * this instead of pairing an action with aggregate commit metadata.
   */
  update_owners: OwnerUpdateInfo[];
  updated_at: string | null;
}
/**
 * Where a skill is deployed on disk for a specific agent
 */
export interface Deployment {
  /**
   * Display name of the agent (e.g. "Claude Code"). Universal roots
   * still use the compatibility label `shared`.
   */
  agent: string;
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
   * Codex's own `agents/openai.yaml` `policy.allow_implicit_invocation`
   * value, read straight off disk - note-only, doesn't affect
   * `InstalledSkill.invocation` (that's driven by SKILL.md frontmatter).
   */
  codex_implicit_invocation: boolean | null;
  /**
   * This deployment's own sha256 content hash, empty when unreadable
   * (e.g. a broken symlink). Lets the UI point at which specific copies
   * of a duplicated skill differ, not just the skill as a whole.
   */
  content_hash: string;
  /**
   * Where a skill is installed relative to harness folders. Universal owns
   * `.agents/skills`; Per harness owns an independent copy in one harness dir
   * and never writes `.agents/skills`.
   */
  destination: "universal" | "per-harness";
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
   * (Codex config / OpenCode permission deny) - `"codex"`, `"open-code"`.
   * Always empty for other deployments.
   */
  disabled_readers?: string[];
  /**
   * Stable id (`dep:v1/...`) for exact mutations. Empty only on
   * lock-file-only records that have no on-disk path.
   */
  id: string;
  /**
   * Which invocation channels this deployment's own SKILL.md allows - see
   * `frontmatter::invocation_policy`. Defaults to `Both`, same as
   * `InstalledSkill.invocation`, for deployments serialized before this
   * field existed (fixtures, cached snapshots).
   */
  invocation: "both" | "user-only" | "model-only";
  is_symlink: boolean;
  /**
   * Whether Skill Studio may mutate this deployment through an owner adapter.
   */
  mutability: "mutable" | "read-only";
  /**
   * `owner:v1/...` when a matching ledger owns this deployment.
   */
  owner_id?: string | null;
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
  path: string;
  /**
   * Set when this deployment is a skill shipped by a plugin.
   */
  plugin?: PluginInfo | null;
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
  scope: string;
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
   * Set when `is_symlink` and resolving the target failed for a reason
   * other than "doesn't exist" (permission denied, symlink loop, etc.).
   */
  symlink_error?: string | null;
  /**
   * True when `is_symlink` but the target doesn't exist.
   */
  symlink_is_broken: boolean;
  /**
   * Canonicalized symlink target, when `is_symlink` and the target resolves.
   */
  symlink_target?: string | null;
}
/**
 * A plugin that shipped a skill, per the agent-plugins.org convention
 * (Claude Code / Codex plugin caches, or any directory with a `plugin.json`
 * manifest and a `skills/` subdirectory).
 */
export interface PluginInfo {
  /**
   * Which agent's plugin system this came from, e.g. "Claude Code", "Codex".
   */
  harness: string;
  /**
   * `"<plugin>@<marketplace>"`, the id the harness's plugin CLI expects.
   */
  id: string;
  /**
   * Marketplace directory name.
   */
  marketplace: string;
  name: string;
  version: string | null;
}
/**
 * Fork provenance shown on a forked skill's detail header - see
 * `skill_fork_registry::ForkRecord`, which this is a read-only projection
 * of for the frontend.
 */
export interface ForkInfo {
  base_commit: string;
  forked_at: string;
  origin_source: string;
  origin_tool: OriginTool;
  repo: string;
}
/**
 * A trial's remaining-time projection, read-only for the frontend - see
 * `skill_fork_registry::TrialRecord`, which this is a projection of.
 */
export interface TrialInfo {
  deployment_id: string;
  expires_at: string;
  method: AddMethod;
  project_path: string | null;
  /**
   * The trial's scope - needed so `keep_skill_trial`/expiry can key back
   * into `trials` (`"global/<name>"` or `"project/<name>"`) correctly.
   */
  scope: "global" | "project";
  status: TrialStatus;
}
/**
 * Persisted update state for one exact lifecycle owner.
 */
export interface OwnerUpdateInfo {
  latest_commit: string | null;
  latest_commit_at: string | null;
  owner_id: string;
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
 * Result of `update_skill_pack`: whether the rebuilt tree actually differed
 * from the pack's last commit.
 */
export interface UpdatePackResult {
  changed: boolean;
  pack: PackInfo;
}
