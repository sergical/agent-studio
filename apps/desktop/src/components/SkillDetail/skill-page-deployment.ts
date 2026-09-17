// ============================================================================
// skill-page-deployment - Resolves the one deployment SkillPage edits: the
// caller's requested copy when given (never silently falling back to a
// different one), otherwise the skill's first editable, then own, then any
// deployment.
// ============================================================================

import {
  editableDeployments,
  isUnresolvedDeployment,
  ownDeployments,
  skillMdPathForDeployment,
} from "@skill-studio/lib";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";

export interface SkillPageDeployment {
  deployment: Deployment | undefined;
  /** A caller-requested `deploymentPath` that no longer matches any deployment (the copy was
   * removed by a rescan) - must not silently fall back to a different copy of the skill. */
  deploymentUnresolved: boolean;
  /** A broken deployment symlink can't be read at all - `SkillRepairCard` takes over the
   * SKILL.md card's spot instead of firing the doomed `readInstalledSkillMd` for it. */
  isDeploymentBroken: boolean;
  skillMdPath: string | undefined;
  isPluginManaged: boolean;
}

/**
 * The deployment this page edits: only the one the caller clicked, when
 * given. With no `deploymentPath` at all, falls back to the skill's first
 * physical file (a symlink only points at another copy), then its first own
 * deployment, then its first deployment (a plugin-only skill has no own
 * deployment).
 */
export function resolveSkillPageDeployment(
  skill: InstalledSkill | null,
  deploymentPath: string | undefined,
): SkillPageDeployment {
  const requestedDeployment =
    skill && deploymentPath ? skill.deployments.find((d) => d.path === deploymentPath) : undefined;
  const deploymentUnresolved = Boolean(skill && deploymentPath && !requestedDeployment);
  const deployment = skill
    ? deploymentPath
      ? requestedDeployment
      : editableDeployments(skill)[0] || ownDeployments(skill)[0] || skill.deployments[0]
    : undefined;
  const isDeploymentBroken = Boolean(deployment && isUnresolvedDeployment(deployment));
  const skillMdPath =
    deployment && !isDeploymentBroken ? skillMdPathForDeployment(deployment) : undefined;
  const isPluginManaged = Boolean(deployment?.plugin);
  return { deployment, deploymentUnresolved, isDeploymentBroken, skillMdPath, isPluginManaged };
}
