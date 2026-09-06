// ============================================================================
// skill-location-helpers - pure functions shared by the Locations card's
// action wiring. Split out so `skill-location-status.ts` only exports the
// card's status model (react-doctor/only-export-components: mixing component
// and non-component exports in one file defeats Fast Refresh).
// ============================================================================

import { agentIdFromDeploymentLabel } from "@skill-studio/lib";
import type { Deployment } from "@skill-studio/lib";
import type { ScopeGroup } from "./skill-location-status";

/** Harnesses with a per-skill disable switch - see `skill_harness_disable.rs`. */
const HARNESSES_WITH_PER_SKILL_DISABLE = ["codex", "open-code", "claude-code"];

type SharedFolderSwitchAction = { kind: "park" } | { kind: "unpark" };

interface SharedFolderSwitchPolicy {
  checked: boolean;
  disabled: boolean;
  actionForCheckedChange: (enabled: boolean) => SharedFolderSwitchAction | null;
}

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

/** Keep Project Universal visibility read-only; only the Global switch may park or unpark. */
export function sharedFolderSwitchPolicy(group: ScopeGroup): SharedFolderSwitchPolicy {
  return {
    checked: group.shared?.switchOn ?? false,
    disabled: !group.isGlobal,
    actionForCheckedChange: (enabled) =>
      group.isGlobal ? (enabled ? { kind: "unpark" } : { kind: "park" }) : null,
  };
}
