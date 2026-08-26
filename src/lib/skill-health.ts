// ============================================================================
// Skill Studio - skill-health
// Pure functions over InstalledSkill[] that flag things worth the user's
// attention. No Tauri/DOM access here so these stay unit-testable in
// isolation (Vitest, once a runner is wired up).
// ============================================================================

import { ownDeployments } from "./skill-plugin-partition";
import type { Deployment, InstalledSkill, SkillInvocationStats } from "./skill-types";

/** Kind of health issue a `HealthIssue` reports. */
export type HealthIssueKind =
  | "duplicate"
  | "broken-symlink"
  | "spec-violation"
  | "lock-only"
  | "never-invoked"
  | "missing-from-agents";

/** One flagged condition for one skill, with a short human-readable reason. */
export interface HealthIssue {
  kind: HealthIssueKind;
  skill: InstalledSkill;
  detail: string;
}

/**
 * Stable display order for issue kinds, shared by the dashboard's grouped
 * summary, the sidebar count, and the Issues view's filter chips.
 */
export const HEALTH_ISSUE_KIND_ORDER: HealthIssueKind[] = [
  "duplicate",
  "broken-symlink",
  "spec-violation",
  "missing-from-agents",
  "lock-only",
  "never-invoked",
];

/**
 * Severity dot color for one issue kind, shared by the dashboard's grouped
 * summary and the Issues view's table. broken-symlink and lock-only are
 * errors (something is missing); everything else is a warning.
 */
export const HEALTH_ISSUE_SEVERITY = {
  duplicate: "warning",
  "broken-symlink": "error",
  "spec-violation": "warning",
  "missing-from-agents": "warning",
  "lock-only": "error",
  "never-invoked": "warning",
} as const satisfies Record<HealthIssueKind, "error" | "warning">;

/** Singular/plural copy for one issue kind, for chip and row labels. */
export const HEALTH_ISSUE_KIND_LABEL = {
  duplicate: { singular: "skill differs between copies", plural: "skills differ between copies" },
  "broken-symlink": { singular: "broken link", plural: "broken links" },
  "spec-violation": { singular: "skill with spec issues", plural: "skills with spec issues" },
  "missing-from-agents": {
    singular: "skill missing from an agent",
    plural: "skills missing from an agent",
  },
  "lock-only": { singular: "skill only in the lock file", plural: "skills only in the lock file" },
  "never-invoked": { singular: "skill never used", plural: "skills never used" },
} as const satisfies Record<HealthIssueKind, { singular: string; plural: string }>;

/** The first-class agents `findMissingFromAgents` expects full coverage across. */
const FIRST_CLASS_AGENTS = [
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
 * as the reference and lists the copies that differ from it, e.g. "Differs
 * in webvitals.com · shared (from Global · Claude Code)"; with no strict
 * majority, every copy is listed instead, e.g. "Copies differ: Global ·
 * Claude Code, webvitals.com · shared".
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
      detail = `Differs in ${differingLabels.join(", ")} (from ${majorityLabel})`;
    } else {
      detail = `Copies differ: ${withHash.map(deploymentLabel).join(", ")}`;
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
        detail: `${deployment.agent} · ${deployment.path}`,
      });
    }
  }
  return issues;
}

/**
 * Skills whose SKILL.md violates one or more agentskills.io spec rules.
 */
export function findSpecViolations(skills: InstalledSkill[]): HealthIssue[] {
  return skills
    .filter((skill) => skill.spec_violations.length > 0)
    .map((skill) => ({
      kind: "spec-violation" as const,
      skill,
      detail: skill.spec_violations.join("; "),
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
 * Deployed skills with no recorded invocation in `stats`.
 */
export function findNeverInvoked(
  skills: InstalledSkill[],
  stats: SkillInvocationStats[],
): HealthIssue[] {
  const totals = new Map(stats.map((s) => [s.skill, s.total]));
  return skills
    .filter((skill) => skill.deployments.length > 0 && (totals.get(skill.name) ?? 0) === 0)
    .map((skill) => ({
      kind: "never-invoked" as const,
      skill,
      detail: "No recorded invocations",
    }));
}

/**
 * Skills deployed to some, but not all, of the four first-class agents at
 * the same scope (global, or a given project).
 */
export function findMissingFromAgents(skills: InstalledSkill[]): HealthIssue[] {
  const issues: HealthIssue[] = [];

  for (const skill of skills) {
    const groups = new Map<string, Set<string>>();
    for (const deployment of skill.deployments) {
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
        const missing = FIRST_CLASS_AGENTS.filter((a) => !agents.has(a));
        issues.push({
          kind: "missing-from-agents",
          skill,
          detail: `${groupKey === "global" ? "Global" : groupKey.slice("project:".length)}: missing from ${missing.join(", ")}`,
        });
      }
    }
  }

  return issues;
}

/**
 * Every dashboard-worthy issue across `skills`: duplicate, broken-symlink,
 * spec-violation, lock-only, and missing-from-agents. Deliberately excludes
 * never-invoked - it's noise, not something worth fixing for every skill.
 * Sorted by `HEALTH_ISSUE_KIND_ORDER` then skill name, so both the dashboard
 * and the Issues view show a stable order.
 */
export function collectDashboardIssues(skills: InstalledSkill[]): HealthIssue[] {
  const issues = [
    ...findDuplicateSkills(skills),
    ...findBrokenSymlinks(skills),
    ...findSpecViolations(skills),
    ...findLockOnlySkills(skills),
    ...findMissingFromAgents(skills),
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
