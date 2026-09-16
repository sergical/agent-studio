// ============================================================================
// ActivityFilters - The Activity page's harness and trigger filter menus, and
// the `useActivityLens` hook that owns their state plus the data every
// section derives from it. Only harnesses with a usage reader in core AND
// turned on in Settings' discovery switches are offered; if the selected
// harness gets turned off underneath the filter, it resets to "all".
// ============================================================================

import { useEffect, useState } from "react";
import { ChevronDown, Layers } from "lucide-react";
import {
  ALL_ACTIVITY,
  dayTotals,
  deploymentLabelFromAgentId,
  formatCount,
  heatmapDateRangeLocal,
  TRIGGERS,
  triggerLabel,
  yearByHarness,
  yearByTrigger,
} from "@skill-studio/lib";
import type {
  AgentId,
  HarnessFilter,
  SkillInvocationStats,
  TriggerFilter,
} from "@skill-studio/lib";
import { Button } from "@skill-studio/ui";
import { getDiscoverySources } from "../../lib/skill-api";
import { HarnessIcon } from "../ui/HarnessIcon";
import { MenuControl, MenuRadioGroup, MenuRadioItem, MenuSeparator } from "../ui/MenuControl";
import { useAppStore } from "../../store/appStore";

/** Harnesses with a usage reader in `crates/skill-studio-core/src/skill_uses/`, in menu order. */
const USAGE_READER_HARNESSES: AgentId[] = [
  "claude-code",
  "codex",
  "open-code",
  "pi",
  "cursor",
  "grok-build",
];

const FILTER_TRIGGER =
  "inline-flex h-(--control-height) cursor-pointer items-center gap-1.5 rounded-sm border border-border bg-transparent px-2.5 text-body text-text-secondary transition-colors hover:bg-bg-hover hover:text-text-primary data-popup-open:bg-bg-hover";
const ITEM = "pr-8";

/** Filter state, the harnesses turned on in Settings, and the data every section derives from them. */
export function useActivityLens(stats: SkillInvocationStats[], now: Date) {
  const filter = useAppStore((state) => state.activityFilter);
  const setFilter = useAppStore((state) => state.setActivityFilter);
  const { harness, trigger } = filter;
  const setHarness = (next: HarnessFilter) => setFilter({ harness: next, trigger });
  const setTrigger = (next: TriggerFilter) => setFilter({ harness, trigger: next });
  const clearFilter = () => setFilter(ALL_ACTIVITY);
  const [enabledHarnesses, setEnabledHarnesses] = useState<Set<string> | null>(null);

  useEffect(() => {
    let cancelled = false;
    getDiscoverySources()
      .then((sources) => {
        if (cancelled) return;
        const enabled = new Set<string>();
        for (const s of sources) if (s.enabled) enabled.add(s.harness);
        setEnabledHarnesses(enabled);
      })
      .catch(() => {
        // Settings couldn't be read - show every harness with a usage reader rather than none.
        if (!cancelled) setEnabledHarnesses(new Set(USAGE_READER_HARNESSES));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // If the selected harness gets turned off in Settings underneath this page, the filter can't
  // keep pointing at it. Goes through `setFilter` (stable, from the store) rather than
  // `setHarness` (a plain function, recreated every render) so this effect doesn't refire on
  // every unrelated render.
  useEffect(() => {
    if (harness !== "all" && enabledHarnesses && !enabledHarnesses.has(harness)) {
      setFilter({ harness: "all", trigger });
    }
  }, [harness, trigger, enabledHarnesses, setFilter]);

  const dates = heatmapDateRangeLocal(now).dates;
  const days = dayTotals(stats, filter);
  const yearTotal = Object.values(days).reduce((a, b) => a + b, 0);

  return {
    harness,
    setHarness,
    trigger,
    setTrigger,
    clearFilter,
    filter,
    dates,
    days,
    yearTotal,
    enabledHarnesses,
  };
}

export type ActivityLens = ReturnType<typeof useActivityLens>;

export function ActivityFilters({
  stats,
  harness,
  setHarness,
  trigger,
  setTrigger,
  clearFilter,
  enabledHarnesses,
}: Pick<
  ActivityLens,
  "harness" | "setHarness" | "trigger" | "setTrigger" | "clearFilter" | "enabledHarnesses"
> & {
  stats: SkillInvocationStats[];
}) {
  const harnesses = enabledHarnesses
    ? USAGE_READER_HARNESSES.filter((id) => enabledHarnesses.has(id))
    : [];
  const harnessCounts = yearByHarness(stats, trigger);
  const triggerCounts = yearByTrigger(stats, harness);
  const harnessTotal = Object.values(harnessCounts).reduce((a: number, b) => a + (b ?? 0), 0);
  const triggerTotal = Object.values(triggerCounts).reduce((a: number, b) => a + (b ?? 0), 0);
  const showClear = harness !== "all" || trigger !== "all";

  return (
    <>
      <MenuControl
        triggerClassName={FILTER_TRIGGER}
        triggerAriaLabel="Filter by harness"
        popupClassName="min-w-60"
        trigger={
          <>
            {harness === "all" ? (
              <Layers size={14} className="text-icon-muted" aria-hidden="true" />
            ) : (
              // SAFETY: HarnessFilter is AgentId | "all"; the "all" branch is handled above.
              <HarnessIcon harness={harness as AgentId} size={14} />
            )}
            {harness === "all" ? "All harnesses" : deploymentLabelFromAgentId(harness)}
            <ChevronDown size={12} className="text-icon-muted" aria-hidden="true" />
          </>
        }
      >
        <MenuRadioGroup
          value={harness}
          // SAFETY: MenuRadioItem values below are only "all" or a HarnessFilter member.
          onValueChange={(v) => setHarness(v as HarnessFilter)}
        >
          <MenuRadioItem value="all" closeOnClick className={ITEM}>
            <Layers size={14} className="text-icon-muted" aria-hidden="true" />
            All harnesses
            <span className="ml-auto pl-4 text-small text-text-tertiary tabular-nums">
              {formatCount(harnessTotal)}
            </span>
          </MenuRadioItem>
          <MenuSeparator />
          {harnesses.map((id) => {
            const n = harnessCounts[id] ?? 0;
            return (
              <MenuRadioItem key={id} value={id} disabled={n === 0} closeOnClick className={ITEM}>
                <HarnessIcon harness={id} size={14} muted={n === 0} />
                {deploymentLabelFromAgentId(id)}
                <span className="ml-auto pl-4 text-small text-text-tertiary tabular-nums">
                  {n === 0 ? "No uses" : formatCount(n)}
                </span>
              </MenuRadioItem>
            );
          })}
        </MenuRadioGroup>
      </MenuControl>
      <MenuControl
        triggerClassName={FILTER_TRIGGER}
        triggerAriaLabel="Filter by trigger"
        popupClassName="min-w-64"
        trigger={
          <>
            {trigger === "all" ? "Any trigger" : triggerLabel(trigger)}
            <ChevronDown size={12} className="text-icon-muted" aria-hidden="true" />
          </>
        }
      >
        <MenuRadioGroup
          value={trigger}
          // SAFETY: MenuRadioItem values below are only "all" or a TriggerFilter member.
          onValueChange={(v) => setTrigger(v as TriggerFilter)}
        >
          <MenuRadioItem value="all" closeOnClick className={ITEM}>
            Any trigger
            <span className="ml-auto pl-4 text-small text-text-tertiary tabular-nums">
              {formatCount(triggerTotal)}
            </span>
          </MenuRadioItem>
          <MenuSeparator />
          {TRIGGERS.map(({ id, label, hint }) => {
            const n = triggerCounts[id] ?? 0;
            return (
              <MenuRadioItem
                key={id}
                value={id}
                disabled={n === 0}
                closeOnClick
                className={`${ITEM} h-auto py-1.5`}
              >
                <span className="flex flex-col">
                  <span>{label}</span>
                  <span className="text-small text-text-tertiary">{hint}</span>
                </span>
                <span className="ml-auto self-start pl-4 text-small text-text-tertiary tabular-nums">
                  {n === 0 ? "No uses" : formatCount(n)}
                </span>
              </MenuRadioItem>
            );
          })}
        </MenuRadioGroup>
      </MenuControl>
      {showClear && (
        <Button variant="ghost" size="sm" className="text-text-tertiary" onClick={clearFilter}>
          Clear
        </Button>
      )}
    </>
  );
}
