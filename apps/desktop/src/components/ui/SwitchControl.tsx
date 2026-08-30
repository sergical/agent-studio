// ============================================================================
// SwitchControl - Base UI Switch wrapper: a 28x16 track with a 12 px thumb,
// accent fill when checked. Used for the Skills filter bar's "Show
// coverage" toggle.
// ============================================================================

import { Switch } from "@base-ui/react/switch";

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
    <Switch.Root
      className="switch-control-root"
      checked={checked}
      onCheckedChange={onCheckedChange}
      disabled={disabled}
      aria-label={ariaLabel}
    >
      <Switch.Thumb className="switch-control-thumb" />
    </Switch.Root>
  );
}
