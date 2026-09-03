// ============================================================================
// WindowSegmentedControl - Four-button toggle for the shared usage window
// ("24h" / "7d" / "14d" / "30d"), used by the dashboard's Top skills list and
// the Activity page's By skill table
// ============================================================================

import { ToggleGroup, ToggleGroupItem } from "@skill-studio/ui";
import { USAGE_WINDOWS } from "@skill-studio/lib";
import type { UsageWindow } from "@skill-studio/lib";
import { singleSelectToggleValue } from "../../lib/single-select-toggle-group";

interface WindowSegmentedControlProps {
  value: UsageWindow;
  onChange: (window: UsageWindow) => void;
}

export function WindowSegmentedControl({ value, onChange }: WindowSegmentedControlProps) {
  return (
    <ToggleGroup
      variant="segmented"
      value={[value]}
      onValueChange={(next) => singleSelectToggleValue<UsageWindow>(next, onChange)}
    >
      {USAGE_WINDOWS.map(({ id }) => (
        <ToggleGroupItem key={id} value={id}>
          {id}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}
