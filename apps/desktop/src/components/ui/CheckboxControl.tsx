// ============================================================================
// CheckboxControl - Base UI Checkbox wrapper: a 16 px box, accent fill when
// checked, indeterminate support. Pass `ariaLabel` for a standalone checkbox,
// or wrap it in a caller's own <label> and omit both.
// Built on the Base UI primitive directly rather than the kit's Checkbox: the
// kit always renders a fixed check icon and ignores `indeterminate`, so it
// can't show the Minus glyph this control needs.
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
      // `checkbox-control-root` is kept as a bare hook (no rules of its own
      // beyond the toolbar hit-area extension in App.css): the Skills table's
      // toolbar checkbox targets it via a nested `::before` selector.
      className="checkbox-control-root relative inline-flex size-4 shrink-0 items-center justify-center rounded-xs border border-border-strong bg-transparent transition-colors not-data-[disabled]:hover:border-text-tertiary focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-border-strong data-checked:border-accent data-checked:bg-accent data-indeterminate:border-accent data-indeterminate:bg-accent data-disabled:cursor-not-allowed data-disabled:opacity-50"
      checked={checked}
      onCheckedChange={onCheckedChange}
      indeterminate={indeterminate}
      disabled={disabled}
      aria-label={ariaLabel}
    >
      <Checkbox.Indicator className="inline-flex text-text-on-accent" keepMounted={false}>
        {indeterminate ? <Minus size={12} /> : <Check size={12} />}
      </Checkbox.Indicator>
    </Checkbox.Root>
  );
}
