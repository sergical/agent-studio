// ============================================================================
// TopSkillsList - Top 8 skills by invocations in the last 30 days
// ============================================================================

import { pluginLabelForSkill } from "../../lib/skill-plugin-partition";
import { formatRelativeTime, invocationsInWindow, topSkills } from "../../lib/skill-stats";
import type { UsageWindow } from "../../lib/skill-stats";
import type { InstalledSkill, SkillInvocationStats } from "../../lib/skill-types";

const MAX_SHOWN = 10;

interface TopSkillsListProps {
  /** Every skill (own and plugin), so plugin skills can still show up here tagged. */
  skills: InstalledSkill[];
  stats: SkillInvocationStats[];
  window: UsageWindow;
  onSelectSkill: (name: string) => void;
}

/** The 10 most-invoked skills in the selected window. */
export function TopSkillsList({ skills, stats, window, onSelectSkill }: TopSkillsListProps) {
  const top = topSkills(stats, MAX_SHOWN, window).filter((s) => invocationsInWindow(s, window) > 0);
  const skillsByName = new Map(skills.map((s) => [s.name, s]));

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
              <span className="dashboard-top-skills-last-used">
                {stat.last_used ? formatRelativeTime(stat.last_used) : "never"}
              </span>
              <span className="dashboard-top-skills-count">
                {invocationsInWindow(stat, window)}
              </span>
            </button>
          </li>
        );
      })}
    </ol>
  );
}
