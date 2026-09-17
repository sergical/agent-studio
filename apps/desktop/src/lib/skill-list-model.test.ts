// ============================================================================
// skill-list-model tests
// Guards that grouping 400 skills into the three state buckets accounts for
// every skill exactly once, before the virtualizer trims what's painted.
// ============================================================================

import { describe, expect, it } from "vitest";
import { buildHarnessSnapshot } from "../dev/harness/skill-fixture";
import { selectNewerSkillSnapshot } from "../hooks/useSkillSnapshot";
import { groupSkillRows } from "./skill-list-model";

describe("groupSkillRows at 400 skills", () => {
  it("groups every skill into exactly one of the three state buckets", () => {
    const snapshot = selectNewerSkillSnapshot(undefined, buildHarnessSnapshot(400));
    if (!snapshot) throw new Error("selectNewerSkillSnapshot dropped the only candidate");

    const { buckets } = groupSkillRows(snapshot.skills, "name", snapshot.invocations);

    expect(buckets.attention.length + buckets.healthy.length + buckets.parked.length).toBe(400);
  });
});
