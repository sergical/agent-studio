// ============================================================================
// SelectControl - Kit Select wrapper: a 32 px trigger and a popup sized
// to match it, with a check icon on the selected item. Keyboard nav and
// typeahead come from the kit's Base UI foundation.
// ============================================================================

import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@skill-studio/ui";

export interface SelectControlItem {
  value: string;
  label: string;
}

interface SelectControlProps {
  value: string;
  onValueChange: (value: string) => void;
  items: SelectControlItem[];
  ariaLabel: string;
  /** Rendered before the trigger's value text, e.g. a harness icon. */
  leadingIcon?: React.ReactNode;
}

export function SelectControl({
  value,
  onValueChange,
  items,
  ariaLabel,
  leadingIcon,
}: SelectControlProps) {
  return (
    <Select
      items={items}
      value={value}
      onValueChange={(next) => {
        if (next != null) onValueChange(next);
      }}
    >
      <SelectTrigger
        aria-label={ariaLabel}
        className="h-(--control-height) min-w-0 justify-between gap-1.5 rounded-sm border-border bg-bg-tertiary py-0 pr-2 pl-3 text-body text-text-secondary hover:bg-bg-hover hover:text-text-primary data-open:text-text-primary"
      >
        <span className="inline-flex min-w-0 flex-1 items-center gap-1.5 overflow-hidden">
          {leadingIcon}
          <SelectValue className="truncate" />
        </span>
      </SelectTrigger>
      <SelectContent className="min-w-(--anchor-width) gap-px rounded-md border border-border bg-bg-secondary p-1 shadow-md">
        {items.map((item) => (
          <SelectItem
            key={item.value}
            value={item.value}
            className="h-(--control-height) rounded-sm px-2.5 text-body text-text-secondary data-highlighted:bg-bg-hover data-highlighted:text-text-primary [&_svg]:text-accent"
          >
            <span className="truncate" title={item.label}>
              {item.label}
            </span>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
