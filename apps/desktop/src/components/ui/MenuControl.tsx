// ============================================================================
// MenuControl - Base UI Menu wrapper: the root, trigger, and popup shell.
// Callers build the items with the re-exported MenuItem, MenuRadioGroup,
// MenuRadioItem, and MenuSeparator, styled with the .menu-control-item(-*)
// classes that live on in App.css. Kept on the Base UI primitive directly
// (not the kit's DropdownMenu): those classes and re-exports are load-bearing
// for out-of-scope callers (InstalledSkillHeader, SkillListFilterBar) that
// build their own menu items against this exact API. Focus returns to the
// trigger on close (Base UI default).
// ============================================================================

import { Menu } from "@base-ui/react/menu";

export const MenuItem = Menu.Item;
export const MenuRadioGroup = Menu.RadioGroup;
export const MenuRadioItem = Menu.RadioItem;
export const MenuSeparator = Menu.Separator;

interface MenuControlProps {
  trigger: React.ReactNode;
  triggerClassName?: string;
  triggerAriaLabel?: string;
  children: React.ReactNode;
  /** Which edge of the trigger the popup aligns to - "end" keeps a right-edge trigger's popup from overflowing the window. */
  align?: "start" | "end";
}

/** The root + trigger + popup shell of a menu; `children` is the item list. */
export function MenuControl({
  trigger,
  triggerClassName,
  triggerAriaLabel,
  children,
  align = "start",
}: MenuControlProps) {
  return (
    <Menu.Root>
      <Menu.Trigger className={triggerClassName} aria-label={triggerAriaLabel}>
        {trigger}
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner sideOffset={4} align={align} collisionPadding={8}>
          <Menu.Popup className="z-(--z-dropdown) min-w-(--anchor-width) rounded-md border border-border bg-bg-secondary p-1 shadow-md">
            {children}
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}
