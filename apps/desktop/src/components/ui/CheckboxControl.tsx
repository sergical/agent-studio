// ============================================================================
// CheckboxControl - thin wrapper over the kit's Checkbox: a 16 px box, accent
// fill when checked or indeterminate. Pass `ariaLabel` for a standalone
// checkbox, or wrap it in a caller's own <label> and omit both.
// ============================================================================

import { Checkbox } from "@skill-studio/ui";
import type { ComponentProps } from "react";

type CheckboxChangeEventDetails = Parameters<
  NonNullable<ComponentProps<typeof Checkbox>["onCheckedChange"]>
>[1];

interface CheckboxControlProps {
  checked: boolean;
  onCheckedChange: (checked: boolean, eventDetails: CheckboxChangeEventDetails) => void;
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
    <Checkbox
      // `checkbox-control-root` is kept as a bare hook with no rules of its
      // own: SkillListTable's toolbar checkbox reaches in via a descendant
      // `[&_.checkbox-control-root]:before:*` selector to extend its hit area.
      className="checkbox-control-root relative inline-flex size-4 shrink-0 items-center justify-center rounded-xs border border-border-strong bg-transparent transition-colors not-data-[disabled]:hover:border-text-tertiary focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-border-strong data-checked:border-accent data-checked:bg-accent data-indeterminate:border-accent data-indeterminate:bg-accent data-disabled:cursor-not-allowed data-disabled:opacity-50"
      checked={checked}
      onCheckedChange={onCheckedChange}
      indeterminate={indeterminate}
      disabled={disabled}
      aria-label={ariaLabel}
    />
  );
}
