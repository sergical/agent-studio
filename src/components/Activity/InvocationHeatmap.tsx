// ============================================================================
// InvocationHeatmap - GitHub-style 52x7 grid of daily skill invocations
// ============================================================================

import type { InvocationHeatmap as InvocationHeatmapData } from "../../lib/skill-types";

const WEEKS = 52;
const DAYS_PER_WEEK = 7;
const MONTH_NAMES = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];
const WEEKDAY_LABELS = ["Mon", "", "Wed", "", "Fri", "", ""];

function toDateKey(date: Date): string {
  return date.toISOString().slice(0, 10);
}

function formatCellDate(date: Date): string {
  return `${MONTH_NAMES[date.getMonth()]} ${date.getDate()}`;
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
 * the exact date and count. Month labels sit above the first week of each
 * month; weekday labels sit in a gutter to the left.
 */
export function InvocationHeatmap({ heatmap }: InvocationHeatmapProps) {
  const totalDays = WEEKS * DAYS_PER_WEEK;
  const today = new Date();
  today.setHours(0, 0, 0, 0);

  const days: { date: Date; key: string; count: number }[] = [];
  for (let i = totalDays - 1; i >= 0; i--) {
    const date = new Date(today);
    date.setDate(date.getDate() - i);
    const key = toDateKey(date);
    days.push({ date, key, count: heatmap.days[key] ?? 0 });
  }

  const max = Math.max(0, ...days.map((d) => d.count));

  const weeks: { date: Date; key: string; count: number }[][] = [];
  for (let w = 0; w < WEEKS; w++) {
    weeks.push(days.slice(w * DAYS_PER_WEEK, (w + 1) * DAYS_PER_WEEK));
  }

  // One label per month, placed at the week that first shows that month and
  // spanning to the next month's first week (or the grid's end, for the
  // last month), so the label's width tracks how many weeks it covers.
  const monthStarts: { weekIndex: number; label: string }[] = [];
  let lastMonth = -1;
  weeks.forEach((week, weekIndex) => {
    const month = week[0].date.getMonth();
    if (month !== lastMonth) {
      monthStarts.push({ weekIndex, label: MONTH_NAMES[month] });
      lastMonth = month;
    }
  });
  const monthLabels = monthStarts.map(({ weekIndex, label }, i) => ({
    weekIndex,
    label,
    span: (monthStarts[i + 1]?.weekIndex ?? WEEKS) - weekIndex,
  }));

  const invocationsThisYear = days.reduce((sum, d) => sum + d.count, 0);

  return (
    <div className="activity-heatmap-wrap">
      <div className="activity-heatmap-months" aria-hidden="true">
        {monthLabels.map(({ weekIndex, label, span }) => (
          <span
            key={`${weekIndex}-${label}`}
            className="activity-heatmap-month-label"
            style={{ gridColumnStart: weekIndex + 1, gridColumnEnd: `span ${span}` }}
          >
            {label}
          </span>
        ))}
      </div>
      <div className="activity-heatmap-body">
        <div className="activity-heatmap-weekdays" aria-hidden="true">
          {WEEKDAY_LABELS.map((label, i) => (
            <span key={i}>{label}</span>
          ))}
        </div>
        <div
          className="activity-heatmap-grid"
          role="img"
          aria-label={`Invocations per day over the last year, ${invocationsThisYear} total`}
        >
          {days.map(({ date, key, count }) => (
            <div
              key={key}
              className={`activity-heatmap-cell level-${intensityLevel(count, max)}`}
              title={
                count > 0
                  ? `${count} invocation${count === 1 ? "" : "s"} · ${formatCellDate(date)}`
                  : `No invocations · ${formatCellDate(date)}`
              }
              aria-hidden="true"
            />
          ))}
        </div>
      </div>
    </div>
  );
}
