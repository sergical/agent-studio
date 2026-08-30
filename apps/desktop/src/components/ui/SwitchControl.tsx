// ============================================================================
// SwitchControl - Kit Switch wrapper, sized "sm" (24x14 track, 12 px thumb)
// to match the app's compact control scale, accent fill when checked. Used
// for the Skills filter bar's "Show coverage" toggle.
// ============================================================================

import { Switch } from "@skill-studio/ui";

interface SwitchControlProps {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean;
  ariaLabel?: string;
}

export function SwitchControl({
  checked,
  onCheckedChange,
  disabled = false,
  ariaLabel,
}: SwitchControlProps) {
  return (
    <Switch
      size="sm"
      checked={checked}
      onCheckedChange={onCheckedChange}
      disabled={disabled}
      aria-label={ariaLabel}
      className="data-checked:bg-accent data-unchecked:border-border-strong data-unchecked:bg-bg-active"
    />
  );
}
