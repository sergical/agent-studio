// ============================================================================
// TopSkillsList - Top 8 skills by invocations in the last 30 days
// ============================================================================

import { pluginLabelForSkill } from "../../lib/skill-plugin-partition";
import { formatRelativeTime, topSkills } from "../../lib/skill-stats";
import type { InstalledSkill, SkillInvocationStats } from "../../lib/skill-types";

const MAX_SHOWN = 10;

interface TopSkillsListProps {
  /** Every skill (own and plugin), so plugin skills can still show up here tagged. */
  skills: InstalledSkill[];
  stats: SkillInvocationStats[];
  onSelectSkill: (name: string) => void;
}

/** The 8 most-invoked skills in the last 30 days, with a proportional bar. */
export function TopSkillsList({ skills, stats, onSelectSkill }: TopSkillsListProps) {
  const top = topSkills(stats, MAX_SHOWN).filter((s) => s.last_30_days > 0);
  const skillsByName = new Map(skills.map((s) => [s.name, s]));
  const max = Math.max(1, ...top.map((s) => s.last_30_days));

  if (top.length === 0) {
    return <p className="dashboard-top-skills-empty">No invocations recorded yet</p>;
  }

  return (
    <ol className="dashboard-top-skills">
      {top.map((stat) => {
        const skill = skillsByName.get(stat.skill);
        const pluginLabel = skill && pluginLabelForSkill(skill);
        return (
          <li key={stat.skill}>
            <button className="dashboard-top-skills-row" onClick={() => onSelectSkill(stat.skill)}>
              <span className="dashboard-top-skills-name-group">
                <span className="dashboard-top-skills-name">{stat.skill}</span>
                {pluginLabel && (
                  <span className="dashboard-top-skills-plugin-tag">{pluginLabel}</span>
                )}
              </span>
              <span className="dashboard-top-skills-bar-track">
                <span
                  className="dashboard-top-skills-bar-fill"
                  style={{ width: `${(stat.last_30_days / max) * 100}%` }}
                />
              </span>
              <span className="dashboard-top-skills-last-used">
                {stat.last_used ? formatRelativeTime(stat.last_used) : "never"}
              </span>
              <span className="dashboard-top-skills-count">{stat.last_30_days}</span>
            </button>
          </li>
        );
      })}
    </ol>
  );
}
