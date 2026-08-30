// ============================================================================
// SelectControl - Base UI Select wrapper: a 32 px trigger and a popup sized
// to match it, with a check icon on the selected item. Keyboard nav and
// typeahead come from Base UI.
// ============================================================================

import { Select } from "@base-ui/react/select";
import { Check, ChevronDown } from "lucide-react";

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
    <Select.Root
      items={items}
      value={value}
      onValueChange={(next) => {
        if (next != null) onValueChange(next);
      }}
    >
      <Select.Trigger className="select-control-trigger" aria-label={ariaLabel}>
        <span className="select-control-trigger-value">
          {leadingIcon}
          <Select.Value />
        </span>
        <Select.Icon className="select-control-trigger-icon">
          <ChevronDown size={14} />
        </Select.Icon>
      </Select.Trigger>
      <Select.Portal>
        <Select.Positioner>
          <Select.Popup className="select-control-popup">
            {items.map((item) => (
              <Select.Item key={item.value} value={item.value} className="select-control-item">
                <Select.ItemText className="select-control-item-text" title={item.label}>
                  {item.label}
                </Select.ItemText>
                <Select.ItemIndicator className="select-control-item-indicator">
                  <Check size={14} />
                </Select.ItemIndicator>
              </Select.Item>
            ))}
          </Select.Popup>
        </Select.Positioner>
      </Select.Portal>
    </Select.Root>
  );
}
