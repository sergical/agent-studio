// ============================================================================
// Skill Studio - skill-health
// Pure functions over InstalledSkill[] that flag things worth the user's
// attention. No Tauri/DOM access here so these stay unit-testable in
// isolation (Vitest, once a runner is wired up).
// ============================================================================

import type { Deployment, InstalledSkill } from "./skill-types";

/**
 * Kind of health issue a `HealthIssue` reports. `update-available` is not an
 * issue - see `skill-updates.ts`'s `skillsWithUpdates` - and neither is
 * `never-invoked` (noise, not a problem) or `missing-from-agents` (kept as
 * `coverageGaps` for the coverage column, not surfaced as something broken).
 */
export type HealthIssueKind =
  | "duplicate"
  | "broken-symlink"
  | "linked-root"
  | "parked-but-reinstalled"
  | "spec-violation"
  | "lock-only";

/** One flagged condition for one skill, with a short human-readable reason. */
export interface HealthIssue {
  kind: HealthIssueKind;
  skill: InstalledSkill;
  detail: string;
  deploymentPath?: string;
  /**
   * For `"linked-root"` only: the harness's agent id (e.g. `"claude-code"`,
   * what the backend commands key on, never the display label) and the
   * shared-folder root path it reads through. A `"linked-root"` issue isn't
   * really about one skill, it's about a harness/root pair, so `skill` above
   * is just a representative one for the row's harness marks and "Open"
   * action.
   */
  harness?: string;
  harnessLabel?: string;
  root?: string;
}

/**
 * Stable display order for issue kinds, shared by the dashboard's grouped
 * summary and the Skills list's issue filter.
 */
export const HEALTH_ISSUE_KIND_ORDER: HealthIssueKind[] = [
  "parked-but-reinstalled",
  // A root link outranks the per-skill issues below it: it is one structural
  // fault that blocks per-skill control for a whole harness, and Home only
  // previews the first few rows before collapsing the rest.
  "linked-root",
  "duplicate",
  "broken-symlink",
  "spec-violation",
  "lock-only",
];

/**
 * Severity dot color for one issue kind, shared by the dashboard's grouped
 * summary. Everything that means "this skill is broken or inconsistent" is
 * an error; `lock-only` (known only from the lock file, nothing to load) is
 * a warning.
 */
export const HEALTH_ISSUE_SEVERITY = {
  "parked-but-reinstalled": "error",
  duplicate: "warning",
  "broken-symlink": "error",
  "linked-root": "warning",
  "spec-violation": "error",
  "lock-only": "warning",
} as const satisfies Record<HealthIssueKind, "error" | "warning">;

/** Singular/plural copy for one issue kind, for chip and row labels. */
export const HEALTH_ISSUE_KIND_LABEL = {
  "parked-but-reinstalled": {
    singular: "parked skill was reinstalled",
    plural: "parked skills were reinstalled",
  },
  duplicate: { singular: "skill differs between copies", plural: "skills differ between copies" },
  "broken-symlink": { singular: "broken link", plural: "broken links" },
  "linked-root": {
    singular: "harness reads the Universal folder through a root link",
    plural: "harnesses read the Universal folder through a root link",
  },
  "spec-violation": {
    singular: "skill that fails to load",
    plural: "skills that fail to load",
  },
  "lock-only": { singular: "skill only in the lock file", plural: "skills only in the lock file" },
} as const satisfies Record<HealthIssueKind, { singular: string; plural: string }>;

/** The first-class agents `coverageGaps` expects full coverage across; also the harness chip list in `SkillListFilterBar`. */
export const FIRST_CLASS_AGENTS = [
  "Claude Code",
  "Codex",
  "OpenCode",
  "pi",
  "Cursor",
  "Grok Build",
] as const;

/**
 * Agents that natively discover the shared `.agents/skills` root without a
 * symlink (Claude Code does not), so a "shared" deployment counts as
 * coverage for each of them. See docs/agent-skill-conventions.md.
 */
const SHARED_ROOT_READERS = ["Codex", "OpenCode", "pi", "Cursor", "Grok Build"] as const;

/** Which first-class agents one deployment gives coverage for. */
export function agentsCoveredByDeployment(agent: string): readonly string[] {
  if (agent === "shared") return SHARED_ROOT_READERS;
  return FIRST_CLASS_AGENTS.some((first) => first === agent) ? [agent] : [];
}

/**
 * "Global" or the project directory basename, plus the deployment's agent
 * (already a display label, e.g. "Claude Code" or "shared"), so two copies
 * at the same scope but different agents get distinct labels, e.g.
 * "Global · Claude Code", "Global · Universal folder", "webvitals.com · Universal folder".
 */
export function deploymentLabel(deployment: Deployment): string {
  const scope =
    deployment.scope === "project" && deployment.project_path
      ? (deployment.project_path.split("/").filter(Boolean).pop() ?? "Global")
      : "Global";
  const agent = deployment.agent === "shared" ? "Universal folder" : deployment.agent;
  return `${scope} · ${agent}`;
}

/**
 * Prefixes (from `frontmatter::validate_skill`, Rust side) of a
 * `spec_violations` entry that stops the skill from loading at all: a
 * malformed YAML, a missing or invalid `name`, a missing `description`, or a name/directory
 * mismatch. Every other violation (description/compatibility length, the
 * 500-line recommendation, conflicting invocation keys) is a spec note the
 * skill still loads with, shown on the skill page rather than as an issue.
 */
const BLOCKING_SPEC_VIOLATION_PREFIXES = [
  "invalid YAML frontmatter at line ",
  "missing required frontmatter field: name",
  "missing required frontmatter field: description",
  'name "', // covers both the invalid-name-format and name/dir-mismatch messages
] as const;

/** True when `violation` is one of `BLOCKING_SPEC_VIOLATION_PREFIXES` - see there for why. */
export function isBlockingSpecViolation(violation: string): boolean {
  return BLOCKING_SPEC_VIOLATION_PREFIXES.some((prefix) => violation.startsWith(prefix));
}

/**
 * One skill's coverage gap at one scope: deployed to some, but not all, of
 * the four first-class agents. Not a `HealthIssue` - a gap here isn't
 * something broken, just a column the coverage view highlights.
 */
export interface CoverageGap {
  skill: InstalledSkill;
  /** "Global", or the project path, whichever scope the gap is at. */
  scopeLabel: string;
  missing: string[];
}

/**
 * Skills deployed to some, but not all, of the four first-class agents at
 * the same scope (global, or a given project). See `CoverageGap`.
 */
export function coverageGaps(skills: InstalledSkill[]): CoverageGap[] {
  const gaps: CoverageGap[] = [];

  for (const skill of skills) {
    if (skill.parked) continue;
    const groups = new Map<string, Set<string>>();
    for (const deployment of skill.deployments) {
      // A harness the user explicitly disabled isn't "missing" coverage.
      if (deployment.disabled) continue;
      const covered = agentsCoveredByDeployment(deployment.agent);
      if (covered.length === 0) {
        continue;
      }
      const groupKey =
        deployment.scope === "project" ? `project:${deployment.project_path}` : "global";
      const agents = groups.get(groupKey) ?? new Set<string>();
      for (const agent of covered) agents.add(agent);
      groups.set(groupKey, agents);
    }

    for (const [groupKey, agents] of groups) {
      if (agents.size > 0 && agents.size < FIRST_CLASS_AGENTS.length) {
        gaps.push({
          skill,
          scopeLabel: groupKey === "global" ? "Global" : groupKey.slice("project:".length),
          missing: FIRST_CLASS_AGENTS.filter((a) => !agents.has(a)),
        });
      }
    }
  }

  return gaps;
}

/** `issues` bucketed by kind, in `HEALTH_ISSUE_KIND_ORDER`, omitting zero counts. */
export function groupIssuesByKind(
  issues: HealthIssue[],
): { kind: HealthIssueKind; count: number }[] {
  const counts = new Map<HealthIssueKind, number>();
  for (const issue of issues) {
    counts.set(issue.kind, (counts.get(issue.kind) ?? 0) + 1);
  }

  return HEALTH_ISSUE_KIND_ORDER.filter((kind) => (counts.get(kind) ?? 0) > 0).map((kind) => ({
    kind,
    count: counts.get(kind) ?? 0,
  }));
}
