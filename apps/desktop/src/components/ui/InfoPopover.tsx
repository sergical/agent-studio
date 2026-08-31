// ============================================================================
// InfoPopover - A click-to-open "ⓘ" dialog, not a hover tooltip: its body can
// hold a "Learn more" link, which a tooltip can't (tooltips close before a
// click on their content lands). One at a time; the trigger and its dialog
// are siblings in a wrapper span, since a `<button>` can't nest inside
// another `<button>`.
// ============================================================================

import { useEffect, useRef, useState } from "react";
import type { Dispatch, RefObject, SetStateAction, ReactNode } from "react";

/**
 * Closes the popover and returns focus to its trigger. A module-level
 * function so its identity never changes - it needs no dependency in the
 * mousedown/keydown effect below beyond `isOpen`, which already governs
 * when that effect (re)subscribes.
 */
function close(
  triggerRef: RefObject<HTMLButtonElement | null>,
  setIsOpen: Dispatch<SetStateAction<boolean>>,
) {
  setIsOpen(false);
  triggerRef.current?.focus();
}

interface InfoPopoverProps {
  /** aria-label for the ⓘ trigger button, e.g. "About broken". */
  label: string;
  /** aria-label for the dialog itself, e.g. "Broken and warnings". */
  title: string;
  /** The one-sentence body. */
  children: ReactNode;
  /** Renders a "Learn more →" button when given; closes the popover after calling it. */
  onLearnMore?: () => void;
  /** Extra classes on the anchor span - e.g. a caller's own hover-reveal positioning. */
  className?: string;
}

export function InfoPopover({ label, title, children, onLearnMore, className }: InfoPopoverProps) {
  const [isOpen, setIsOpen] = useState(false);
  const anchorRef = useRef<HTMLSpanElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!isOpen) return;

    function onMouseDown(event: MouseEvent) {
      const target = event.target;
      if (!(target instanceof Node) || !anchorRef.current?.contains(target)) setIsOpen(false);
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      // Stops here so the SkillPage-style global Escape handler doesn't also
      // fire and, e.g., close a skill page behind this popover.
      event.stopPropagation();
      close(triggerRef, setIsOpen);
    }
    window.addEventListener("mousedown", onMouseDown);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [isOpen]);

  const handleLearnMore = () => {
    onLearnMore?.();
    close(triggerRef, setIsOpen);
  };

  return (
    <span className={`relative inline-flex ${className ?? ""}`} ref={anchorRef}>
      <button
        ref={triggerRef}
        type="button"
        className={`relative inline-flex size-3.5 border-0 bg-none p-0 align-middle text-text-quaternary transition-colors hover:text-text-secondary focus-visible:text-text-secondary ${
          isOpen ? "text-accent" : ""
        }`}
        aria-label={label}
        aria-expanded={isOpen}
        onClick={() => setIsOpen((open) => !open)}
      >
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5}>
          <circle cx={8} cy={8} r={6.25} />
          <path d="M8 7.2v4M8 5h.01" strokeLinecap="round" />
        </svg>
      </button>
      <div
        className="absolute top-[calc(100%+8px)] left-1/2 z-(--z-tooltip) flex w-70 -translate-x-1/2 flex-col items-start gap-2 rounded-sm border border-border bg-bg-elevated py-2.5 px-3 text-left text-small leading-[1.45] font-normal whitespace-normal text-text-primary shadow-md"
        role="dialog"
        aria-label={title}
        hidden={!isOpen}
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
