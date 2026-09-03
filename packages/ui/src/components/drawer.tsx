// ============================================================================
// @skill-studio/ui - Drawer
// Vendored from shadcn's Base UI registry (`shadcn add drawer -b base`).
// ============================================================================

import type { ComponentProps } from "react";
import { Drawer as DrawerPrimitive } from "@base-ui/react/drawer";
import { XIcon } from "lucide-react";

import { cn } from "../lib/cn";
import { Button } from "./button";

function Drawer({ swipeDirection = "right", ...props }: DrawerPrimitive.Root.Props) {
  return <DrawerPrimitive.Root data-slot="drawer" swipeDirection={swipeDirection} {...props} />;
}

function DrawerTrigger({ ...props }: DrawerPrimitive.Trigger.Props) {
  return <DrawerPrimitive.Trigger data-slot="drawer-trigger" {...props} />;
}

function DrawerPortal({ ...props }: DrawerPrimitive.Portal.Props) {
  return <DrawerPrimitive.Portal data-slot="drawer-portal" {...props} />;
}

function DrawerClose({ ...props }: DrawerPrimitive.Close.Props) {
  return <DrawerPrimitive.Close data-slot="drawer-close" {...props} />;
}

function DrawerBackdrop({ className, ...props }: DrawerPrimitive.Backdrop.Props) {
  return (
    <DrawerPrimitive.Backdrop
      data-slot="drawer-backdrop"
      className={cn(
        "fixed inset-0 z-[var(--z-modal,50)] bg-black/50 duration-200 data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0",
        className,
      )}
      {...props}
    />
  );
}

const SIDE_CLASSES = {
  right:
    "inset-y-0 right-0 h-full w-[min(560px,92vw)] border-l slide-in-from-right slide-out-to-right",
  left: "inset-y-0 left-0 h-full w-[min(560px,92vw)] border-r slide-in-from-left slide-out-to-left",
  bottom:
    "inset-x-0 bottom-0 max-h-[85vh] rounded-t-xl border-t slide-in-from-bottom slide-out-to-bottom",
} as const satisfies Record<string, string>;

function DrawerContent({
  className,
  children,
  side = "right",
  showCloseButton = true,
  ...props
}: DrawerPrimitive.Popup.Props & {
  side?: keyof typeof SIDE_CLASSES;
  showCloseButton?: boolean;
}) {
  return (
    <DrawerPortal>
      <DrawerBackdrop />
      <DrawerPrimitive.Popup
        data-slot="drawer-content"
        className={cn(
          "fixed z-[var(--z-modal,50)] flex flex-col bg-popover text-popover-foreground outline-none duration-200 data-open:animate-in data-closed:animate-out",
          SIDE_CLASSES[side],
          className,
        )}
        {...props}
      >
        {side === "bottom" && <div className="mx-auto mt-2 h-1.5 w-12 rounded-full bg-muted" />}
        {children}
        {showCloseButton && (
          <DrawerPrimitive.Close
            data-slot="drawer-close"
            render={<Button variant="ghost" className="absolute top-2 right-2" size="icon-sm" />}
          >
            <XIcon />
            <span className="sr-only">Close</span>
          </DrawerPrimitive.Close>
        )}
      </DrawerPrimitive.Popup>
    </DrawerPortal>
  );
}

function DrawerHeader({ className, ...props }: ComponentProps<"div">) {
  return (
    <div data-slot="drawer-header" className={cn("flex flex-col gap-2", className)} {...props} />
  );
}

function DrawerFooter({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="drawer-footer"
      className={cn("mt-auto flex flex-col gap-2", className)}
      {...props}
    />
  );
}

function DrawerTitle({ className, ...props }: DrawerPrimitive.Title.Props) {
  return (
    <DrawerPrimitive.Title
      data-slot="drawer-title"
      className={cn("font-heading text-base leading-none font-medium", className)}
      {...props}
    />
  );
}

function DrawerDescription({ className, ...props }: DrawerPrimitive.Description.Props) {
  return (
    <DrawerPrimitive.Description
      data-slot="drawer-description"
      className={cn("text-sm text-muted-foreground", className)}
      {...props}
    />
  );
}

export {
  Drawer,
  DrawerClose,
  DrawerContent,
  DrawerDescription,
  DrawerFooter,
  DrawerHeader,
  DrawerPortal,
  DrawerTitle,
  DrawerTrigger,
};
