// ============================================================================
// CheckboxControl - Base UI Checkbox wrapper: a 16 px box, accent fill when
// checked, indeterminate support. Pass `ariaLabel` for a standalone checkbox,
// or wrap it in a caller's own <label> and omit both.
// ============================================================================

import { Checkbox } from "@base-ui/react/checkbox";
import { Check, Minus } from "lucide-react";

interface CheckboxControlProps {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  indeterminate?: boolean;
  disabled?: boolean;
  ariaLabel?: string;
}

export function CheckboxControl({
  checked,
  onCheckedChange,
  indeterminate = false,
  disabled = false,
  ariaLabel,
}: CheckboxControlProps) {
  return (
    <Checkbox.Root
      className="checkbox-control-root"
      checked={checked}
      onCheckedChange={onCheckedChange}
      indeterminate={indeterminate}
      disabled={disabled}
      aria-label={ariaLabel}
    >
      <Checkbox.Indicator className="checkbox-control-indicator" keepMounted={false}>
        {indeterminate ? <Minus size={12} /> : <Check size={12} />}
      </Checkbox.Indicator>
    </Checkbox.Root>
  );
}
