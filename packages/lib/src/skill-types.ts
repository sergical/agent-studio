// ============================================================================
// Skill Studio - skill-types
// Re-exports the generated wire types (skill-types.generated.ts, produced
// from the Rust `#[derive(JsonSchema)]` DTOs by `npm run types:generate` -
// see that script and `docs/spec-core-primitives.md` section 10's PR 3 row)
// plus the UI-only types and helpers that have no Rust counterpart.
//
// `ParsedSkillSource` and `SkillRunSummary` are deliberately NOT re-exported
// here even though the generator also produces them: `skill-source-parse.ts`
// and `skill-run-history-types.ts` are those two types' canonical homes
// (the frontend builds the former; both are already exported from
// `index.ts`), and re-exporting a second copy of the same name from this
// file would make every import of it ambiguous.
// ============================================================================

export type {
  AgentId,
  SkillDestination,
  AddMethod,
  InstallScope,
  ParsedSkillSourceKind,
  OriginTool,
  FrontmatterRepairApplyMode,
  PackImportPreflightResult,
  HarnessId,
  OpencodeConfigKind,
  DisabledBy,
  AddMethodDefaults,
  AddSkillOutcome,
  AddSkillResult,
  AddSkillRequest,
  AddSkillsRequest,
  GithubSkillEntry,
  AgentTarget,
  ForkRecord,
  FixSkillOutcome,
  FixApplied,
  ConflictSummary,
  UnrepairedIssue,
  FrontmatterRepairPreview,
  GithubSkillListing,
  HarnessReport,
  HarnessDetection,
  HarnessesChoice,
  HarnessVisibilityTarget,
  ImportResult,
  InstallResult,
  InstallPreferences,
  InvocationHeatmap,
  PackImportRequest,
  PackInfo,
  PackMember,
  PaginatedSkillsResponse,
  SkillSearchResult,
  PullResult,
  RemoveOutcome,
  SkillDetails,
  SkillEventDto,
  SkillEventDto as SkillEvent,
  SkillInvocation,
  SkillSnapshot,
  SkillInvocationStats,
  SkillTrigger,
  SkillUseHour,
  InstalledSkill,
  Deployment,
  PluginInfo,
  ForkInfo,
  OwnerUpdateInfo,
  UpdateCheckSummary,
  SkillsShAccessInfo,
  UpdatePackResult,
  LifecycleTarget,
  TrackedProjects,
  DiscoverySourceSetting,
  ProjectFolder,
  ProjectFolderSource,
  CommandHealth,
  UpdateOutcome,
  UpdateAllOutcome,
  UpdateAllItem,
  DoctorReport,
  DoctorViolation,
} from "./skill-types.generated";

import type { Deployment, InstalledSkill, SkillSearchResult } from "./skill-types.generated";

// ============================================================================
// Field-derived aliases
// `#[serde(default = ...)]` on these Deployment/InstalledSkill fields makes
// schemars emit `$ref` + a sibling `default`, which json-schema-to-typescript
// dereferences and inlines rather than keeping the named Rust type - see
// generate-types.mjs's header. Deriving these with an indexed-access type
// keeps one source of truth (the generated interface) instead of retyping
// the literal unions by hand, so they can't drift from it.
// ============================================================================

/** Tool or source that owns lifecycle changes for a deployment. */
export type LifecycleOwnerKind = NonNullable<Deployment["owner_kind"]>;

/** How a deployment relates to a Universal folder of the same skill. */
export type BackingRelationship = NonNullable<Deployment["backing"]>;

/** Which invocation channels a skill or deployment allows. */
export type InvocationPolicy = NonNullable<Deployment["invocation"]>;

/**
 * How a skill made it onto disk - see `InstalledSkill.source_kind` and the
 * Rust `provenance::SourceKind`.
 */
export type SkillSourceKind = InstalledSkill["source_kind"];

// ============================================================================
// UI-only types
// No Rust counterpart - purely frontend state.
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
  /** A secondary button - see `ToastContainer`. */
  action?: { label: string; onClick: () => void };
}

/**
 * First-class agents featured for quick selection in the agent-target
 * picker. These are the install targets `npx skills --agent <id>` accepts,
 * not every harness the scanner reads: Grok Build is scanned for coverage
 * and health but is not an `npx skills` install target, so it is excluded
 * here (see AgentId::GrokBuild rejection in skills/commands.rs).
 */
export const COMMON_AGENTS: import("./skill-types.generated").AgentId[] = [
  "claude-code",
  "codex",
  "open-code",
  "pi",
  "cursor",
];

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

/**
 * The running app's own version, build commit, and release notes for
 * Settings' "Version" row and "What's new" panel - see the Rust
 * `app_version` command.
 */
export interface AppVersion {
  version: string;
  commit: string;
  notes: string | null;
}

/**
 * Settings' "Version" card update state (unit 6.2) - see the Rust
 * `skill_update::UpdateStatus`. `"error"` names the failure but is not
 * itself an install failure: no update ever installs without
 * `installUpdate`, which the card's "Restart to update" button is the only
 * caller of.
 */
export type UpdateStatus =
  | { status: "up-to-date" }
  | { status: "checking" }
  | { status: "downloading"; version: string }
  | { status: "ready-to-install"; version: string }
  | { status: "error"; message: string };
