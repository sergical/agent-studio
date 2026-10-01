// ============================================================================
// skill-page-deployment - Resolves the one deployment SkillPage edits: the
// caller's requested copy when given (never silently falling back to a
// different one), otherwise the copy with spec violations (blocking first),
// else the skill's first editable, then own, then any deployment.
// ============================================================================

import {
  deploymentWithSpecViolations,
  editableDeployments,
  isUnresolvedDeployment,
  ownDeployments,
  skillMdPathForDeployment,
} from "@skill-studio/lib";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import type { PinnedDeployment } from "../../lib/nav-history";

interface SkillPageDeployment {
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
 * given. With no `deploymentPath` at all, prefers a readable copy that has
 * spec violations (see `deploymentWithSpecViolations`) so the warning is
 * visible; a skill with none falls back to its first physical file (a symlink only points at another copy), then its first own
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
      : (deploymentWithSpecViolations(skill) ??
        (editableDeployments(skill)[0] || ownDeployments(skill)[0] || skill.deployments[0]))
    : undefined;
  const isDeploymentBroken = Boolean(deployment && isUnresolvedDeployment(deployment));
  const skillMdPath =
    deployment && !isDeploymentBroken ? skillMdPathForDeployment(deployment) : undefined;
  const isPluginManaged = Boolean(deployment?.plugin);
  return { deployment, deploymentUnresolved, isDeploymentBroken, skillMdPath, isPluginManaged };
}

/**
 * `pinned` unchanged unless the page must pick a default copy again. Another skill always resets
 * the pin, even when the caller requested a path, so a stale pin never outlives a visit elsewhere.
 * Within one skill, a requested `deploymentPath` leaves the pin alone, and without one the pin
 * moves only when its copy is gone from the skill. The store clears the pin on every fresh
 * `openSkill` (all but back/forward), so a new visit resolves the default copy again.
 */
export function repinDeployment(
  pinned: PinnedDeployment,
  skill: InstalledSkill | null,
  deploymentPath: string | undefined,
): PinnedDeployment {
  if (!skill) return pinned;
  if (deploymentPath) {
    return pinned.skillName === skill.name ? pinned : { skillName: skill.name, path: undefined };
  }
  const stillPinned =
    pinned.skillName === skill.name && skill.deployments.some((d) => d.path === pinned.path);
  if (stillPinned) return pinned;
  const path = resolveSkillPageDeployment(skill, undefined).deployment?.path;
  return pinned.skillName === skill.name && pinned.path === path
    ? pinned
    : { skillName: skill.name, path };
}
