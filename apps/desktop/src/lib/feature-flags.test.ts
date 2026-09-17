// ============================================================================
// Skill Studio - Feature flags tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { isFeatureEnabled } from "./feature-flags";
import type { FeatureFlag } from "./feature-flags";

// Compile-time guard: if "skill-packs" ever rejoins `FeatureFlag`, `Extract`
// yields "skill-packs" instead of `never` and this assignment stops
// typechecking - `npm run typecheck` then fails the same way a runtime
// assertion would. Packs are deferred with no runtime flag (unit 4.3).
type _NoSkillPacksFlag = Extract<FeatureFlag, "skill-packs"> extends never
  ? true
  : ["skill-packs must not be a FeatureFlag - see unit 4.3"];
const _noSkillPacksFlag: _NoSkillPacksFlag = true;
void _noSkillPacksFlag;

describe("isFeatureEnabled", () => {
  it("reads the skill-assistant default off with no localStorage override", () => {
    expect(isFeatureEnabled("skill-assistant")).toBe(false);
  });
});
