// ============================================================================
// TooltipControl - Base UI Tooltip wrapper: text-only content. `Tooltip.
// Provider` (delay=400) is mounted once in App.tsx so every tooltip in the
// app shares one open/close delay group.
// ============================================================================

import { Tooltip } from "@base-ui/react/tooltip";

interface TooltipControlProps {
  content: string;
  children: React.ReactElement;
}

/** Wraps `children` (the trigger) with a 400 ms-delayed, text-only tooltip. */
export function TooltipControl({ content, children }: TooltipControlProps) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger render={children} />
      <Tooltip.Portal>
        <Tooltip.Positioner sideOffset={6}>
          <Tooltip.Popup className="tooltip-control-popup">{content}</Tooltip.Popup>
        </Tooltip.Positioner>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}
