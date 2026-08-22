// ============================================================================
// Agent Studio - skill-health
// Pure functions over InstalledSkill[] that flag things worth the user's
// attention. No Tauri/DOM access here so these stay unit-testable in
// isolation (Vitest, once a runner is wired up).
// ============================================================================

import type { InstalledSkill, SkillInvocationStats } from "./skill-types";

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

/** The first-class agents `findMissingFromAgents` expects full coverage across. */
const FIRST_CLASS_AGENTS = ["Claude Code", "Codex", "OpenCode", "pi"] as const;

/**
 * Agents that natively discover the shared `.agents/skills` root without a
 * symlink (Claude Code does not), so a "shared" deployment counts as
 * coverage for each of them. See docs/agent-skill-conventions.md.
 */
const SHARED_ROOT_READERS = ["Codex", "OpenCode", "pi"] as const;

/** Which first-class agents one deployment gives coverage for. */
function agentsCoveredByDeployment(agent: string): readonly string[] {
  if (agent === "shared") return SHARED_ROOT_READERS;
  return FIRST_CLASS_AGENTS.some((first) => first === agent) ? [agent] : [];
}

/**
 * Skills whose deployments disagree on content: the same skill name has more
 * than one distinct `content_hash` across its deployments (e.g. a stale copy
 * left behind by a manual edit).
 */
export function findDuplicateSkills(skills: InstalledSkill[]): HealthIssue[] {
  return skills
    .filter((skill) => skill.content_hashes.length > 1)
    .map((skill) => ({
      kind: "duplicate" as const,
      skill,
      detail: `${skill.content_hashes.length} distinct content hashes across deployments`,
    }));
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
