// ============================================================================
// WindowSegmentedControl - Four-button toggle for the shared usage window
// ("24h" / "7d" / "14d" / "30d"), used by the dashboard's Top skills list and
// the Activity page's By skill table
// ============================================================================

import { USAGE_WINDOWS } from "@skill-studio/lib";
import type { UsageWindow } from "@skill-studio/lib";

interface WindowSegmentedControlProps {
  value: UsageWindow;
  onChange: (window: UsageWindow) => void;
}

export function WindowSegmentedControl({ value, onChange }: WindowSegmentedControlProps) {
  return (
    <div className="flex overflow-hidden rounded-sm border border-border">
      {USAGE_WINDOWS.map(({ id }, i) => (
        <button
          key={id}
          type="button"
          aria-pressed={value === id}
          className={`h-6 px-2 text-caption transition-colors ${i > 0 ? "border-l border-border" : ""} ${
            value === id ? "bg-bg-tertiary text-text-primary" : "bg-transparent text-text-tertiary"
          }`}
          onClick={() => onChange(id)}
        >
          {id}
        </button>
      ))}
    </div>
  );
}
