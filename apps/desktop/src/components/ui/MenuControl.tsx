// ============================================================================
// MenuControl - thin wrapper over the kit's DropdownMenu: the root, trigger,
// and popup shell, styled to the app's tokens. MenuItem, MenuRadioGroup,
// MenuRadioItem, and MenuSeparator carry the app's default item look baked
// in, so callers (InstalledSkillHeader, SkillLocationMenu, SkillListFilterBar)
// only pass a className to override that default, not to restate it - this
// file is the app's only import of the kit's dropdown-menu pieces.
// Focus returns to the trigger on close (Base UI default).
// ============================================================================

import type { ComponentProps } from "react";
import {
  cn,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@skill-studio/ui";

const DEFAULT_ITEM_CLASS =
  "flex h-(--control-height) cursor-pointer items-center gap-2 rounded-sm px-2.5 text-body text-text-secondary transition-colors data-highlighted:bg-bg-hover data-highlighted:text-text-primary data-disabled:cursor-not-allowed data-disabled:text-text-quaternary data-[variant=destructive]:text-error data-[variant=destructive]:data-highlighted:bg-error-soft data-[variant=destructive]:data-highlighted:text-error";

export function MenuItem({ className, ...props }: ComponentProps<typeof DropdownMenuItem>) {
  return <DropdownMenuItem className={cn(DEFAULT_ITEM_CLASS, className)} {...props} />;
}

export const MenuRadioGroup = DropdownMenuRadioGroup;

export function MenuRadioItem({
  className,
  ...props
}: ComponentProps<typeof DropdownMenuRadioItem>) {
  return <DropdownMenuRadioItem className={cn(DEFAULT_ITEM_CLASS, className)} {...props} />;
}

export function MenuSeparator({
  className,
  ...props
}: ComponentProps<typeof DropdownMenuSeparator>) {
  return (
    <DropdownMenuSeparator
      className={cn("mx-0.5 my-1 h-px border-none bg-border-subtle", className)}
      {...props}
    />
  );
}

interface MenuControlProps {
  trigger: React.ReactNode;
  triggerClassName?: string;
  triggerAriaLabel?: string;
  children: React.ReactNode;
  /** Which edge of the trigger the popup aligns to - "end" keeps a right-edge trigger's popup from overflowing the window. */
  align?: "start" | "end";
  /** Extra classes on the popup. The popup sizes to its content (`w-auto` undoes the kit's anchor-width default), so this is for a floor like `min-w-[200px]`. */
  popupClassName?: string;
}

/** The root + trigger + popup shell of a menu; `children` is the item list. */
export function MenuControl({
  trigger,
  triggerClassName,
  triggerAriaLabel,
  children,
  align = "start",
  popupClassName,
}: MenuControlProps) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger className={triggerClassName} aria-label={triggerAriaLabel}>
        {trigger}
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align={align}
        sideOffset={4}
        className={cn(
          "z-(--z-dropdown) w-auto min-w-(--anchor-width) rounded-md border border-border bg-bg-secondary p-1 shadow-md ring-0",
          popupClassName,
        )}
      >
        {children}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
