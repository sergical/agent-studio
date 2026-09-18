// ============================================================================
// skill-location-helpers - pure functions shared by the Locations card's
// action wiring. Split out so `skill-location-status.ts` only exports the
// card's status model (react-doctor/only-export-components: mixing component
// and non-component exports in one file defeats Fast Refresh).
// ============================================================================

import { agentIdFromDeploymentLabel } from "@skill-studio/lib";
import type { Deployment } from "@skill-studio/lib";
import type { AgentLocationRow, ScopeGroup } from "./skill-location-status";

/** Shown on a disabled Enabled switch that has no way to turn the row off - see `canOfferHarnessSwitch`. */
export const NO_OFF_SWITCH_TITLE =
  "This copy has no off switch; park the skill from the header instead";

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

/**
 * Whether a per-harness row's Enabled switch has anywhere to send a
 * toggle-off. `park`/`unpark` are the Global Universal deployment's off
 * switch only (see `ops::park` in the core crate) - never this row's. A
 * `studio-moved` row is the one legacy exception with a way back in
 * (`restore_moved_deployment`); every other row with no native per-harness
 * disable has no off switch at all, and the caller must disable the control
 * instead of offering it.
 */
export function canOfferHarnessSwitch(deployment: Deployment): boolean {
  return deployment.disabled_by === "studio-moved" || canToggleHarness(deployment);
}

/**
 * Whether the Harnesses rail's switch should be interactive for `row`.
 * `park` is the off switch only for the Global Universal deployment - which
 * never reaches the rail's Harnesses popover, since it filters out `"shared"`
 * rows - so a row here offers a switch only when its harness has a native
 * per-skill mechanism (`canToggleHarness`), or the row is a legacy
 * `.skill-studio-disabled/` copy that can still be switched back on via
 * `restoreMovedDeployment`. Every other row (a project-scope copy, pi,
 * Cursor, Grok Build) has no off switch at all.
 */
export function canOfferHarnessSwitchForRow(row: AgentLocationRow): boolean {
  if (row.kind === "reader") return row.hasSwitch;
  if (!row.deployment) return false;
  return canOfferHarnessSwitch(row.deployment);
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
