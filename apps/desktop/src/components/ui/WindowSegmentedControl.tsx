// ============================================================================
// WindowSegmentedControl - Four-button toggle for the shared usage window
// ("24h" / "7d" / "14d" / "30d"), used by the dashboard's Top skills list and
// the Activity page's By skill table
// ============================================================================

import { USAGE_WINDOWS } from "../../lib/skill-stats";
import type { UsageWindow } from "../../lib/skill-stats";

interface WindowSegmentedControlProps {
  value: UsageWindow;
  onChange: (window: UsageWindow) => void;
}

export function WindowSegmentedControl({ value, onChange }: WindowSegmentedControlProps) {
  return (
    <div className="window-segmented-control">
      {USAGE_WINDOWS.map(({ id }) => (
        <button
          key={id}
          type="button"
          aria-pressed={value === id}
          className={`window-segmented-control-item ${value === id ? "active" : ""}`}
          onClick={() => onChange(id)}
        >
          {id}
        </button>
      ))}
    </div>
  );
}
