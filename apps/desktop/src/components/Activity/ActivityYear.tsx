// ============================================================================
// ActivityYear - "Last year" section: the heatmap plus its total, wired to
// the shared day-card hover content. A narrow section shows only the most
// recent weeks, so the day cells never shrink below a readable size.
// ============================================================================

import { useLayoutEffect, useRef, useState } from "react";
import { recentWeeks, uses } from "@skill-studio/lib";
import type { ActivityFilter, SkillInvocationStats } from "@skill-studio/lib";
import { ActivityDayCard } from "./ActivityDayCard";
import { ActivityHeatmap } from "./ActivityHeatmap";
import { Muted, SectionHeader } from "./ActivityParts";

const MIN_CELL_PX = 10;
// Matches the heatmap's `gap-[3px]`.
const CELL_GAP_PX = 3;
// The heatmap's weekday label column (`w-7`) plus its `gap-1.5`.
const DAY_LABELS_PX = 34;

/** How many week columns fit `width` with cells at `MIN_CELL_PX` or wider. */
function weeksThatFit(width: number | null): number {
  if (!width) return Number.POSITIVE_INFINITY;
  return Math.floor((width - DAY_LABELS_PX + CELL_GAP_PX) / (MIN_CELL_PX + CELL_GAP_PX));
}

function useWidth() {
  const ref = useRef<HTMLElement>(null);
  const [width, setWidth] = useState<number | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    // Zero while the page sits hidden behind another view; keep the last real width.
    const update = (next: number) => {
      if (next > 0) setWidth(next);
    };
    // Measured here too, not only in the observer, so the first paint already has the fitted grid.
    update(el.getBoundingClientRect().width);
    const observer = new ResizeObserver(([entry]) => update(entry.contentRect.width));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  return [ref, width] as const;
}

export function ActivityYear({
  stats,
  dates,
  days,
  yearTotal,
  filter,
  selected,
  onSelect,
}: {
  stats: SkillInvocationStats[];
  dates: string[];
  days: Record<string, number>;
  yearTotal: number;
  filter: ActivityFilter;
  selected: string | null;
  onSelect: (dayKey: string | null) => void;
}) {
  const [ref, width] = useWidth();
  const shown = recentWeeks(dates, weeksThatFit(width));
  const trimmed = shown.length < dates.length;
  const total = trimmed ? shown.reduce((sum, key) => sum + (days[key] ?? 0), 0) : yearTotal;

  return (
    <section ref={ref} className="flex min-w-0 flex-col gap-3">
      {/* A trimmed range starts on a Monday, so its length divides into weeks directly. */}
      <SectionHeader title={trimmed ? `Last ${Math.ceil(shown.length / 7)} weeks` : "Last year"}>
        <Muted>{uses(total)}</Muted>
      </SectionHeader>
      <ActivityHeatmap
        dates={shown}
        days={days}
        label="Uses per day. Enter opens the day's details"
        cardWidth={248}
        selected={selected}
        onSelect={onSelect}
        renderCard={(key) => (
          <ActivityDayCard
            stats={stats}
            dayKey={key}
            filter={filter}
            hint={selected === key ? "Click to close" : "Click for all details"}
          />
        )}
      />
    </section>
  );
}
