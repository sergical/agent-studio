// ============================================================================
// StatCards - Skills / SKILL.md tokens / folder size / 30d invocations
// ============================================================================

import { Activity, FileText, HardDrive, Puzzle } from "lucide-react";
import { formatBytes, formatTokens } from "../../lib/skill-stats";
import type { SkillTotals } from "../../lib/skill-stats";

interface StatCardsProps {
  totals: SkillTotals;
}

/** The four headline numbers at the top of the dashboard. */
export function StatCards({ totals }: StatCardsProps) {
  const cards = [
    { icon: Puzzle, label: "Skills", value: totals.skillCount.toLocaleString() },
    { icon: FileText, label: "SKILL.md tokens", value: formatTokens(totals.tokens) },
    { icon: HardDrive, label: "Folder size", value: formatBytes(totals.bytes) },
    {
      icon: Activity,
      label: "Invocations (30d)",
      value: totals.invocationsLast30Days.toLocaleString(),
    },
  ];

  return (
    <div className="dashboard-stat-cards">
      {cards.map(({ icon: Icon, label, value }) => (
        <div key={label} className="dashboard-stat-card">
          <Icon size={16} className="dashboard-stat-card-icon" />
          <div className="dashboard-stat-card-body">
            <span className="dashboard-stat-card-value">{value}</span>
            <span className="dashboard-stat-card-label">{label}</span>
          </div>
        </div>
      ))}
    </div>
  );
}
