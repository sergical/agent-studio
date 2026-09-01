// ============================================================================
// Skill Studio - home-summary
// Pure helpers behind Home's summary line and "Recently used" block, kept
// separate from HomeView.tsx so they're unit-testable without React.
// ============================================================================

import {
  FIRST_CLASS_AGENTS,
  HEALTH_ISSUE_SEVERITY,
  agentsCoveredByDeployment,
} from "./skill-health";
import type { HealthIssue } from "./skill-health";
import { ownSkillsView, pluginSkillsView } from "./skill-plugin-partition";
import { invocationsInWindow } from "./skill-stats";
import type { InstalledSkill, SkillInvocationStats, SkillSnapshot } from "./skill-types";

/** The counts on Home's summary line: "N skills · N harnesses · N projects · N uses in 30 days", plus the plugin-shipped skill count shown as a separate segment. */
export interface HomeSummaryCounts {
  skillCount: number;
  /** Skills that ship inside a harness plugin, not installed or written by the user - shown as a separate segment, omitted when zero. */
  pluginSkillCount: number;
  harnessCount: number;
  projectCount: number;
  usesIn30Days: number;
}

/**
 * Counts for Home's summary line. `skillCount` and `harnessCount` are scoped
 * to the user's own skills (`ownSkillsView`) - plugin-shipped skills don't
 * count toward either, but do get their own `pluginSkillCount`.
 * `harnessCount` is how many of `FIRST_CLASS_AGENTS` have at least one own
 * deployment; `projectCount` is every project the snapshot tracks, own or
 * not, since tracking a project isn't tied to whether a skill happens to be
 * deployed there yet.
 */
export function homeSummaryCounts(snapshot: SkillSnapshot): HomeSummaryCounts {
  const own = ownSkillsView(snapshot.skills);

  const usesIn30Days = snapshot.invocations.reduce(
    (sum, stat) => sum + invocationsInWindow(stat, "30d"),
    0,
  );

  return {
    skillCount: own.length,
    pluginSkillCount: pluginSkillsView(snapshot.skills).length,
    harnessCount: harnessesPresent(snapshot).length,
    projectCount: snapshot.projects.length,
    usesIn30Days,
  };
}

/**
 * Which of `FIRST_CLASS_AGENTS` at least one own deployment in the snapshot
 * gives coverage for, in `FIRST_CLASS_AGENTS` order. Coverage, not literal
 * deployments: a skill in the shared `.agents/skills` root is readable by
 * every shared-root reader (Codex, OpenCode, pi, ...) even with no
 * harness-specific deployment - see `agentsCoveredByDeployment`.
 */
export function harnessesPresent(snapshot: SkillSnapshot): string[] {
  const own = ownSkillsView(snapshot.skills);
  const present = new Set<string>();
  for (const skill of own) {
    for (const deployment of skill.deployments) {
      for (const agent of agentsCoveredByDeployment(deployment.agent)) {
        present.add(agent);
      }
    }
  }
  return FIRST_CLASS_AGENTS.filter((agent) => present.has(agent));
}

/** One row of Home's "Recently used" block. */
export interface RecentlyUsedSkill {
  skill: InstalledSkill;
  lastUsed: string;
  /** The project this skill was invoked in the most over the last 30 days, when any invocation carried a project path. */
  projectLabel?: string;
  usesIn30Days: number;
}

/** The basename of the project path with the highest invocation count in `byProject`, if any. */
function topProjectLabel(byProject: Record<string, number>): string | undefined {
  const entries = Object.entries(byProject);
  if (entries.length === 0) return undefined;
  const [path] = entries.sort((a, b) => b[1] - a[1])[0];
  return path.split("/").filter(Boolean).pop() ?? path;
}

/**
 * The `n` skills with the most recent invocation, newest first. Skills with
 * no recorded invocation, or whose stats no longer match an installed skill,
 * are excluded.
 */
export function recentlyUsedSkills(
  skills: InstalledSkill[],
  stats: SkillInvocationStats[],
  n: number,
): RecentlyUsedSkill[] {
  const skillsByName = new Map(skills.map((s) => [s.name, s]));

  const rows: RecentlyUsedSkill[] = [];
  for (const stat of stats) {
    const skill = skillsByName.get(stat.skill);
    if (!stat.last_used || !skill) continue;
    rows.push({
      skill,
      lastUsed: stat.last_used,
      projectLabel: topProjectLabel(stat.by_project_30_days),
      usesIn30Days: stat.last_30_days,
    });
  }

  return rows
    .sort((a, b) => new Date(b.lastUsed).getTime() - new Date(a.lastUsed).getTime())
    .slice(0, n);
}

/** How Home's "Invocation" segment breaks down `skills`' own (non-plugin) skills by policy. */
export interface HomeInvocationCounts {
  both: number;
  modelOnly: number;
  userOnly: number;
}

/** Counts of own skills by `invocation` policy - "both" vs "model-only" vs "user-only". */
export function homeInvocationCounts(skills: InstalledSkill[]): HomeInvocationCounts {
  const own = ownSkillsView(skills);
  const counts: HomeInvocationCounts = { both: 0, modelOnly: 0, userOnly: 0 };
  for (const skill of own) {
    if (skill.invocation === "both") counts.both += 1;
    else if (skill.invocation === "model-only") counts.modelOnly += 1;
    else counts.userOnly += 1;
  }
  return counts;
}

/** Home's per-turn prompt cost breakdown, split into skills used vs. idle in the last 30 days. */
export interface HomePromptCost {
  totalTokens: number;
  usedTokens: number;
  usedCount: number;
  idleTokens: number;
  idleCount: number;
}

/**
 * The prompt cost the model actually pays: only skills the model can invoke
 * (`invocation` is `"both"` or `"model-only"`) contribute their
 * `description_tokens` - a `"user-only"` skill is never in the model's
 * context to begin with. Split into "used" (invoked at least once in the
 * last 30 days) and "idle" (not).
 */
export function homePromptCost(
  skills: InstalledSkill[],
  invocations: SkillInvocationStats[],
): HomePromptCost {
  const last30DaysBySkill = new Map(invocations.map((stat) => [stat.skill, stat.last_30_days]));
  const cost: HomePromptCost = {
    totalTokens: 0,
    usedTokens: 0,
    usedCount: 0,
    idleTokens: 0,
    idleCount: 0,
  };

  for (const skill of skills) {
    if (skill.invocation !== "both" && skill.invocation !== "model-only") continue;
    cost.totalTokens += skill.description_tokens;
    const usedIn30Days = (last30DaysBySkill.get(skill.name) ?? 0) > 0;
    if (usedIn30Days) {
      cost.usedTokens += skill.description_tokens;
      cost.usedCount += 1;
    } else {
      cost.idleTokens += skill.description_tokens;
      cost.idleCount += 1;
    }
  }

  return cost;
}

/**
 * Own skills with no invocation in the last 30 days, model-invocable
 * (`"both"` or `"model-only"`) first, then `"user-only"`, alphabetical
 * within each group - the order Home's "Unused" section and Learn's
 * "unused" section list them in.
 */
export function unusedSkills(
  skills: InstalledSkill[],
  invocations: SkillInvocationStats[],
): InstalledSkill[] {
  const last30DaysBySkill = new Map(invocations.map((stat) => [stat.skill, stat.last_30_days]));
  const own = ownSkillsView(skills).filter(
    (skill) => (last30DaysBySkill.get(skill.name) ?? 0) === 0,
  );

  return own.sort((a, b) => {
    const aModelInvocable = a.invocation !== "user-only";
    const bModelInvocable = b.invocation !== "user-only";
    if (aModelInvocable !== bModelInvocable) return aModelInvocable ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
}

/** Home's "Needs attention" issues, split by severity - errors ("broken") vs. warnings. */
export interface HomeAttentionGroups {
  broken: HealthIssue[];
  warnings: HealthIssue[];
}

/** Splits `issues` into `broken` (severity `"error"`) and `warnings` (severity `"warning"`). */
export function attentionGroups(issues: HealthIssue[]): HomeAttentionGroups {
  const groups: HomeAttentionGroups = { broken: [], warnings: [] };
  for (const issue of issues) {
    if (HEALTH_ISSUE_SEVERITY[issue.kind] === "error") groups.broken.push(issue);
    else groups.warnings.push(issue);
  }
  return groups;
}
