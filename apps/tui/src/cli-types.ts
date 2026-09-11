// ============================================================================
// Skill Studio TUI - CLI wire types
// Hand mirror of the `ResultEnvelope` and DTOs in
// `crates/skill-studio-core/src/{dto,error,ops,identity,scope,harness}.rs`.
// Field names and enum tags match the `serde` output exactly (see
// `docs/spec-core-primitives.md` 8.1 and `apps/cli/README.md`); only the
// fields the TUI renders are included.
// ============================================================================

/** `Operation` (`ops.rs`): one CLI subcommand name, as printed in the envelope. */
export type Operation =
  | "scan"
  | "diagnose"
  | "capabilities"
  | "preview_frontmatter_repair"
  | "apply_frontmatter_repair"
  | "list_events"
  | "restore_event";

/** `OpStatus` (`ops.rs`). */
export type OpStatus = "ok" | "partial" | "error";

/** `ErrorCode` (`error.rs`), stable wire strings. */
export type ErrorCode =
  | "invalid_request"
  | "invalid_scope"
  | "ambiguous_target"
  | "unsupported"
  | "execution_failed"
  | "io"
  | "scope_busy"
  | "stale_proposal"
  | "drift_conflict"
  | "ownership_changed"
  | "already_reverted"
  | "incomplete"
  | "cancelled";

/** `ErrorEntry` (`error.rs`). */
export interface ErrorEntry {
  code: ErrorCode;
  message: string;
  path: string | null;
}

/** `ScopeKind` (`scope.rs`). */
export type ScopeKind = "live" | "fixture";

/** `EffectiveScope` (`scope.rs`). */
export interface EffectiveScope {
  id: string;
  kind: ScopeKind;
  home: string;
  projects: string[];
  history_root: string;
}

/** `ResultEnvelope<T>` (`ops.rs`). */
export interface Envelope<T> {
  schema_version: number;
  operation: Operation;
  scope: EffectiveScope;
  status: OpStatus;
  data: T | null;
  errors: ErrorEntry[];
  correlation_id: string;
  event_id: string | null;
}

/** `RootScope` (`identity.rs`): adjacently tagged on `scope`/`project`. */
export type RootScope = { scope: "global" } | { scope: "project"; project: string };

/** `RootKind` (`identity.rs`): adjacently tagged on `kind`/`harness`. */
export type RootKind =
  | { kind: "harness"; harness: string }
  | { kind: "universal" }
  | { kind: "parked" }
  | { kind: "legacy"; harness: string }
  | { kind: "plugin_cache"; harness: string };

/** `RootRef` (`identity.rs`). */
export interface RootRef {
  scope: RootScope;
  kind: RootKind;
}

/** `BackingRelationship` (`identity.rs`). */
export type BackingRelationship = "canonical" | "linked_to" | "independent";

/** `DeploymentMutability` (`identity.rs`). */
export type DeploymentMutability = "mutable" | "read_only";

/** `SkillDestination` (`identity.rs`). */
export type SkillDestination = "universal" | "harness";

/** `LifecycleOwnerKind` (`identity.rs`). */
export type LifecycleOwnerKind = "skills_sh" | "dotagents" | "in_repo" | "manual" | "fork" | "none";

/** `DisabledBy` (`harness.rs`). */
export type DisabledBy =
  | "codex_config"
  | "opencode_permission"
  | "move_aside"
  | "claude_skill_overrides"
  | "pi_settings"
  | "unknown";

/** `PluginSourceDto` (`dto.rs`). */
export interface PluginSourceDto {
  marketplace: string;
  plugin: string;
  version: string | null;
}

/** `DeploymentDto` (`dto.rs`). */
export interface DeploymentDto {
  id: string;
  root: RootRef;
  harness: string | null;
  path: string;
  destination: SkillDestination;
  backing: BackingRelationship;
  mutability: DeploymentMutability;
  link_target: string | null;
  shared_via_whole_dir_link: boolean;
  owner_kind: LifecycleOwnerKind;
  owner_id: string | null;
  content_fingerprint: string | null;
  disabled_by: DisabledBy | null;
  disabled_readers: string[];
  spec_violations: string[];
  plugin: PluginSourceDto | null;
}

/** `InstalledSkillDto` (`dto.rs`). */
export interface InstalledSkillDto {
  name: string;
  description: string | null;
  deployments: DeploymentDto[];
}

/** `Completeness` (`dto.rs`). */
export type Completeness = "complete" | "partial";

/** `Observation` (`dto.rs`). */
export interface Observation {
  root: RootRef | null;
  message: string;
}

/** `Timing` (`dto.rs`). */
export interface Timing {
  phase: string;
  elapsed_ms: number;
}

/** `Inventory` (`dto.rs`): the result of `scan`. */
export interface Inventory {
  skills: InstalledSkillDto[];
  projects: string[];
  completeness: Completeness;
  observations: Observation[];
  timings: Timing[];
}

/** `Severity` (`dto.rs`). */
export type Severity = "off" | "warning" | "error";

/** `IssueKind` (`dto.rs`). */
export type IssueKind =
  | "broken_link"
  | "unreadable_link"
  | "spec_violation"
  | "repairable_frontmatter"
  | "drift"
  | "duplicate"
  | "parked"
  | "disabled"
  | "root_unreadable";

/** `NextAction` (`dto.rs`): internally tagged on `action`. */
export type NextAction =
  | { action: "preview_repair"; deployment_id: string }
  | { action: "repair_link"; deployment_id: string }
  | { action: "restore"; deployment_id: string }
  | { action: "rescan" }
  | { action: "none" };

/** `Issue` (`dto.rs`). */
export interface Issue {
  kind: IssueKind;
  severity: Severity;
  skill: string;
  deployment_id: string | null;
  message: string;
  next_action: NextAction;
}

/** `Diagnosis` (`dto.rs`): the result of `diagnose`. */
export interface Diagnosis {
  inventory: Inventory;
  issues: Issue[];
}

/** One line of `skill-studio watch --json`: `WatchLine` in `apps/cli/src/main.rs`.
 *
 * Every line carries both fields; there is no revision-only line (delta to
 * `docs/spec-core-primitives.md` 8.3 confirmed against `apps/cli/README.md`
 * and `apps/cli/src/main.rs::run_watch`). */
export interface WatchLine {
  revision: number;
  inventory: Inventory;
}
