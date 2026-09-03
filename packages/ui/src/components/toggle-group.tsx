// ============================================================================
// @skill-studio/ui - ToggleGroup
// Vendored from shadcn's Base UI registry (`shadcn add toggle-group -b base`),
// plus a `segmented` variant matching the app's WindowSegmentedControl look.
// ============================================================================

import { createContext, useContext } from "react";
import { ToggleGroup as ToggleGroupPrimitive } from "@base-ui/react/toggle-group";
import { type VariantProps } from "class-variance-authority";

import { cn } from "../lib/cn";
import { Toggle, toggleVariants } from "./toggle";

type ToggleGroupVariant = VariantProps<typeof toggleVariants>["variant"] | "segmented";

interface ToggleGroupContextValue {
  variant: ToggleGroupVariant;
  size: VariantProps<typeof toggleVariants>["size"];
}

const ToggleGroupContext = createContext<ToggleGroupContextValue>({
  variant: "default",
  size: "default",
});

const SEGMENTED_ITEM_CLASS =
  "h-6 rounded-none border-l border-border px-2 text-caption text-text-tertiary first:border-l-0 data-pressed:bg-bg-tertiary data-pressed:text-text-primary";

function ToggleGroup({
  className,
  variant = "default",
  size = "default",
  children,
  ...props
}: ToggleGroupPrimitive.Props<string> & {
  variant?: ToggleGroupVariant;
  size?: VariantProps<typeof toggleVariants>["size"];
}) {
  return (
    <ToggleGroupPrimitive
      data-slot="toggle-group"
      data-variant={variant}
      data-size={size}
      className={cn(
        "flex items-center gap-1 data-[variant=segmented]:inline-flex data-[variant=segmented]:w-fit data-[variant=segmented]:gap-0 data-[variant=segmented]:overflow-hidden data-[variant=segmented]:rounded-sm data-[variant=segmented]:border data-[variant=segmented]:border-border",
        className,
      )}
      {...props}
    >
      <ToggleGroupContext value={{ variant, size }}>{children}</ToggleGroupContext>
    </ToggleGroupPrimitive>
  );
}

function ToggleGroupItem({ className, children, variant, size, ...props }: ToggleGroupItemProps) {
  const context = useContext(ToggleGroupContext);
  const resolvedVariant = variant ?? context.variant;
  const resolvedSize = size ?? context.size;

  if (resolvedVariant === "segmented") {
    return (
      <Toggle
        data-slot="toggle-group-item"
        className={cn(SEGMENTED_ITEM_CLASS, className)}
        {...props}
      >
        {children}
      </Toggle>
    );
  }

  return (
    <Toggle
      data-slot="toggle-group-item"
      variant={resolvedVariant}
      size={resolvedSize}
      className={className}
      {...props}
    >
      {children}
    </Toggle>
  );
}

type ToggleGroupItemProps = Omit<Parameters<typeof Toggle>[0], "variant"> & {
  variant?: ToggleGroupVariant;
};

export { ToggleGroup, ToggleGroupItem };
