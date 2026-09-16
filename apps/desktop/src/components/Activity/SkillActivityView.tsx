// ============================================================================
// SkillActivityView - Docked layout: a year heatmap, by-skill and by-project
// lists on the left; a right-hand panel that is always there, showing the
// usage window's overview until a day is picked, then that day's details.
// ============================================================================

import type { SkillSnapshot } from "@skill-studio/lib";
import { ActivityBySkill } from "./ActivityBySkill";
import { ActivityDayDetails } from "./ActivityDayDetails";
import { ActivityFilters, useActivityLens } from "./ActivityFilters";
import { ActivityOverview } from "./ActivityOverview";
import { ActivityProjects } from "./ActivityProjects";
import { ActivityYear } from "./ActivityYear";
import { SkillHistorySection } from "./SkillHistorySection";
import { useActivityEscape } from "./useActivityEscape";
import { PageShell } from "../Shell/PageShell";
import { useAppStore } from "../../store/appStore";

interface SkillActivityViewProps {
  snapshot: SkillSnapshot | undefined;
  onSelectSkill: (name: string) => void;
}

/**
 * Full activity history for own and plugin skills, from every harness with a
 * usage reader that's turned on in Settings: a year-long heatmap, a per-skill
 * table filtered to the shared usage window, a 30-day per-project breakdown,
 * and a docked details panel for the day currently picked.
 */
export function SkillActivityView({ snapshot, onSelectSkill }: SkillActivityViewProps) {
  const day = useAppStore((state) => state.activityDay);
  const setDay = useAppStore((state) => state.setActivityDay);
  useActivityEscape(() => setDay(null));

  const stats = snapshot?.invocations ?? [];
  // The snapshot's scan time, not the render time, so a stale snapshot doesn't make the window
  // and the heatmap disagree about "now".
  const now = snapshot?.scanned_at ? new Date(snapshot.scanned_at) : new Date();
  const lens = useActivityLens(stats, now);
  const hasAnyInvocations = snapshot ? snapshot.invocations.some((s) => s.total > 0) : false;

  return (
    <PageShell
      title="Activity"
      subtitle="From every harness turned on in Settings"
      toolbar={
        <ActivityFilters
          stats={stats}
          harness={lens.harness}
          setHarness={lens.setHarness}
          trigger={lens.trigger}
          setTrigger={lens.setTrigger}
          clearFilter={lens.clearFilter}
          enabledHarnesses={lens.enabledHarnesses}
        />
      }
    >
      {!snapshot || !hasAnyInvocations ? (
        <>
          <p className="text-wrap-pretty text-body text-text-tertiary">
            No skill uses recorded yet.
          </p>
          <SkillHistorySection />
        </>
      ) : (
        <div className="@container">
          {/* Narrow: one column, the details panel between the heatmap and the lists. Wide: the
              panel spans both rows of the right column, so its grid area (the sticky containing
              block) reaches the page bottom. The `1fr` row takes any extra height the panel
              needs, so the heatmap row never stretches. */}
          <div className="grid grid-cols-1 gap-x-6 gap-y-5 @[52rem]:grid-cols-[minmax(0,1fr)_18rem] @[52rem]:grid-rows-[auto_1fr]">
            <ActivityYear
              stats={stats}
              dates={lens.dates}
              days={lens.days}
              yearTotal={lens.yearTotal}
              filter={lens.filter}
              selected={day}
              onSelect={setDay}
            />
            <aside
              aria-label="Details"
              // 11rem covers the window's top and bottom insets, the page header and toolbar rows,
              // the sticky offset, and the page's bottom padding. Any taller and the grid's bottom
              // edge pushes the aside up past the sticky offset when scrolled to the end.
              className="flex flex-col gap-5 self-start rounded-md border border-border-subtle bg-bg-secondary p-4 @[52rem]:sticky @[52rem]:top-5 @[52rem]:col-start-2 @[52rem]:row-span-2 @[52rem]:row-start-1 @[52rem]:max-h-[calc(100dvh-11rem)] @[52rem]:overflow-y-auto"
            >
              {day ? (
                <ActivityDayDetails
                  stats={stats}
                  dayKey={day}
                  filter={lens.filter}
                  days={lens.days}
                  dates={lens.dates}
                  onChangeDay={setDay}
                  onOpenSkill={onSelectSkill}
                />
              ) : (
                <ActivityOverview stats={stats} filter={lens.filter} now={now} onOpenDay={setDay} />
              )}
            </aside>
            <div className="flex min-w-0 flex-col gap-5 @[52rem]:col-start-1 @[52rem]:row-start-2">
              <ActivityBySkill
                stats={stats}
                filter={lens.filter}
                now={now}
                onOpenSkill={onSelectSkill}
              />
              <ActivityProjects stats={stats} filter={lens.filter} now={now} />
              <SkillHistorySection />
            </div>
          </div>
        </div>
      )}
    </PageShell>
  );
}
