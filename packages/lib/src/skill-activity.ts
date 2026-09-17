// ============================================================================
// Skill Studio - skill-activity
// Pure aggregation over SkillInvocationStats.by_hour for the Activity page's
// "Docked" layout: the year heatmap, a day's details, the window overview,
// per-skill and per-project rows, and the harness/trigger filter menus'
// counts. Every count on this page comes from the same flattened hourly
// buckets, so a day's splits always add up to its total and the totals
// always agree with the heatmap.
// ============================================================================

import type { SkillInvocationStats, SkillTrigger } from "./skill-types";
import { localDateKey } from "./skill-stats";
import type { UsageWindow } from "./skill-stats";

/** A harness id, or "all" for no harness filter. */
export type HarnessFilter = string | "all";
/** A `SkillTrigger`, or "all" for no trigger filter. */
export type TriggerFilter = SkillTrigger | "all";

export interface ActivityFilter {
  harness: HarnessFilter;
  trigger: TriggerFilter;
}

export const ALL_ACTIVITY: ActivityFilter = { harness: "all", trigger: "all" };

/** Labels and menu hints for each `SkillTrigger`, in display order. */
export const TRIGGERS: { id: SkillTrigger; label: string; hint: string }[] = [
  { id: "user", label: "Typed", hint: "You typed /name" },
  { id: "agent", label: "Called by the model", hint: "The model called the skill tool" },
  { id: "file_read", label: "SKILL.md read", hint: "The model opened the skill's SKILL.md" },
];

export function triggerLabel(id: SkillTrigger): string {
  return TRIGGERS.find((t) => t.id === id)?.label ?? id;
}

/** One `SkillUseHour` bucket, flattened out of its owning skill's stats and
 * carrying its local day/hour alongside the UTC hour it came from. */
interface FlatUse {
  skill: string;
  hour: number;
  harness: string;
  trigger: SkillTrigger;
  projectPath: string | null;
  count: number;
  /** Local calendar day, "YYYY-MM-DD". */
  dayKey: string;
  /** Local hour of day, 0-23. */
  localHour: number;
}

// Flattening is O(skills x hours); every filter/window change on the page
// otherwise re-walks the same nested `by_hour` arrays. Memoized on the
// invocations array reference, which is stable between renders of the same
// snapshot.
const flatCache = new WeakMap<SkillInvocationStats[], FlatUse[]>();

function flattenUses(stats: SkillInvocationStats[]): FlatUse[] {
  const cached = flatCache.get(stats);
  if (cached) return cached;
  const flat: FlatUse[] = [];
  for (const stat of stats) {
    for (const bucket of stat.by_hour) {
      // `Date`'s local getters convert the bucket's whole UTC hour to a wall-clock
      // day and hour. In a time zone with a half-hour (or 45-minute) UTC offset,
      // this can move a use into the neighboring local hour, and rarely the
      // neighboring day - accepted, since the source bucket is hour-grained.
      const at = new Date(bucket.hour * 3_600_000);
      flat.push({
        skill: stat.skill,
        hour: bucket.hour,
        harness: bucket.harness,
        trigger: bucket.trigger,
        projectPath: bucket.project_path ?? null,
        count: bucket.count,
        dayKey: localDateKey(at),
        localHour: at.getHours(),
      });
    }
  }
  flatCache.set(stats, flat);
  return flat;
}

function matchesFilter(u: FlatUse, filter: ActivityFilter): boolean {
  return (
    (filter.harness === "all" || u.harness === filter.harness) &&
    (filter.trigger === "all" || u.trigger === filter.trigger)
  );
}

/** Millisecond span of each `UsageWindow` from `USAGE_WINDOWS` in skill-stats.ts. */
const WINDOW_MS = {
  "24h": 24 * 3_600_000,
  "7d": 7 * 24 * 3_600_000,
  "14d": 14 * 24 * 3_600_000,
  "30d": 30 * 24 * 3_600_000,
} satisfies Record<UsageWindow, number>;

/** The whole-hour cutoff for `window`, relative to `now`: a bucket counts when its hour >= this. */
function windowCutoffHour(window: UsageWindow, now: Date): number {
  return Math.floor((now.getTime() - WINDOW_MS[window]) / 3_600_000);
}

export type Counts<K extends string> = Partial<Record<K, number>>;

/** Sorted non-zero entries, largest first. */
export function ranked<K extends string>(counts: Counts<K>): [K, number][] {
  // SAFETY: `Counts<K>` is `Partial<Record<K, number>>`; `Object.entries` only returns keys
  // actually present on `counts`, whose values are always `number` by that type.
  return (Object.entries(counts) as [K, number][])
    .filter(([, n]) => n > 0)
    .sort((a, b) => b[1] - a[1]);
}

function bump<K extends string>(counts: Counts<K>, key: K, n: number): void {
  counts[key] = (counts[key] ?? 0) + n;
}

/** Per-local-day use totals over the whole (365-day) window, for the heatmap. */
export function dayTotals(stats: SkillInvocationStats[], filter: ActivityFilter) {
  const out: Record<string, number> = {};
  for (const u of flattenUses(stats)) {
    if (!matchesFilter(u, filter)) continue;
    out[u.dayKey] = (out[u.dayKey] ?? 0) + u.count;
  }
  return out;
}

export interface Breakdown {
  total: number;
  bySkill: Counts<string>;
  byHarness: Counts<string>;
  byTrigger: Counts<SkillTrigger>;
}

/** Skill/harness/trigger totals for one local day. */
export function dayBreakdown(
  stats: SkillInvocationStats[],
  dayKey: string,
  filter: ActivityFilter,
): Breakdown {
  const out: Breakdown = { total: 0, bySkill: {}, byHarness: {}, byTrigger: {} };
  for (const u of flattenUses(stats)) {
    if (u.dayKey !== dayKey || !matchesFilter(u, filter)) continue;
    out.total += u.count;
    bump(out.bySkill, u.skill, u.count);
    bump(out.byHarness, u.harness, u.count);
    bump(out.byTrigger, u.trigger, u.count);
  }
  return out;
}

export interface DayDetail extends Breakdown {
  byProject: Counts<string>;
  /** Uses per local hour of the day, 0-23. */
  byHour: number[];
  /** Harnesses behind each skill's uses that day, busiest first. */
  skillHarnesses: Record<string, string[]>;
}

/** Everything `dayBreakdown` has, plus the project split, the hour strip, and per-skill harnesses. */
export function dayDetail(
  stats: SkillInvocationStats[],
  dayKey: string,
  filter: ActivityFilter,
): DayDetail {
  const breakdown = dayBreakdown(stats, dayKey, filter);
  const byProject: Counts<string> = {};
  const byHour = Array.from({ length: 24 }, () => 0);
  const perSkillHarness = new Map<string, Map<string, number>>();
  for (const u of flattenUses(stats)) {
    if (u.dayKey !== dayKey || !matchesFilter(u, filter)) continue;
    if (u.projectPath) bump(byProject, u.projectPath, u.count);
    byHour[u.localHour] += u.count;
    let harnessCounts = perSkillHarness.get(u.skill);
    if (!harnessCounts) {
      harnessCounts = new Map();
      perSkillHarness.set(u.skill, harnessCounts);
    }
    harnessCounts.set(u.harness, (harnessCounts.get(u.harness) ?? 0) + u.count);
  }
  const skillHarnesses: Record<string, string[]> = {};
  for (const [skill, harnessCounts] of perSkillHarness) {
    skillHarnesses[skill] = [...harnessCounts.entries()]
      .sort((a, b) => b[1] - a[1])
      .map(([id]) => id);
  }
  return { ...breakdown, byProject, byHour, skillHarnesses };
}

export interface WindowSummary extends Breakdown {
  busiest: { date: string; count: number } | null;
}

/** Totals, splits, and the busiest day over `window`, relative to `now`. */
export function windowSummary(
  stats: SkillInvocationStats[],
  window: UsageWindow,
  filter: ActivityFilter,
  now: Date,
): WindowSummary {
  const cutoffHour = windowCutoffHour(window, now);
  const out: WindowSummary = { total: 0, bySkill: {}, byHarness: {}, byTrigger: {}, busiest: null };
  const perDay: Counts<string> = {};
  for (const u of flattenUses(stats)) {
    if (u.hour < cutoffHour || !matchesFilter(u, filter)) continue;
    out.total += u.count;
    bump(out.bySkill, u.skill, u.count);
    bump(out.byHarness, u.harness, u.count);
    bump(out.byTrigger, u.trigger, u.count);
    bump(perDay, u.dayKey, u.count);
  }
  const [top] = ranked(perDay);
  out.busiest = top ? { date: top[0], count: top[1] } : null;
  return out;
}

export interface SkillRow {
  skill: string;
  count: number;
  lastUsed: string | null;
  byHarness: Counts<string>;
  byTrigger: Counts<SkillTrigger>;
}

/**
 * Per-skill totals over `window`, under `filter`. `lastUsed` ignores the
 * window: with no filter it's the skill's exact `last_used`; with a filter
 * it's the start of the latest bucket that still matches the filter.
 */
export function skillRows(
  stats: SkillInvocationStats[],
  window: UsageWindow,
  filter: ActivityFilter,
  now: Date,
): SkillRow[] {
  const cutoffHour = windowCutoffHour(window, now);
  const unfiltered = filter.harness === "all" && filter.trigger === "all";
  const rows = new Map<string, SkillRow>();
  const latestMatchingHour = new Map<string, number>();

  for (const u of flattenUses(stats)) {
    if (!matchesFilter(u, filter)) continue;
    const previous = latestMatchingHour.get(u.skill);
    if (previous === undefined || u.hour > previous) latestMatchingHour.set(u.skill, u.hour);
    if (u.hour < cutoffHour) continue;
    let row = rows.get(u.skill);
    if (!row) {
      row = { skill: u.skill, count: 0, lastUsed: null, byHarness: {}, byTrigger: {} };
      rows.set(u.skill, row);
    }
    row.count += u.count;
    bump(row.byHarness, u.harness, u.count);
    bump(row.byTrigger, u.trigger, u.count);
  }

  const lastUsedBySkill = new Map(stats.map((s) => [s.skill, s.last_used]));
  for (const row of rows.values()) {
    if (unfiltered) {
      row.lastUsed = lastUsedBySkill.get(row.skill) ?? null;
    } else {
      const hour = latestMatchingHour.get(row.skill);
      row.lastUsed = hour !== undefined ? new Date(hour * 3_600_000).toISOString() : null;
    }
  }

  return [...rows.values()].sort((a, b) => b.count - a.count || a.skill.localeCompare(b.skill));
}

/** Per-project use totals over the last 30 days, under `filter`. */
export function projectRows(
  stats: SkillInvocationStats[],
  filter: ActivityFilter,
  now: Date,
): { project: string; count: number }[] {
  const cutoffHour = windowCutoffHour("30d", now);
  const counts: Counts<string> = {};
  for (const u of flattenUses(stats)) {
    if (u.hour < cutoffHour || !u.projectPath || !matchesFilter(u, filter)) continue;
    bump(counts, u.projectPath, u.count);
  }
  return ranked(counts).map(([project, count]) => ({ project, count }));
}

/** Year totals per harness under a trigger filter - the harness menu's counts. */
export function yearByHarness(stats: SkillInvocationStats[], trigger: TriggerFilter) {
  const out: Counts<string> = {};
  for (const u of flattenUses(stats)) {
    if (trigger !== "all" && u.trigger !== trigger) continue;
    bump(out, u.harness, u.count);
  }
  return out;
}

/** Year totals per trigger under a harness filter - the trigger menu's counts. */
export function yearByTrigger(stats: SkillInvocationStats[], harness: HarnessFilter) {
  const out: Counts<SkillTrigger> = {};
  for (const u of flattenUses(stats)) {
    if (harness !== "all" && u.harness !== harness) continue;
    bump(out, u.trigger, u.count);
  }
  return out;
}

/** Median uses on days with any use. */
export function usualDay(days: Record<string, number>): number {
  const sorted = Object.values(days)
    .filter((n) => n > 0)
    .sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)] ?? 0;
}

/** The date `by` days away within `dates` (oldest-first day keys), or null past either end. */
export function shiftDay(dates: string[], dayKey: string, by: number): string | null {
  const index = dates.indexOf(dayKey);
  if (index === -1) return null;
  return dates[index + by] ?? null;
}

export function formatHour(hour: number): string {
  const h = hour % 12 === 0 ? 12 : hour % 12;
  return `${h} ${hour < 12 ? "AM" : "PM"}`;
}

const DAY_FORMAT = new Intl.DateTimeFormat("en-US", {
  weekday: "short",
  month: "short",
  day: "numeric",
});
const COUNT_FORMAT = new Intl.NumberFormat("en-US");

/** Formats a local "YYYY-MM-DD" day key as e.g. "Wed, Sep 16". */
export function formatDay(key: string): string {
  const [year, month, day] = key.split("-").map(Number);
  return DAY_FORMAT.format(new Date(year, month - 1, day));
}

export function formatCount(n: number): string {
  return COUNT_FORMAT.format(n);
}

export function uses(n: number): string {
  return `${formatCount(n)} ${n === 1 ? "use" : "uses"}`;
}
