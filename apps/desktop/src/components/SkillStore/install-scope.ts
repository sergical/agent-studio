// ============================================================================
// install-scope - Deriving the remove/update scope for a skill from its own
// deployments. Split out of InstallControls.tsx so the logic is testable
// without rendering the component (the repo has no DOM test environment) and
// so InstallControls.tsx only exports the component.
// ============================================================================

import type { InstallScope, SkillWithStatus } from "@skill-studio/lib";

export interface ResolvedInstallScope {
  scope: InstallScope;
  /** `null` for a global action, or the project directory a project action targets. */
  projectPath: string | null;
}

/**
 * Derive the scope a remove/update action should target for an installed
 * skill, from the skill's own `installed_info.deployments` - the same data
 * the SkillDetail page's `RemoveDeploymentsDialog` reads. A project
 * deployment carrying a `project_path` wins (project scope + that path);
 * every other shape (no deployments, global-only, plugin, parked, a project
 * deployment without a path) falls back to global.
 *
 * The previous `InstallControls` design kept a single `installScope` state
 * defaulted to `"global"` that was only ever mutated by the not-installed
 * branch's scope toggle, so a drawer reopened over a project-scoped skill
 * reset to `"global"` and silently removed/updated the wrong deployment.
 * Deriving from the deployments (instead of state) leaves nothing to reset,
 * so a remount always targets the deployment's real scope.
 *
 * For a skill deployed in several project locations, a single scope can't
 * represent them all; this returns the first project deployment, matching
 * the SkillStore drawer's single-action model. Per-deployment remove
 * buttons (as on the SkillDetail page) remain the real fix for that case.
 */
export function resolveInstallScope(skill: SkillWithStatus): ResolvedInstallScope {
  const projectDep = skill.installed_info?.deployments.find(
    (d) => d.scope === "project" && Boolean(d.project_path),
  );
  if (projectDep?.project_path) {
    return { scope: "project", projectPath: projectDep.project_path };
  }
  return { scope: "global", projectPath: null };
}
