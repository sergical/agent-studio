// ============================================================================
// Skill Studio - Feature flags
// Build-time defaults with a per-machine localStorage override, so unfinished
// features stay in the codebase without shipping in the UI. Toggle one from
// the devtools console:
//   localStorage.setItem("feature:skill-packs", "on")   // or "off"
// and reload. Flags are read at render time; there is no live subscription.
// ============================================================================

const FLAG_DEFAULTS = {
  /**
   * Multi-select in the Skills table, "Create pack", and the Packs view.
   * Off until packs earn a place in v1.
   */
  "skill-packs": false,
} as const;

export type FeatureFlag = keyof typeof FLAG_DEFAULTS;

/** The localStorage override key for `flag`. */
function storageKey(flag: FeatureFlag): string {
  return `feature:${flag}`;
}

export function isFeatureEnabled(flag: FeatureFlag): boolean {
  try {
    const override = localStorage.getItem(storageKey(flag));
    if (override === "on") return true;
    if (override === "off") return false;
  } catch {
    // Storage unavailable (private mode, etc.) - fall through to the default.
  }
  return FLAG_DEFAULTS[flag];
}
