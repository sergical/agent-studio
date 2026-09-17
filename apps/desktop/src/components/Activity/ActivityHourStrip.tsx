// ============================================================================
// ActivityHourStrip - A day's use counts by local hour, as a bar strip with
// its peak hour called out above.
// ============================================================================

import { formatHour } from "@skill-studio/lib";
import { Muted } from "./ActivityParts";
import { PanelLabel } from "./ActivitySplits";

export function HourStrip({ byHour }: { byHour: number[] }) {
  const max = Math.max(...byHour);
  const peak = formatHour(byHour.indexOf(max));
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-baseline justify-between gap-3">
        <PanelLabel>Time of day</PanelLabel>
        <Muted>Most around {peak}</Muted>
      </div>
      <div className="pointer-events-none flex h-8 items-end gap-px" aria-hidden="true">
        {byHour.map((n, hour) => (
          <span
            key={hour}
            className="flex-1 rounded-[1px] bg-text-tertiary"
            style={{
              height: n === 0 ? 1 : `${Math.max(12, (n / max) * 100)}%`,
              opacity: n === 0 ? 0.35 : 1,
            }}
          />
        ))}
      </div>
      <div className="grid grid-cols-4 text-caption text-text-quaternary" aria-hidden="true">
        <span>12 AM</span>
        <span>6 AM</span>
        <span>12 PM</span>
        <span>6 PM</span>
      </div>
    </div>
  );
}
