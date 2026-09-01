// ============================================================================
// InfoPopover - A click-to-open "ⓘ" popover, not a hover tooltip: its body can
// hold a "Learn more" link, which a tooltip can't (tooltips close before a
// click on their content lands). One at a time; the trigger and its panel
// are siblings in a wrapper span, since a `<button>` can't nest inside
// another `<button>`.
// ============================================================================

import { useId, useRef, useState } from "react";
import type { ReactNode } from "react";

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
  const panelId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  // Mirrors the panel's native open/closed state for the trigger's
  // highlighted style and `aria-expanded` - the platform (not this
  // component) now owns outside-click and Escape dismissal.
  const [isOpen, setIsOpen] = useState(false);

  const handleLearnMore = () => {
    onLearnMore?.();
    panelRef.current?.hidePopover();
  };

  return (
    <span className={`relative inline-flex ${className ?? ""}`}>
      <button
        type="button"
        popoverTarget={panelId}
        className={`relative inline-flex size-3.5 border-0 bg-none p-0 align-middle text-text-quaternary transition-colors hover:text-text-secondary focus-visible:text-text-secondary ${
          isOpen ? "text-accent" : ""
        }`}
        aria-label={label}
        aria-expanded={isOpen}
      >
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5}>
          <circle cx={8} cy={8} r={6.25} />
          <path d="M8 7.2v4M8 5h.01" strokeLinecap="round" />
        </svg>
      </button>
      <div
        ref={panelRef}
        id={panelId}
        popover="auto"
        onToggle={(event) => setIsOpen(event.newState === "open")}
        // `absolute`/`inset-auto` override the popover UA stylesheet's
        // `position: fixed; inset: 0`, which otherwise fights the
        // anchor-relative placement below.
        className="absolute inset-auto top-[calc(100%+8px)] left-1/2 z-(--z-tooltip) m-0 flex w-70 -translate-x-1/2 flex-col items-start gap-2 rounded-sm border border-border bg-bg-elevated py-2.5 px-3 text-left text-small leading-[1.45] font-normal whitespace-normal text-text-primary shadow-md"
        aria-label={title}
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
      </div>
    </span>
  );
}
