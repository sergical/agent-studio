// ============================================================================
// Skill Studio - skill-plugin-partition
// Pure functions that separate the user's own skills from ones shipped by an
// agent's plugin cache, so plugin-managed skills (Codex's openai-templates,
// Claude Code plugin caches, ~200 of the 308 rows on the old dashboard) stop
// counting toward "your skills", health checks, and coverage.
// ============================================================================

import type { Deployment, InstalledSkill } from "./skill-types";

/** One plugin's skills, grouped for the Plugins view. */
export interface PluginGroup {
  harness: string;
  pluginName: string;
  version?: string;
  skills: InstalledSkill[];
}

/** `skill`'s deployments that are not plugin-managed. */
export function ownDeployments(skill: InstalledSkill): Deployment[] {
  return skill.deployments.filter((d) => d.plugin === undefined);
}

/** `skill`'s deployments that are plugin-managed. */
export function pluginDeployments(skill: InstalledSkill): Deployment[] {
  return skill.deployments.filter((d) => d.plugin !== undefined);
}

/**
 * Every skill with at least one non-plugin deployment, each cloned with
 * `deployments` narrowed to just the owned ones. A mixed-origin skill never
 * carries its plugin deployments along here, so they can't inflate scope
 * lists, agent chips, coverage, or stat counts derived from this view.
 */
export function ownSkillsView(skills: InstalledSkill[]): InstalledSkill[] {
  return skills
    .filter((skill) => ownDeployments(skill).length > 0)
    .map((skill) => ({ ...skill, deployments: ownDeployments(skill) }));
}

/** The plugin-deployment mirror of `ownSkillsView`, for plugin-side counts. */
export function pluginSkillsView(skills: InstalledSkill[]): InstalledSkill[] {
  return skills
    .filter((skill) => pluginDeployments(skill).length > 0)
    .map((skill) => ({ ...skill, deployments: pluginDeployments(skill) }));
}

/** The name of the first plugin deployment shipping `skill`, if any. */
export function pluginLabelForSkill(skill: InstalledSkill): string | undefined {
  return skill.deployments.find((d) => d.plugin)?.plugin?.name;
}

/** The absolute path to `deployment`'s `SKILL.md`, for read/write commands. */
export function skillMdPathForDeployment(deployment: Deployment): string {
  return `${deployment.path}/SKILL.md`;
}

/**
 * Groups every plugin-shipped skill by (harness, plugin name), sorted by
 * harness then plugin name. A skill appears under each plugin that ships it.
 */
export function groupPluginSkills(skills: InstalledSkill[]): PluginGroup[] {
  const groups = new Map<string, PluginGroup>();

  for (const skill of skills) {
    for (const deployment of skill.deployments) {
      if (!deployment.plugin) continue;
      const key = `${deployment.plugin.harness}::${deployment.plugin.name}`;
      let group = groups.get(key);
      if (!group) {
        group = {
          harness: deployment.plugin.harness,
          pluginName: deployment.plugin.name,
          version: deployment.plugin.version,
          skills: [],
        };
        groups.set(key, group);
      }
      if (!group.skills.includes(skill)) {
        group.skills.push(skill);
      }
    }
  }

  return [...groups.values()].sort(
    (a, b) => a.harness.localeCompare(b.harness) || a.pluginName.localeCompare(b.pluginName),
  );
}

/**
 * Distinct skill count per harness with plugin-shipped skills, for the
 * Plugins sidebar section. Built from `groupPluginSkills` (deployments), so
 * a skill that also has an own deployment elsewhere still counts toward its
 * harness here.
 */
export function pluginHarnessCounts(skills: InstalledSkill[]): Map<string, number> {
  const namesByHarness = new Map<string, Set<string>>();
  for (const group of groupPluginSkills(skills)) {
    const names = namesByHarness.get(group.harness) ?? new Set<string>();
    for (const skill of group.skills) names.add(skill.name);
    namesByHarness.set(group.harness, names);
  }
  return new Map([...namesByHarness].map(([harness, names]) => [harness, names.size]));
}
