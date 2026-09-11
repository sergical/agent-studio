// ============================================================================
// Skill Studio TUI - Inventory list rows
// Pure helpers that turn an `Inventory` into rows the inventory screen's
// `<select>` renders: grouped by harness, showing scope, provenance, and
// state. Kept free of any rendering so it is testable on its own.
// ============================================================================

import type { DeploymentDto, InstalledSkillDto, Inventory } from "./cli-types.ts";

export interface SkillRow {
  skill: InstalledSkillDto;
  /** Harness (or `universal`/`parked`) the skill's first deployment belongs to. */
  groupLabel: string;
  /** One-line scope, deployment count, and state flags for the option description. */
  stateSummary: string;
}

function harnessGroupLabel(deployment: DeploymentDto): string {
  const kind = deployment.root.kind;
  if (deployment.harness !== null) {
    if (kind.kind === "legacy") return `${deployment.harness} (legacy)`;
    if (kind.kind === "plugin_cache") return `${deployment.harness} (plugin)`;
    return deployment.harness;
  }
  return kind.kind === "parked" ? "parked" : "universal";
}

function scopeLabel(deployment: DeploymentDto): string {
  return deployment.root.scope.scope === "project" ? "project" : "global";
}

function deploymentStateFlags(deployment: DeploymentDto): string[] {
  const flags: string[] = [];
  if (deployment.disabled_by !== null) flags.push("disabled");
  if (deployment.root.kind.kind === "parked") flags.push("parked");
  if (deployment.spec_violations.length > 0) flags.push("spec violation");
  if (deployment.backing === "linked_to" && deployment.link_target === null)
    flags.push("broken link");
  return flags;
}

/** Groups every skill by its primary (first) deployment's harness, sorted by
 * group then by skill name. */
export function buildSkillRows(inventory: Inventory): SkillRow[] {
  const rows = inventory.skills.map((skill): SkillRow => {
    const primary = skill.deployments[0];
    const groupLabel = primary === undefined ? "unknown" : harnessGroupLabel(primary);
    const scopes = new Set(skill.deployments.map(scopeLabel));
    const flags = new Set(skill.deployments.flatMap(deploymentStateFlags));
    const count = skill.deployments.length;
    const parts = [
      `${String(count)} deployment${count === 1 ? "" : "s"}`,
      Array.from(scopes).join("/"),
    ];
    for (const flag of flags) parts.push(flag);
    return { skill, groupLabel, stateSummary: parts.join(" · ") };
  });
  return rows.sort((a, b) => {
    const byGroup = a.groupLabel.localeCompare(b.groupLabel);
    return byGroup !== 0 ? byGroup : a.skill.name.localeCompare(b.skill.name);
  });
}
