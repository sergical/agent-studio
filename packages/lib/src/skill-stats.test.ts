// ============================================================================
// Skill Studio - skill-stats tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { shortSha } from "./skill-stats";

describe("shortSha", () => {
  it("truncates a full sha to 7 characters", () => {
    expect(shortSha("1111111111111111111111111111111111aaaa")).toBe("1111111");
  });

  it("leaves a sha already at or under 7 characters alone", () => {
    expect(shortSha("abc123")).toBe("abc123");
  });
});
