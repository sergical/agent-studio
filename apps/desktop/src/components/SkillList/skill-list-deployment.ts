// ============================================================================
// skill-list-deployment - The copy a skill-list row opens on for the current
// scope filter.
// ============================================================================

import { deploymentWithSpecViolations, isProjectScope } from "@skill-studio/lib";
import type { InstalledSkill, SkillListFilter } from "@skill-studio/lib";

/** The deployment the current scope shows for `skill`, so the detail drawer opens on that copy.
 * With no scope, the copy carrying spec violations, so the row's badge and the page agree. */
export function deploymentForScope(
  skill: InstalledSkill,
  scope: SkillListFilter["scope"],
): string | undefined {
  if (scope === "global") {
    return skill.deployments.find((d) => d.scope === "global" || d.scope === "plugin")?.path;
  }
  if (scope === "parked") return skill.deployments.find((d) => d.scope === "parked")?.path;
  if (isProjectScope(scope)) {
    return skill.deployments.find((d) => d.project_path === scope.project)?.path;
  }
  return deploymentWithSpecViolations(skill)?.path;
}
