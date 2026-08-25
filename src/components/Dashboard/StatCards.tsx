// ============================================================================
// StatCards - Your skills / plugin skills / 30d invocations / issues
// ============================================================================

import { Activity, AlertTriangle, Blocks, Puzzle } from "lucide-react";

interface StatCardsProps {
  ownCount: number;
  pluginCount: number;
  invocations30d: number;
  issuesCount: number;
}

/** The four headline numbers at the top of the dashboard, one thin row. */
export function StatCards({ ownCount, pluginCount, invocations30d, issuesCount }: StatCardsProps) {
  const cards = [
    { icon: Puzzle, label: "Your skills", value: ownCount.toLocaleString() },
    { icon: Blocks, label: "Plugin skills", value: pluginCount.toLocaleString() },
    { icon: Activity, label: "Invocations (30d)", value: invocations30d.toLocaleString() },
    { icon: AlertTriangle, label: "Issues", value: issuesCount.toLocaleString() },
  ];

  return (
    <div className="dashboard-stat-cards">
      {cards.map(({ icon: Icon, label, value }) => (
        <div key={label} className="dashboard-stat-card">
          <Icon size={14} className="dashboard-stat-card-icon" />
          <div className="dashboard-stat-card-body">
            <span className="dashboard-stat-card-value">{value}</span>
            <span className="dashboard-stat-card-label">{label}</span>
          </div>
        </div>
      ))}
    </div>
  );
}
