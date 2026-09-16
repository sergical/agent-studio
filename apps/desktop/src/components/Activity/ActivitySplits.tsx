// ============================================================================
// ActivitySplits - The harness/trigger split bars shared by a day's details
// and the usage-window overview: a label above ranked bars, each with a
// share-of-max width.
// ============================================================================

import type { ReactNode } from "react";
import { deploymentLabelFromAgentId, formatCount, ranked, TRIGGERS } from "@skill-studio/lib";
import type { ActivityFilter, AgentId, Breakdown } from "@skill-studio/lib";
import { HarnessIcon } from "../ui/HarnessIcon";
import { Muted } from "./ActivityParts";

export function PanelLabel({ children }: { children: ReactNode }) {
  return <h3 className="text-caption font-medium text-text-tertiary">{children}</h3>;
}

function BarRow({
  icon,
  label,
  value,
  max,
}: {
  icon?: ReactNode;
  label: string;
  value: number;
  max: number;
}) {
  return (
    <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1">
      <span className="flex min-w-0 items-center gap-2 text-body text-text-secondary">
        {icon}
        <span className="truncate">{label}</span>
      </span>
      <Muted>{formatCount(value)}</Muted>
      <div
        className="col-span-2 h-1 overflow-hidden rounded-full bg-bg-tertiary"
        aria-hidden="true"
      >
        <div
          className="h-full rounded-full bg-text-tertiary"
          style={{ width: `${max > 0 ? (value / max) * 100 : 0}%` }}
        />
      </div>
    </div>
  );
}

/** Harness and trigger bars; a split the filter already pins is left out. */
export function Splits({
  b,
  filter,
}: {
  b: Pick<Breakdown, "byHarness" | "byTrigger">;
  filter: ActivityFilter;
}) {
  const harnesses = ranked(b.byHarness);
  const triggers: [(typeof TRIGGERS)[number], number][] = [];
  for (const t of TRIGGERS) {
    const n = b.byTrigger[t.id] ?? 0;
    if (n > 0) triggers.push([t, n]);
  }
  const tMax = Math.max(0, ...triggers.map(([, n]) => n));
  return (
    <>
      {filter.harness === "all" && harnesses.length > 0 && (
        <div className="flex flex-col gap-2.5">
          <PanelLabel>Harness</PanelLabel>
          {harnesses.map(([id, n]) => (
            <BarRow
              key={id}
              // SAFETY: harness ids come off the wire as plain strings; every one this page can
              // show is a known AgentId.
              icon={<HarnessIcon harness={id as AgentId} size={14} />}
              label={deploymentLabelFromAgentId(id)}
              value={n}
              max={harnesses[0][1]}
            />
          ))}
        </div>
      )}
      {filter.trigger === "all" && triggers.length > 0 && (
        <div className="flex flex-col gap-2.5">
          <PanelLabel>Trigger</PanelLabel>
          {triggers.map(([t, n]) => (
            <BarRow key={t.id} label={t.label} value={n} max={tMax} />
          ))}
        </div>
      )}
    </>
  );
}
