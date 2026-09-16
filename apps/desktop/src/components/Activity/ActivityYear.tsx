// ============================================================================
// ActivityYear - "Last year" section: the heatmap plus its total, wired to
// the shared day-card hover content.
// ============================================================================

import { uses } from "@skill-studio/lib";
import type { ActivityFilter, SkillInvocationStats } from "@skill-studio/lib";
import { ActivityDayCard } from "./ActivityDayCard";
import { ActivityHeatmap } from "./ActivityHeatmap";
import { Muted, SectionHeader } from "./ActivityParts";

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
  return (
    <section className="flex flex-col gap-3">
      <SectionHeader title="Last year">
        <Muted>{uses(yearTotal)}</Muted>
      </SectionHeader>
      <ActivityHeatmap
        dates={dates}
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
