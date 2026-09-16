// ============================================================================
// Skill Studio - skill-activity tests
// ============================================================================

import { describe, expect, it } from "vitest";
import {
  ALL_ACTIVITY,
  dayBreakdown,
  dayDetail,
  dayTotals,
  shiftDay,
  skillRows,
  usualDay,
  windowSummary,
} from "./skill-activity";
import type { ActivityFilter } from "./skill-activity";
import type { SkillInvocationStats, SkillUseHour } from "./skill-types";

/** One hourly bucket, overridable per test. */
function hourBucket(
  hour: number,
  overrides: Partial<Omit<SkillUseHour, "hour">> = {},
): SkillUseHour {
  return {
    hour,
    harness: "claude-code",
    trigger: "agent",
    project_path: null,
    count: 1,
    ...overrides,
  };
}

/** Minimal `SkillInvocationStats` fixture, overridable per test. */
function fixtureStats(
  skill: string,
  byHour: SkillUseHour[],
  overrides: Partial<SkillInvocationStats> = {},
): SkillInvocationStats {
  return {
    skill,
    total: byHour.reduce((sum, b) => sum + b.count, 0),
    last_24_hours: 0,
    last_7_days: 0,
    last_14_days: 0,
    last_30_days: 0,
    last_used: null,
    by_project_30_days: {},
    by_day: {},
    by_harness_30_days: {},
    by_trigger_30_days: { user: 0, agent: 0, file_read: 0 },
    by_hour: byHour,
    ...overrides,
  };
}

// A fixed whole hour, far from any epoch edge case, used as the base for every test.
const BASE_HOUR = 486_123;
const baseLocal = new Date(BASE_HOUR * 3_600_000);
const baseDayKey = `${baseLocal.getFullYear()}-${String(baseLocal.getMonth() + 1).padStart(2, "0")}-${String(baseLocal.getDate()).padStart(2, "0")}`;
const baseLocalHour = baseLocal.getHours();

describe("local day and hour conversion", () => {
  it("buckets a use into its local day key and local hour of day", () => {
    const stats = [fixtureStats("write-tests", [hourBucket(BASE_HOUR, { count: 3 })])];
    const detail = dayDetail(stats, baseDayKey, ALL_ACTIVITY);
    expect(detail.total).toBe(3);
    expect(detail.byHour[baseLocalHour]).toBe(3);
  });

  it("a use lands on a different local day is not counted for this day", () => {
    const otherDayHour = BASE_HOUR + 24;
    const stats = [fixtureStats("write-tests", [hourBucket(otherDayHour, { count: 1 })])];
    // Only assert when the shifted hour actually lands on a different local day -
    // true for every offset except right at a DST fold, which 24h always clears.
    const detail = dayDetail(stats, baseDayKey, ALL_ACTIVITY);
    expect(detail.total).toBe(0);
  });
});

describe("filter by harness and trigger", () => {
  const stats = [
    fixtureStats("write-tests", [
      hourBucket(BASE_HOUR, { harness: "claude-code", trigger: "agent", count: 5 }),
      hourBucket(BASE_HOUR, { harness: "codex", trigger: "file_read", count: 2 }),
    ]),
  ];

  it("harness filter keeps only matching buckets", () => {
    const filter: ActivityFilter = { harness: "codex", trigger: "all" };
    expect(dayBreakdown(stats, baseDayKey, filter).total).toBe(2);
  });

  it("trigger filter keeps only matching buckets", () => {
    const filter: ActivityFilter = { harness: "all", trigger: "agent" };
    expect(dayBreakdown(stats, baseDayKey, filter).total).toBe(5);
  });

  it("no filter counts everything", () => {
    expect(dayBreakdown(stats, baseDayKey, ALL_ACTIVITY).total).toBe(7);
  });
});

describe("window cutoff", () => {
  it("excludes a bucket older than the window", () => {
    const now = new Date(BASE_HOUR * 3_600_000 + 3_600_000);
    const withinWindow = hourBucket(BASE_HOUR, { count: 4 });
    const outsideWindow = hourBucket(BASE_HOUR - 24 * 40, { count: 9 });
    const stats = [fixtureStats("write-tests", [withinWindow, outsideWindow])];
    const summary = windowSummary(stats, "30d", ALL_ACTIVITY, now);
    expect(summary.total).toBe(4);
  });
});

describe("usualDay", () => {
  it("is the median of the non-zero days", () => {
    expect(usualDay({ a: 1, b: 5, c: 3, d: 0 })).toBe(3);
  });

  it("is 0 with no non-zero days", () => {
    expect(usualDay({ a: 0 })).toBe(0);
  });
});

describe("dayBreakdown totals", () => {
  it("equal the sum of its splits", () => {
    const stats = [
      fixtureStats("write-tests", [
        hourBucket(BASE_HOUR, { harness: "claude-code", trigger: "agent", count: 3 }),
      ]),
      fixtureStats("lint-code", [
        hourBucket(BASE_HOUR, { harness: "codex", trigger: "file_read", count: 2 }),
      ]),
    ];
    const breakdown = dayBreakdown(stats, baseDayKey, ALL_ACTIVITY);
    const sumOf = (counts: Record<string, number | undefined>) =>
      Object.values(counts).reduce((sum, n) => sum + (n ?? 0), 0);
    expect(breakdown.total).toBe(5);
    expect(sumOf(breakdown.bySkill)).toBe(breakdown.total);
    expect(sumOf(breakdown.byHarness)).toBe(breakdown.total);
    // SAFETY: Counts<SkillTrigger> is structurally Record<string, number | undefined>; only the
    // key type narrows.
    expect(sumOf(breakdown.byTrigger as Record<string, number | undefined>)).toBe(breakdown.total);
  });
});

describe("shiftDay", () => {
  const dates = ["2026-09-14", "2026-09-15", "2026-09-16"];

  it("returns null past the start", () => {
    expect(shiftDay(dates, "2026-09-14", -1)).toBeNull();
  });

  it("returns null past the end", () => {
    expect(shiftDay(dates, "2026-09-16", 1)).toBeNull();
  });

  it("returns the neighboring date within range", () => {
    expect(shiftDay(dates, "2026-09-15", 1)).toBe("2026-09-16");
    expect(shiftDay(dates, "2026-09-15", -1)).toBe("2026-09-14");
  });
});

describe("skillRows lastUsed", () => {
  const now = new Date(BASE_HOUR * 3_600_000 + 3_600_000);

  it("uses the stats' exact last_used with no filter", () => {
    const stats = [
      fixtureStats("write-tests", [hourBucket(BASE_HOUR, { count: 1 })], {
        last_used: "2026-01-01T00:00:00Z",
      }),
    ];
    const rows = skillRows(stats, "30d", ALL_ACTIVITY, now);
    expect(rows[0].lastUsed).toBe("2026-01-01T00:00:00Z");
  });

  it("uses the latest matching bucket's start when filtered", () => {
    const stats = [
      fixtureStats(
        "write-tests",
        [
          hourBucket(BASE_HOUR, { harness: "claude-code", count: 1 }),
          hourBucket(BASE_HOUR - 1, { harness: "codex", count: 1 }),
        ],
        { last_used: "2026-01-01T00:00:00Z" },
      ),
    ];
    const rows = skillRows(stats, "30d", { harness: "codex", trigger: "all" }, now);
    expect(rows[0].lastUsed).toBe(new Date((BASE_HOUR - 1) * 3_600_000).toISOString());
  });
});

describe("dayTotals", () => {
  it("sums by local day, ignoring the window", () => {
    const stats: SkillInvocationStats[] = [
      fixtureStats("write-tests", [hourBucket(BASE_HOUR, { count: 2 })]),
    ];
    const totals = dayTotals(stats, ALL_ACTIVITY);
    expect(totals[baseDayKey]).toBe(2);
  });
});
