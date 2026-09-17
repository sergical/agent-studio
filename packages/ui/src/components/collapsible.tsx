// ============================================================================
// @skill-studio/ui - Collapsible
// Vendored from shadcn's Base UI registry (`shadcn add collapsible -b base`).
// ============================================================================

import { Collapsible as CollapsiblePrimitive } from "@base-ui/react/collapsible";

import { cn } from "../lib/cn";

function Collapsible({ ...props }: CollapsiblePrimitive.Root.Props) {
  return <CollapsiblePrimitive.Root data-slot="collapsible" {...props} />;
}

function CollapsibleTrigger({ ...props }: CollapsiblePrimitive.Trigger.Props) {
  return <CollapsiblePrimitive.Trigger data-slot="collapsible-trigger" {...props} />;
}

function CollapsiblePanel({ className, ...props }: CollapsiblePrimitive.Panel.Props) {
  return (
    <CollapsiblePrimitive.Panel
      data-slot="collapsible-content"
      className={cn(
        // Open/close is instant: Home's group panels are the only caller and
        // none needs the animated height Base UI supports by default.
        "h-(--collapsible-panel-height) overflow-hidden data-starting-style:h-0 data-ending-style:h-0",
        className,
      )}
      {...props}
    />
  );
}

// shadcn parity alias: the Base UI part is named `Panel`, upstream registries call it `Content`.
const CollapsibleContent = CollapsiblePanel;

export { Collapsible, CollapsibleTrigger, CollapsiblePanel, CollapsibleContent };
