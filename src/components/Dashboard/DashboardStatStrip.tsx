// ============================================================================
// DashboardStatStrip - Your skills / plugin skills / 30d invocations / issues
// ============================================================================

interface DashboardStatStripProps {
  ownCount: number;
  pluginCount: number;
  invocations30d: number;
  issuesCount: number;
}

/**
 * The four headline numbers at the top of the dashboard: one thin row of
 * cells separated by hairlines, no boxes, no icons, not interactive.
 */
export function DashboardStatStrip({
  ownCount,
  pluginCount,
  invocations30d,
  issuesCount,
}: DashboardStatStripProps) {
  const cells = [
    { label: "Your skills", value: ownCount },
    { label: "Plugin skills", value: pluginCount },
    { label: "Invocations, 30 days", value: invocations30d },
    { label: "Issues", value: issuesCount, warning: issuesCount > 0 },
  ];

  return (
    <div className="dashboard-stat-strip">
      {cells.map(({ label, value, warning }) => (
        <div key={label} className="dashboard-stat-cell">
          <span className={`dashboard-stat-value ${warning ? "warning" : ""}`}>
            {value.toLocaleString()}
          </span>
          <span className="dashboard-stat-label">{label}</span>
        </div>
      ))}
    </div>
  );
}
