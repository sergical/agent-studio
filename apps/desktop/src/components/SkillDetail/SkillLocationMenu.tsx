// ============================================================================
// SkillLocationMenu - the ⋯ menu every Locations row and scope head opens:
// fixes and reveal first, danger items after a separator, then a hint line.
// The ordering itself is `rowMenu`'s job (skill-location-status.ts); this
// file only renders what it returns.
// ============================================================================

import { Ellipsis } from "lucide-react";
import { MenuControl, MenuItem, MenuSeparator } from "../ui/MenuControl";
import type { LocationAction, MenuEntry } from "./skill-location-status";

export function SkillLocationMenu({
  entries,
  danger,
  hint,
  onAction,
  ariaLabel,
}: {
  entries: MenuEntry[];
  danger: MenuEntry[];
  hint?: string;
  onAction: (action: LocationAction) => void;
  /** The row's own label, for the trigger's "More actions for <label>" accessible name. */
  ariaLabel: string;
}) {
  if (entries.length === 0 && danger.length === 0) return <span className="size-7" />;
  return (
    <MenuControl
      triggerClassName="flex h-7 w-7 cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent text-text-tertiary transition-colors hover:bg-bg-tertiary hover:text-text-primary"
      triggerAriaLabel={`More actions for ${ariaLabel}`}
      trigger={<Ellipsis size={14} />}
      align="end"
      popupClassName="min-w-[200px]"
    >
      {entries.map((entry) => (
        <MenuItem key={entry.label} closeOnClick onClick={() => onAction(entry.action)}>
          {entry.label}
        </MenuItem>
      ))}
      {danger.length > 0 && <MenuSeparator />}
      {danger.map((entry) => (
        <MenuItem
          key={entry.label}
          closeOnClick
          variant="destructive"
          onClick={() => onAction(entry.action)}
        >
          {entry.label}
        </MenuItem>
      ))}
      {hint && <div className="px-2.5 py-1 text-caption text-text-tertiary">{hint}</div>}
    </MenuControl>
  );
}
