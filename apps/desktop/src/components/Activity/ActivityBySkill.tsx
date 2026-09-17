// ============================================================================
// ActivityBySkill - "By skill" section: every skill with a use in the shared
// usage window, under the page's harness/trigger filter.
// ============================================================================

import {
  deploymentLabelFromAgentId,
  formatCount,
  formatRelativeTime,
  ranked,
  skillRows,
  triggerLabel,
} from "@skill-studio/lib";
import type { ActivityFilter, SkillInvocationStats, SkillRow } from "@skill-studio/lib";
import { Button } from "@skill-studio/ui";
import { useAppStore } from "../../store/appStore";
import { TooltipControl } from "../ui/TooltipControl";
import { WindowSegmentedControl } from "../ui/WindowSegmentedControl";
import { CappedList, SectionHeader } from "./ActivityParts";

function rowTooltip(row: SkillRow): string[] {
  const harnesses = ranked(row.byHarness)
    .map(([id, n]) => `${deploymentLabelFromAgentId(id)} ${formatCount(n)}`)
    .join(" · ");
  const triggers = ranked(row.byTrigger)
    .map(([id, n]) => `${triggerLabel(id)} ${formatCount(n)}`)
    .join(" · ");
  return [harnesses, triggers];
}

export function ActivityBySkill({
  stats,
  filter,
  now,
  onOpenSkill,
}: {
  stats: SkillInvocationStats[];
  filter: ActivityFilter;
  now: Date;
  onOpenSkill: (name: string) => void;
}) {
  const usageWindow = useAppStore((state) => state.usageWindow);
  const setUsageWindow = useAppStore((state) => state.setUsageWindow);
  const rows = skillRows(stats, usageWindow, filter, now);
  const filtered = filter.harness !== "all" || filter.trigger !== "all";
  return (
    <section className="flex flex-col gap-3">
      <SectionHeader title="By skill">
        <WindowSegmentedControl value={usageWindow} onChange={setUsageWindow} />
      </SectionHeader>
      {rows.length === 0 ? (
        <p className="text-body text-text-tertiary">
          {filtered ? "No uses in this window with these filters." : "No uses in this window."}
        </p>
      ) : (
        <CappedList
          items={rows}
          render={(row) => (
            <TooltipControl key={row.skill} content={rowTooltip(row)}>
              <Button
                variant="ghost"
                className="grid h-8 w-full grid-cols-[minmax(0,1fr)_88px_64px] justify-start gap-3 rounded-none border-b-border-subtle px-2 text-left"
                onClick={() => onOpenSkill(row.skill)}
              >
                <span className="truncate text-body text-text-primary">{row.skill}</span>
                <span className="text-small text-nowrap text-text-tertiary tabular-nums">
                  {row.lastUsed ? formatRelativeTime(row.lastUsed, now) : "never"}
                </span>
                <span className="text-right text-body text-text-secondary tabular-nums">
                  {formatCount(row.count)}
                </span>
              </Button>
            </TooltipControl>
          )}
        />
      )}
    </section>
  );
}
