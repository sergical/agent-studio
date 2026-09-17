// ============================================================================
// ActivityOverview - The docked panel's default content: the usage window's
// totals, harness/trigger splits, and busiest day. Replaced by
// ActivityDayDetails once a day is picked.
// ============================================================================

import { formatDay, USAGE_WINDOWS, uses, windowSummary } from "@skill-studio/lib";
import type { ActivityFilter, SkillInvocationStats } from "@skill-studio/lib";
import { Button } from "@skill-studio/ui";
import { useAppStore } from "../../store/appStore";
import { Muted } from "./ActivityParts";
import { PanelLabel, Splits } from "./ActivitySplits";

export function ActivityOverview({
  stats,
  filter,
  now,
  onOpenDay,
}: {
  stats: SkillInvocationStats[];
  filter: ActivityFilter;
  now: Date;
  onOpenDay: (dayKey: string) => void;
}) {
  const usageWindow = useAppStore((state) => state.usageWindow);
  const windowLabel = USAGE_WINDOWS.find((w) => w.id === usageWindow)?.label ?? "30 days";
  const summary = windowSummary(stats, usageWindow, filter, now);
  return (
    <>
      <div className="flex flex-col gap-0.5">
        <h2 className="text-body font-medium text-text-primary">Last {windowLabel}</h2>
        <Muted>
          {summary.total === 0 ? `No uses in the last ${windowLabel}.` : uses(summary.total)}
        </Muted>
      </div>
      {summary.total > 0 && (
        <>
          <Splits b={summary} filter={filter} />
          {summary.busiest && (
            <div className="flex flex-col gap-1.5">
              <PanelLabel>Busiest day</PanelLabel>
              <Button
                variant="ghost"
                size="sm"
                className="-mx-2 justify-between px-2 font-normal"
                onClick={() => onOpenDay(summary.busiest!.date)}
              >
                <span className="text-text-secondary">{formatDay(summary.busiest.date)}</span>
                <Muted>{uses(summary.busiest.count)}</Muted>
              </Button>
            </div>
          )}
        </>
      )}
      <p className="text-small text-pretty text-text-quaternary">
        Click a day to see its details here.
      </p>
    </>
  );
}
