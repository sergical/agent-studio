// ============================================================================
// skill-list-model tests
// Guards the Skills list's per-render cost: grouping 400 skills into the
// three state buckets must stay well under one frame, even before the
// virtualizer trims what's actually painted.
// ============================================================================

import { describe, expect, it } from "vitest";
import { buildHarnessSnapshot } from "../dev/harness/skill-fixture";
import { selectNewerSkillSnapshot } from "../hooks/useSkillSnapshot";
import { groupSkillRows } from "./skill-list-model";

function median(values: number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0 ? (sorted[mid - 1] + sorted[mid]) / 2 : sorted[mid];
}

describe("groupSkillRows at 400 skills", () => {
  it("groups every skill into exactly one of the three state buckets", () => {
    const snapshot = selectNewerSkillSnapshot(undefined, buildHarnessSnapshot(400));
    if (!snapshot) throw new Error("selectNewerSkillSnapshot dropped the only candidate");

    const { buckets } = groupSkillRows(snapshot.skills, "name", snapshot.invocations);

    expect(buckets.attention.length + buckets.healthy.length + buckets.parked.length).toBe(400);
  });

  it("runs in well under one frame - median of 5 runs after a warm-up, under 5ms", () => {
    const harnessSnapshot = buildHarnessSnapshot(400);

    const warmUp = selectNewerSkillSnapshot(undefined, harnessSnapshot);
    if (!warmUp) throw new Error("selectNewerSkillSnapshot dropped the only candidate");
    groupSkillRows(warmUp.skills, "name", warmUp.invocations);

    const durations: number[] = [];
    for (let i = 0; i < 5; i++) {
      const start = performance.now();
      const snapshot = selectNewerSkillSnapshot(undefined, harnessSnapshot);
      if (!snapshot) throw new Error("selectNewerSkillSnapshot dropped the only candidate");
      groupSkillRows(snapshot.skills, "name", snapshot.invocations);
      durations.push(performance.now() - start);
    }

    expect(median(durations)).toBeLessThan(5);
  });
});
