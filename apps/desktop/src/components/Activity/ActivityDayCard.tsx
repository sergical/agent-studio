// ============================================================================
// ActivityDayCard - One day's hover-card summary in the Activity heatmap:
// total, top skills, and (unless the harness filter already pins one) the
// harness split.
// ============================================================================

import { dayBreakdown, formatCount, formatDay, ranked, uses } from "@skill-studio/lib";
import type { ActivityFilter, AgentId } from "@skill-studio/lib";
import type { SkillInvocationStats } from "@skill-studio/lib";
import { HarnessIcon } from "../ui/HarnessIcon";
import { Muted } from "./ActivityParts";

interface ActivityDayCardProps {
  stats: SkillInvocationStats[];
  dayKey: string;
  filter: ActivityFilter;
  hint?: string;
}

export function ActivityDayCard({ stats, dayKey, filter, hint }: ActivityDayCardProps) {
  const day = dayBreakdown(stats, dayKey, filter);
  const skills = ranked(day.bySkill);
  const harnesses = ranked(day.byHarness);
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-baseline justify-between gap-3">
        <span className="font-medium text-text-primary">{formatDay(dayKey)}</span>
        <Muted>{day.total === 0 ? "No uses" : uses(day.total)}</Muted>
      </div>
      {skills.length > 0 && (
        <div className="flex flex-col gap-1 border-t border-border-subtle pt-2">
          {skills.slice(0, 3).map(([skill, n]) => (
            <div key={skill} className="flex items-baseline justify-between gap-3">
              <span className="truncate text-text-secondary">{skill}</span>
              <Muted>{formatCount(n)}</Muted>
            </div>
          ))}
          {skills.length > 3 && (
            <span className="text-text-tertiary">
              {skills.length - 3} more {skills.length - 3 === 1 ? "skill" : "skills"}
            </span>
          )}
        </div>
      )}
      {filter.harness === "all" && harnesses.length > 0 && (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-border-subtle pt-2">
          {harnesses.map(([id, n]) => {
            // SAFETY: harness ids come off the wire as plain strings; every one this page can
            // show is a known AgentId.
            const harnessId = id as AgentId;
            return (
              <span key={id} className="inline-flex items-center gap-1.5">
                <HarnessIcon harness={harnessId} size={12} />
                <Muted>{formatCount(n)}</Muted>
              </span>
            );
          })}
        </div>
      )}
      {hint && <span className="text-caption text-text-quaternary">{hint}</span>}
    </div>
  );
}
