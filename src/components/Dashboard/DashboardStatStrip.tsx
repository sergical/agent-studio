// ============================================================================
// DashboardStatStrip - Your skills / 30d invocations, with a trend sparkline
// ============================================================================

interface DashboardStatStripProps {
  ownCount: number;
  /** Invocation count for the currently selected usage window. */
  invocationsInWindow: number;
  /** e.g. "24 hours", "7 days" - shown after "Invocations, " in the second cell's label. */
  windowLabel: string;
  /** Invocation count per day for the last 30 days, oldest first; the sparkline always covers 30 days regardless of the selected window. */
  dailyCounts: number[];
}

/** Builds the sparkline's polyline points, normalized to a 0-28 y range. */
function sparklinePoints(dailyCounts: number[]): string {
  const max = Math.max(1, ...dailyCounts);
  const stepX = dailyCounts.length > 1 ? 120 / (dailyCounts.length - 1) : 0;
  return dailyCounts
    .map((count, i) => {
      const x = i * stepX;
      const y = 28 - (count / max) * 26 - 1;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

/**
 * Two headline numbers at the top of the dashboard - "Your skills" and
 * "Invocations, 30 days" - the latter followed inline by a 30-day sparkline.
 * No boxes, no icons, not interactive.
 */
export function DashboardStatStrip({
  ownCount,
  invocationsInWindow,
  windowLabel,
  dailyCounts,
}: DashboardStatStripProps) {
  const points = sparklinePoints(dailyCounts);
  const areaPath = `M0,28 L${points} L120,28 Z`;

  return (
    <div className="dashboard-stat-strip">
      <div className="dashboard-stat-cell">
        <span className="dashboard-stat-value">{ownCount.toLocaleString()}</span>
        <span className="dashboard-stat-label">Your skills</span>
      </div>
      <div className="dashboard-stat-cell dashboard-stat-cell-sparkline">
        <span className="dashboard-stat-value-group">
          <span className="dashboard-stat-value">{invocationsInWindow.toLocaleString()}</span>
          <svg
            className="dashboard-sparkline"
            viewBox="0 0 120 28"
            role="img"
            aria-label="Invocations per day, last 30 days"
          >
            <path d={areaPath} fill="var(--color-accent-softer)" stroke="none" />
            <polyline points={points} fill="none" stroke="var(--color-accent)" strokeWidth={1.5} />
          </svg>
        </span>
        <span className="dashboard-stat-label">Invocations, {windowLabel}</span>
      </div>
    </div>
  );
}
