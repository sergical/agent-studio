// ============================================================================
// InvocationHeatmap - GitHub-style 52x7 grid of daily skill invocations
// ============================================================================

import type { InvocationHeatmap as InvocationHeatmapData } from "@skill-studio/lib";

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

function formatCellDate(date: Date): string {
  return `${MONTH_NAMES[date.getUTCMonth()]} ${date.getUTCDate()}`;
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
  /** Inclusive UTC date keys ("YYYY-MM-DD"), oldest first - from `heatmapDateRangeUtc`. */
  dates: string[];
}

/**
 * 52-week x 7-day grid of invocation counts, oldest week first, over the
 * exact `dates` range the caller passes in (so its total agrees with the
 * header and aria-label, which sum over the same list). Each cell's fill
 * intensity is relative to the busiest day in the window; hover shows the
 * exact date and count. Month labels sit above the first week of each month;
 * weekday labels sit in a gutter to the left.
 */
export function InvocationHeatmap({ heatmap, dates }: InvocationHeatmapProps) {
  const days = dates.map((key) => ({
    date: new Date(`${key}T00:00:00Z`),
    key,
    count: heatmap.days[key] ?? 0,
  }));
  const weekCount = days.length / DAYS_PER_WEEK;

  const max = Math.max(0, ...days.map((d) => d.count));

  const weeks: (typeof days)[number][][] = [];
  for (let w = 0; w < weekCount; w++) {
    weeks.push(days.slice(w * DAYS_PER_WEEK, (w + 1) * DAYS_PER_WEEK));
  }

  // One label per month, placed at the week that first shows that month and
  // spanning to the next month's first week (or the grid's end, for the
  // last month), so the label's width tracks how many weeks it covers.
  const monthStarts: { weekIndex: number; label: string }[] = [];
  let lastMonth = -1;
  weeks.forEach((week, weekIndex) => {
    const month = week[0].date.getUTCMonth();
    if (month !== lastMonth) {
      monthStarts.push({ weekIndex, label: MONTH_NAMES[month] });
      lastMonth = month;
    }
  });
  const monthLabels = monthStarts.map(({ weekIndex, label }, i) => ({
    weekIndex,
    label,
    span: (monthStarts[i + 1]?.weekIndex ?? weekCount) - weekIndex,
  }));

  const invocationsThisYear = days.reduce((sum, d) => sum + d.count, 0);

  return (
    <div>
      <div
        className="ml-7 grid grid-cols-52 gap-[3px] text-small text-text-tertiary"
        aria-hidden="true"
      >
        {monthLabels.map(({ weekIndex, label, span }) => (
          <span
            key={`${weekIndex}-${label}`}
            className="overflow-hidden whitespace-nowrap"
            style={{ gridColumnStart: weekIndex + 1, gridColumnEnd: `span ${span}` }}
          >
            {label}
          </span>
        ))}
      </div>
      <div className="flex gap-1.5">
        <div
          className="flex w-7 shrink-0 flex-col justify-between py-0.5 text-caption text-text-tertiary"
          aria-hidden="true"
        >
          {WEEKDAY_LABELS.map((label, i) => (
            <span key={i}>{label}</span>
          ))}
        </div>
        <div
          className="grid flex-1 grid-cols-52 grid-rows-7 grid-flow-col gap-[3px]"
          role="img"
          aria-label={`Invocations per day over the last year, ${invocationsThisYear} total`}
        >
          {days.map(({ date, key, count }) => {
            const level = intensityLevel(count, max);
            return (
              <div
                key={key}
                className="aspect-square rounded-[2px] border-0 bg-bg-tertiary"
                style={
                  level > 0
                    ? {
                        background: `color-mix(in oklch, var(--color-accent) ${level * 25}%, var(--color-bg-tertiary))`,
                      }
                    : undefined
                }
                title={
                  count > 0
                    ? `${count} invocation${count === 1 ? "" : "s"} · ${formatCellDate(date)}`
                    : `No invocations · ${formatCellDate(date)}`
                }
                aria-hidden="true"
              />
            );
          })}
        </div>
      </div>
    </div>
  );
}
