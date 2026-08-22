// ============================================================================
// InvocationHeatmap - GitHub-style 52x7 grid of daily skill invocations
// ============================================================================

import type { InvocationHeatmap as InvocationHeatmapData } from "../../lib/skill-types";

const WEEKS = 52;
const DAYS_PER_WEEK = 7;

function toDateKey(date: Date): string {
  return date.toISOString().slice(0, 10);
}

/** Buckets a count into one of 5 intensity levels (0 = no activity). */
function intensityLevel(count: number, max: number): number {
  if (count <= 0 || max <= 0) return 0;
  const ratio = count / max;
  if (ratio > 0.75) return 4;
  if (ratio > 0.5) return 3;
  if (ratio > 0.25) return 2;
  return 1;
}

interface InvocationHeatmapProps {
  heatmap: InvocationHeatmapData;
}

/**
 * 52-week x 7-day grid of invocation counts, oldest week first. Each cell's
 * fill intensity is relative to the busiest day in the window; hover shows
 * the exact date and count.
 */
export function InvocationHeatmap({ heatmap }: InvocationHeatmapProps) {
  const totalDays = WEEKS * DAYS_PER_WEEK;
  const today = new Date();
  today.setHours(0, 0, 0, 0);

  const days: { date: string; count: number }[] = [];
  for (let i = totalDays - 1; i >= 0; i--) {
    const date = new Date(today);
    date.setDate(date.getDate() - i);
    const key = toDateKey(date);
    days.push({ date: key, count: heatmap.days[key] ?? 0 });
  }

  const max = Math.max(0, ...days.map((d) => d.count));
  const weeks: { date: string; count: number }[][] = [];
  for (let w = 0; w < WEEKS; w++) {
    weeks.push(days.slice(w * DAYS_PER_WEEK, (w + 1) * DAYS_PER_WEEK));
  }

  return (
    <div className="dashboard-heatmap">
      {weeks.map((week, wi) => (
        <div key={wi} className="dashboard-heatmap-week">
          {week.map(({ date, count }) => (
            <div
              key={date}
              className={`dashboard-heatmap-day level-${intensityLevel(count, max)}`}
              title={`${date}: ${count} invocation${count === 1 ? "" : "s"}`}
            />
          ))}
        </div>
      ))}
    </div>
  );
}
