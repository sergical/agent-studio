// ============================================================================
// Skill Studio - skill-stats
// Pure aggregation helpers over a SkillSnapshot: dashboard totals and top
// skills by recent invocations. The skill x agent matrix lives in
// skill-coverage.ts.
// ============================================================================

import type { SkillInvocationStats, SkillSnapshot } from "./skill-types";

/** Aggregate counts shown in the dashboard's stat cards. */
export interface SkillTotals {
  skillCount: number;
  tokens: number;
  bytes: number;
  invocationsLast30Days: number;
}

/**
 * Totals over every skill in the snapshot: count, SKILL.md token sum,
 * folder byte sum, and invocations across all skills in the last 30 days.
 */
export function computeTotals(snapshot: SkillSnapshot): SkillTotals {
  let tokens = 0;
  let bytes = 0;
  for (const skill of snapshot.skills) {
    tokens += skill.skill_md_tokens;
    bytes += skill.folder_bytes;
  }

  let invocationsLast30Days = 0;
  for (const stat of snapshot.invocations) {
    invocationsLast30Days += stat.last_30_days;
  }

  return { skillCount: snapshot.skills.length, tokens, bytes, invocationsLast30Days };
}

/** Formats a byte count as e.g. "1.2 MB", "340 KB", "12 B". */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

/** Formats a token count as e.g. "12.3k" above 1,000, otherwise the raw number. */
export function formatTokens(tokens: number): string {
  if (tokens < 1000) return tokens.toString();
  return `${(tokens / 1000).toFixed(1)}k`;
}

/**
 * Timestamps older than this are treated as unknown rather than formatted:
 * the plugin cache carries epoch (1969-12-31) mtimes for files it hasn't
 * touched, and a "56y ago" reading is worse than omitting the segment.
 */
const UNKNOWN_TIME_CUTOFF = new Date("2000-01-01T00:00:00Z").getTime();

/** Formats an RFC3339 timestamp as e.g. "3d ago", "2w ago", "just now". */
export function formatRelativeTime(iso: string, now: Date = new Date()): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then) || then < UNKNOWN_TIME_CUTOFF) return "unknown";

  const ms = now.getTime() - then;
  if (ms < 0) return "just now";

  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;

  const days = Math.floor(hours / 24);
  if (days < 14) return `${days}d ago`;

  const weeks = Math.floor(days / 7);
  if (days < 365) return `${weeks}w ago`;

  return `${Math.floor(days / 365)}y ago`;
}

/** The usage window selectable on the dashboard and Activity page's "By skill" table. */
export type UsageWindow = "24h" | "7d" | "14d" | "30d";

/** `UsageWindow` options in display order, for the segmented control. */
export const USAGE_WINDOWS: { id: UsageWindow; label: string }[] = [
  { id: "24h", label: "24 hours" },
  { id: "7d", label: "7 days" },
  { id: "14d", label: "14 days" },
  { id: "30d", label: "30 days" },
];

/** A skill's invocation count for the given usage window. */
export function invocationsInWindow(stats: SkillInvocationStats, window: UsageWindow): number {
  switch (window) {
    case "24h":
      return stats.last_24_hours;
    case "7d":
      return stats.last_7_days;
    case "14d":
      return stats.last_14_days;
    case "30d":
      return stats.last_30_days;
  }
}

/** One "YYYY-MM-DD" key built from `date`'s UTC calendar fields, never local time. */
function utcDateKey(date: Date): string {
  const year = date.getUTCFullYear();
  const month = String(date.getUTCMonth() + 1).padStart(2, "0");
  const day = String(date.getUTCDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

/** An inclusive UTC calendar-date range plus its "YYYY-MM-DD" day keys in order. */
export interface HeatmapDateRange {
  start: string;
  end: string;
  dates: string[];
}

/**
 * One inclusive UTC calendar-date range of exactly 364 days ending on `now`'s
 * UTC date, plus its "YYYY-MM-DD" day keys in order (oldest first). Callers
 * that need the heatmap's grid, header total, and aria-label to agree on the
 * exact same set of days should call this once and share the result, rather
 * than each computing its own range.
 */
export function heatmapDateRangeUtc(now: Date): HeatmapDateRange {
  const endUtcMs = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate());
  const dayMs = 24 * 60 * 60 * 1000;
  const dates: string[] = [];
  for (let i = 363; i >= 0; i--) {
    dates.push(utcDateKey(new Date(endUtcMs - i * dayMs)));
  }
  return { start: dates[0], end: dates[dates.length - 1], dates };
}

/** The `n` skills with the most invocations in `window`, descending; ties break by name. */
export function topSkills(
  stats: SkillInvocationStats[],
  n: number,
  window: UsageWindow = "30d",
): SkillInvocationStats[] {
  return [...stats]
    .sort(
      (a, b) =>
        invocationsInWindow(b, window) - invocationsInWindow(a, window) ||
        a.skill.localeCompare(b.skill),
    )
    .slice(0, n);
}
