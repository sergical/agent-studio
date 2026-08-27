// ============================================================================
// Skill Studio - home-summary
// Pure helpers behind Home's summary line and "Recently used" block, kept
// separate from HomeView.tsx so they're unit-testable without React.
// ============================================================================

import { FIRST_CLASS_AGENTS } from "./skill-health";
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

  const harnesses = new Set<string>();
  for (const skill of own) {
    for (const deployment of skill.deployments) {
      if (FIRST_CLASS_AGENTS.some((agent) => agent === deployment.agent)) {
        harnesses.add(deployment.agent);
      }
    }
  }

  const usesIn30Days = snapshot.invocations.reduce(
    (sum, stat) => sum + invocationsInWindow(stat, "30d"),
    0,
  );

  return {
    skillCount: own.length,
    pluginSkillCount: pluginSkillsView(snapshot.skills).length,
    harnessCount: harnesses.size,
    projectCount: snapshot.projects.length,
    usesIn30Days,
  };
}

/**
 * Which of `FIRST_CLASS_AGENTS` have at least one own deployment in the
 * snapshot, in `FIRST_CLASS_AGENTS` order - reuses `homeSummaryCounts`'
 * counting logic so Home's harness number and the filter bar's harness chips
 * never disagree about which harnesses "count".
 */
export function harnessesPresent(snapshot: SkillSnapshot): string[] {
  const own = ownSkillsView(snapshot.skills);
  const present = new Set<string>();
  for (const skill of own) {
    for (const deployment of skill.deployments) {
      if (FIRST_CLASS_AGENTS.some((agent) => agent === deployment.agent)) {
        present.add(deployment.agent);
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
