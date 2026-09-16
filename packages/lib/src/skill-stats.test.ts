// ============================================================================
// Skill Studio - skill-stats tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { heatmapDateRangeLocal, mondayLead, recentWeeks, shortSha } from "./skill-stats";

describe("shortSha", () => {
  it("truncates a full sha to 7 characters", () => {
    expect(shortSha("1111111111111111111111111111111111aaaa")).toBe("1111111");
  });

  it("leaves a sha already at or under 7 characters alone", () => {
    expect(shortSha("abc123")).toBe("abc123");
  });
});

describe("mondayLead", () => {
  it("counts the blank slots before the first day in a Monday-first week", () => {
    expect(mondayLead("2026-09-14")).toBe(0);
    expect(mondayLead("2026-09-16")).toBe(2);
    expect(mondayLead("2026-09-13")).toBe(6);
  });
});

describe("recentWeeks", () => {
  // Wednesday 2026-09-02 through Sunday 2026-09-13: a partial week, then a whole one.
  const twoWeeks = ["02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13"].map(
    (d) => `2026-09-${d}`,
  );

  it("keeps every date when all the weeks fit", () => {
    expect(recentWeeks(twoWeeks, 2)).toBe(twoWeeks);
    expect(recentWeeks(twoWeeks, 10)).toBe(twoWeeks);
  });

  it("drops the oldest weeks and starts the rest on a Monday", () => {
    expect(recentWeeks(twoWeeks, 1)).toEqual(twoWeeks.slice(5));
  });

  it("always keeps at least one week", () => {
    expect(recentWeeks(twoWeeks, 0)).toEqual(twoWeeks.slice(5));
  });

  it("returns an empty range unchanged", () => {
    expect(recentWeeks([], 3)).toEqual([]);
  });

  it("ends a trimmed year on its last day", () => {
    const { dates, end } = heatmapDateRangeLocal(new Date(2026, 8, 16));
    const shown = recentWeeks(dates, 3);
    expect(mondayLead(shown[0])).toBe(0);
    expect(shown[shown.length - 1]).toBe(end);
    // Two whole weeks plus Monday to Wednesday.
    expect(shown).toHaveLength(17);
  });
});
