// ============================================================================
// InfoPopover - A click-to-open "ⓘ" popover, not a hover tooltip: its body can
// hold a "Learn more" link, which a tooltip can't (tooltips close before a
// click on their content lands). Built on the kit Popover (Base UI) so
// placement, flipping, outside-click and Escape come from the same layer
// as TooltipControl.
// ============================================================================

import { useState } from "react";
import type { ReactNode } from "react";
import { Popover, PopoverContent, PopoverTrigger } from "@skill-studio/ui";

interface InfoPopoverProps {
  /** aria-label for the ⓘ trigger button, e.g. "About broken". */
  label: string;
  /** aria-label for the popover panel itself, e.g. "Broken and warnings". */
  title: string;
  /** The one-sentence body. */
  children: ReactNode;
  /** Renders a "Learn more →" button when given; closes the popover after calling it. */
  onLearnMore?: () => void;
  /** Extra classes on the anchor span - e.g. a caller's own hover-reveal positioning. */
  className?: string;
}

export function InfoPopover({ label, title, children, onLearnMore, className }: InfoPopoverProps) {
  // Controlled so the trigger can take its highlighted style while open and
  // "Learn more" can close the panel after navigating.
  const [isOpen, setIsOpen] = useState(false);

  const handleLearnMore = () => {
    onLearnMore?.();
    setIsOpen(false);
  };

  return (
    <span className={`relative inline-flex ${className ?? ""}`}>
      <Popover open={isOpen} onOpenChange={setIsOpen}>
        <PopoverTrigger
          className={`relative inline-flex size-3.5 border-0 bg-none p-0 align-middle text-text-quaternary transition-colors hover:text-text-secondary focus-visible:text-text-secondary ${
            isOpen ? "text-accent" : ""
          }`}
          aria-label={label}
        >
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5}>
            <circle cx={8} cy={8} r={6.25} />
            <path d="M8 7.2v4M8 5h.01" strokeLinecap="round" />
          </svg>
        </PopoverTrigger>
        <PopoverContent
          sideOffset={8}
          aria-label={title}
          className="w-70 items-start gap-2 rounded-sm bg-bg-elevated py-2.5 px-3 text-left text-small leading-[1.45] font-normal whitespace-normal text-text-primary shadow-md ring-border"
        >
          <p className="m-0">{children}</p>
          {onLearnMore && (
            <button
              type="button"
              className="inline-flex items-center gap-1 border-0 bg-none p-0 text-small text-accent hover:underline"
              onClick={handleLearnMore}
            >
              Learn more
              <span aria-hidden="true">→</span>
            </button>
          )}
        </PopoverContent>
      </Popover>
    </span>
  );
}
