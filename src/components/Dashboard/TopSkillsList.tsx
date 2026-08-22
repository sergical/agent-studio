// ============================================================================
// TopSkillsList - Top 10 skills by invocations in the last 30 days
// ============================================================================

import { topSkills } from "../../lib/skill-stats";
import type { SkillInvocationStats } from "../../lib/skill-types";

interface TopSkillsListProps {
  stats: SkillInvocationStats[];
  onSelectSkill: (name: string) => void;
}

function formatLastUsed(lastUsed: string | undefined): string {
  if (!lastUsed) return "never";
  try {
    return new Date(lastUsed).toLocaleDateString();
  } catch {
    return lastUsed;
  }
}

/** The 10 most-invoked skills in the last 30 days, with total and last-used. */
export function TopSkillsList({ stats, onSelectSkill }: TopSkillsListProps) {
  const top = topSkills(stats, 10).filter((s) => s.last_30_days > 0);

  if (top.length === 0) {
    return <p className="dashboard-top-skills-empty">No invocations recorded yet</p>;
  }

  return (
    <ol className="dashboard-top-skills">
      {top.map((stat) => (
        <li key={stat.skill}>
          <button className="dashboard-top-skills-row" onClick={() => onSelectSkill(stat.skill)}>
            <span className="dashboard-top-skills-name">{stat.skill}</span>
            <span className="dashboard-top-skills-meta">
              {stat.last_30_days} / {stat.total} · last used {formatLastUsed(stat.last_used)}
            </span>
          </button>
        </li>
      ))}
    </ol>
  );
}
