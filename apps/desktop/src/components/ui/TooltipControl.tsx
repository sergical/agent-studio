// ============================================================================
// TooltipControl - Kit Tooltip wrapper: text-only content. App.tsx's
// `Tooltip.Provider` (delay=400) is the same underlying Base UI provider the
// kit's Tooltip builds on, so every tooltip in the app shares one open/close
// delay group.
// ============================================================================

import { Tooltip, TooltipContent, TooltipTrigger } from "@skill-studio/ui";

interface TooltipControlProps {
  content: string;
  children: React.ReactElement;
}

/** Wraps `children` (the trigger) with a 400 ms-delayed, text-only tooltip. */
export function TooltipControl({ content, children }: TooltipControlProps) {
  return (
    <Tooltip>
      <TooltipTrigger render={children} />
      <TooltipContent
        sideOffset={6}
        className="max-w-65 bg-bg-elevated text-small text-text-primary shadow-md ring-1 ring-border"
      >
        {content}
      </TooltipContent>
    </Tooltip>
  );
}
