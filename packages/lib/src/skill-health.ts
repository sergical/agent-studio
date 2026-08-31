// ============================================================================
// Skill Studio - skill-health
// Pure functions over InstalledSkill[] that flag things worth the user's
// attention. No Tauri/DOM access here so these stay unit-testable in
// isolation (Vitest, once a runner is wired up).
// ============================================================================

import { ownDeployments } from "./skill-plugin-partition";
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
  | "parked-but-reinstalled"
  | "spec-violation"
  | "lock-only";

/** One flagged condition for one skill, with a short human-readable reason. */
export interface HealthIssue {
  kind: HealthIssueKind;
  skill: InstalledSkill;
  detail: string;
}

/**
 * Stable display order for issue kinds, shared by the dashboard's grouped
 * summary and the Skills list's issue filter.
 */
export const HEALTH_ISSUE_KIND_ORDER: HealthIssueKind[] = [
  "parked-but-reinstalled",
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
function agentsCoveredByDeployment(agent: string): readonly string[] {
  if (agent === "shared") return SHARED_ROOT_READERS;
  return FIRST_CLASS_AGENTS.some((first) => first === agent) ? [agent] : [];
}

/**
 * "Global" or the project directory basename, plus the deployment's agent
 * (already a display label, e.g. "Claude Code" or "shared"), so two copies
 * at the same scope but different agents get distinct labels, e.g.
 * "Global · Claude Code", "Global · shared", "webvitals.com · shared".
 */
export function deploymentLabel(deployment: Deployment): string {
  const scope =
    deployment.scope === "project" && deployment.project_path
      ? (deployment.project_path.split("/").filter(Boolean).pop() ?? "Global")
      : "Global";
  return `${scope} · ${deployment.agent}`;
}

/**
 * Skills whose non-plugin deployments disagree on content: the same skill
 * name has more than one distinct `content_hash` across its own copies (e.g.
 * a stale copy left behind by a manual edit). Built from `ownDeployments` so
 * a plugin-managed copy - which the user doesn't edit directly - never
 * creates a false duplicate. `detail` names the copy with a strict majority
 * as the reference and lists the copies that differ from it, e.g. "sentry ·
 * Cursor differs from Global · shared" - the verb agrees with the number of
 * differing copies; with no strict majority, every copy
 * is listed instead, e.g. "2 copies differ: Global · Claude Code;
 * webvitals.com · shared".
 */
export function findDuplicateSkills(skills: InstalledSkill[]): HealthIssue[] {
  const issues: HealthIssue[] = [];

  for (const skill of skills) {
    const withHash = ownDeployments(skill).filter((d) => d.content_hash);
    const distinctHashes = new Set(withHash.map((d) => d.content_hash));
    if (distinctHashes.size <= 1) continue;

    const counts = new Map<string, number>();
    for (const d of withHash) {
      counts.set(d.content_hash, (counts.get(d.content_hash) ?? 0) + 1);
    }
    const majorityHash = [...counts.entries()].find(
      ([, count]) => count * 2 > withHash.length,
    )?.[0];

    let detail: string;
    if (majorityHash !== undefined) {
      const majorityDeployment = withHash.find((d) => d.content_hash === majorityHash);
      const majorityLabel = majorityDeployment ? deploymentLabel(majorityDeployment) : "Global";
      const differingLabels = withHash
        .filter((d) => d.content_hash !== majorityHash)
        .map(deploymentLabel);
      const verb = differingLabels.length === 1 ? "differs" : "differ";
      detail = `${differingLabels.join("; ")} ${verb} from ${majorityLabel}`;
    } else {
      detail = `${withHash.length} copies differ: ${withHash.map(deploymentLabel).join("; ")}`;
    }

    issues.push({ kind: "duplicate", skill, detail });
  }

  return issues;
}

/**
 * Skills with at least one deployment whose symlink target doesn't resolve.
 */
export function findBrokenSymlinks(skills: InstalledSkill[]): HealthIssue[] {
  const issues: HealthIssue[] = [];
  for (const skill of skills) {
    const broken = skill.deployments.filter((d) => d.symlink_is_broken);
    for (const deployment of broken) {
      issues.push({
        kind: "broken-symlink",
        skill,
        detail: deployment.symlink_target
          ? `${deployment.agent} links to ${deployment.symlink_target}, which is missing`
          : `${deployment.agent} · broken link at ${deployment.path}`,
      });
    }
  }
  return issues;
}

/**
 * Prefixes (from `frontmatter::validate_skill`, Rust side) of a
 * `spec_violations` entry that stops the skill from loading at all: a
 * missing or invalid `name`, a missing `description`, or a name/directory
 * mismatch. Every other violation (description/compatibility length, the
 * 500-line recommendation, conflicting invocation keys) is a spec note the
 * skill still loads with, shown on the skill page rather than as an issue.
 */
const BLOCKING_SPEC_VIOLATION_PREFIXES = [
  "missing required frontmatter field: name",
  "missing required frontmatter field: description",
  'name "', // covers both the invalid-name-format and name/dir-mismatch messages
] as const;

/** True when `violation` is one of `BLOCKING_SPEC_VIOLATION_PREFIXES` - see there for why. */
export function isBlockingSpecViolation(violation: string): boolean {
  return BLOCKING_SPEC_VIOLATION_PREFIXES.some((prefix) => violation.startsWith(prefix));
}

/**
 * Skills whose SKILL.md violates an agentskills.io spec rule that stops it
 * from loading - see `isBlockingSpecViolation`. A skill with only
 * non-blocking violations (e.g. "description exceeds 1024 characters")
 * isn't flagged here; those stay as spec notes on the skill page.
 */
export function findSpecViolations(skills: InstalledSkill[]): HealthIssue[] {
  return skills
    .map((skill) => ({ skill, blocking: skill.spec_violations.filter(isBlockingSpecViolation) }))
    .filter(({ blocking }) => blocking.length > 0)
    .map(({ skill, blocking }) => ({
      kind: "spec-violation" as const,
      skill,
      detail: blocking.join("; "),
    }));
}

/**
 * Skills known only from the lock file, with no deployment found on disk.
 */
export function findLockOnlySkills(skills: InstalledSkill[]): HealthIssue[] {
  return skills
    .filter((skill) => skill.deployments.length === 0)
    .map((skill) => ({
      kind: "lock-only" as const,
      skill,
      detail: "In the lock file but not deployed anywhere",
    }));
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

/**
 * Parked skills whose shared-folder deployment came back - see
 * `skill_park.rs`'s "parked-but-reinstalled" note: an install or sync run
 * while the skill was parked can recreate `~/.agents/skills/<name>` even
 * though the parked copy is still sitting in `~/.agents/skills-parked`.
 * Unparking reconciles the two; this issue just flags that it's needed.
 */
export function findParkedButReinstalled(skills: InstalledSkill[]): HealthIssue[] {
  return skills
    .filter((skill) => skill.parked && skill.deployments.some((d) => d.scope !== "parked"))
    .map((skill) => ({
      kind: "parked-but-reinstalled" as const,
      skill,
      detail: "Parked, but an install or sync recreated the shared-folder copy",
    }));
}

/**
 * Every dashboard-worthy issue across `skills`: parked-but-reinstalled,
 * duplicate, broken-symlink, spec-violation, and lock-only. Excludes
 * update-available (see `skill-updates.ts`) and coverage gaps (see
 * `coverageGaps` above) - neither is a problem, just something to act on or
 * a coverage-view column. Sorted by `HEALTH_ISSUE_KIND_ORDER` then skill
 * name, so both the dashboard and the Skills view show a stable order.
 */
export function collectDashboardIssues(skills: InstalledSkill[]): HealthIssue[] {
  const issues = [
    ...findParkedButReinstalled(skills),
    ...findDuplicateSkills(skills),
    ...findBrokenSymlinks(skills),
    ...findSpecViolations(skills),
    ...findLockOnlySkills(skills),
  ];

  return issues.sort((a, b) => {
    const orderDiff =
      HEALTH_ISSUE_KIND_ORDER.indexOf(a.kind) - HEALTH_ISSUE_KIND_ORDER.indexOf(b.kind);
    return orderDiff !== 0 ? orderDiff : a.skill.name.localeCompare(b.skill.name);
  });
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
