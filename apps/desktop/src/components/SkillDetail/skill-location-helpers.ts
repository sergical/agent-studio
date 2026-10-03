// ============================================================================
// skill-location-helpers - pure functions shared by the Locations card's
// action wiring. Split out so `skill-location-status.ts` only exports the
// card's status model (react-doctor/only-export-components: mixing component
// and non-component exports in one file defeats Fast Refresh).
// ============================================================================

import type { ScopeGroup } from "./skill-location-status";

type SharedFolderSwitchAction = { kind: "park" } | { kind: "unpark" };

interface SharedFolderSwitchPolicy {
  checked: boolean;
  disabled: boolean;
  actionForCheckedChange: (enabled: boolean) => SharedFolderSwitchAction | null;
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
