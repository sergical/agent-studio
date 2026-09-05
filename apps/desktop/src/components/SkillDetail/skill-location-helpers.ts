// ============================================================================
// skill-location-helpers - pure functions shared by the Locations card's
// action wiring. Split out so `skill-location-status.ts` only exports the
// card's status model (react-doctor/only-export-components: mixing component
// and non-component exports in one file defeats Fast Refresh).
// ============================================================================

import { agentIdFromDeploymentLabel } from "@skill-studio/lib";
import type { Deployment } from "@skill-studio/lib";

/** Harnesses with a per-skill disable switch - see `skill_harness_disable.rs`. */
const HARNESSES_WITH_PER_SKILL_DISABLE = ["codex", "open-code", "claude-code"];

/**
 * Whether the Enabled switch can actually change this deployment. The disable
 * mechanisms are global: Codex config, OpenCode permission, Claude Code's
 * global per-skill symlink. A project-scope copy has nothing to toggle, so
 * showing the switch there just produces an error - except when the row is
 * already disabled, which must stay re-enableable.
 */
export function canToggleHarness(deployment: Deployment): boolean {
  const id = agentIdFromDeploymentLabel(deployment.agent) ?? "";
  if (!HARNESSES_WITH_PER_SKILL_DISABLE.includes(id)) return false;
  if (deployment.disabled) return true;
  if (deployment.scope !== "global") return false;
  return id !== "claude-code" || deployment.is_symlink;
}

/**
 * The `LocationAction` the Shared folder row's "Enabled everywhere" switch
 * dispatches: `park`/`unpark`, mirroring the shared row's own ⋯ menu
 * (`rowMenu` in `skill-location-status.ts`). Never `set-enabled` - the
 * shared root (`deployment.agent === "shared"`) is not a harness
 * deployment, so `canToggleHarness` returns `false` and the generic
 * `set-enabled` path falls back to `setDeploymentEnabled`, which the
 * backend refuses for shared-root deployments ("park the skill instead").
 * Toggling the switch off parks; on unparks.
 */
export function sharedSwitchAction(next: boolean): { kind: "park" } | { kind: "unpark" } {
  return next ? { kind: "unpark" } : { kind: "park" };
}
