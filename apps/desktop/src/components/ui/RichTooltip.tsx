// ============================================================================
// RichTooltip - a structured tooltip with icons, a grid of columns, or mono
// paths, built straight on the kit's Tooltip primitives with the same
// sideOffset and shell TooltipControl uses.
// ============================================================================

import type { ReactElement, ReactNode } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@skill-studio/ui";

interface RichTooltipProps {
  content: ReactNode;
  children: ReactElement;
}

export function RichTooltip({ content, children }: RichTooltipProps) {
  return (
    <Tooltip>
      <TooltipTrigger render={children} />
      <TooltipContent
        sideOffset={6}
        className="max-w-none flex-col items-start gap-0.5 bg-bg-elevated text-small text-text-primary shadow-md ring-1 ring-border [&>[data-slot=tooltip-arrow]]:hidden"
      >
        {content}
      </TooltipContent>
    </Tooltip>
  );
}
