// ============================================================================
// ActivityProjects - "By project, 30 days" section: every project with a use
// in the last 30 days, under the page's harness/trigger filter.
// ============================================================================

import { basename, formatCount, projectRows } from "@skill-studio/lib";
import type { ActivityFilter, SkillInvocationStats } from "@skill-studio/lib";
import { TooltipControl } from "../ui/TooltipControl";
import { CappedList, SectionHeader } from "./ActivityParts";

export function ActivityProjects({
  stats,
  filter,
  now,
}: {
  stats: SkillInvocationStats[];
  filter: ActivityFilter;
  now: Date;
}) {
  const rows = projectRows(stats, filter, now);
  return (
    <section className="flex flex-col gap-3">
      <SectionHeader title="By project, 30 days" />
      {rows.length === 0 ? (
        <p className="text-body text-text-tertiary">
          {filter.harness !== "all" || filter.trigger !== "all"
            ? "No uses in the last 30 days with these filters."
            : "No uses in the last 30 days."}
        </p>
      ) : (
        <CappedList
          items={rows}
          render={({ project, count }) => (
            <TooltipControl key={project} content={[{ text: project, mono: true }]}>
              <div className="flex h-8 items-center justify-between gap-3 border-b border-border-subtle px-2">
                <span className="truncate text-body text-text-primary">{basename(project)}</span>
                <span className="text-body text-text-secondary tabular-nums">
                  {formatCount(count)}
                </span>
              </div>
            </TooltipControl>
          )}
        />
      )}
    </section>
  );
}
