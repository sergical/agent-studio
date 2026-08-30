// ============================================================================
// InfoPopover - A click-to-open "ⓘ" dialog, not a hover tooltip: its body can
// hold a "Learn more" link, which a tooltip can't (tooltips close before a
// click on their content lands). One at a time; the trigger and its dialog
// are siblings in a wrapper span, since a `<button>` can't nest inside
// another `<button>`.
// ============================================================================

import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

interface InfoPopoverProps {
  /** aria-label for the ⓘ trigger button, e.g. "About broken". */
  label: string;
  /** aria-label for the dialog itself, e.g. "Broken and warnings". */
  title: string;
  /** The one-sentence body. */
  children: ReactNode;
  /** Renders a "Learn more →" button when given; closes the popover after calling it. */
  onLearnMore?: () => void;
}

export function InfoPopover({ label, title, children, onLearnMore }: InfoPopoverProps) {
  const [isOpen, setIsOpen] = useState(false);
  const anchorRef = useRef<HTMLSpanElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  const close = () => {
    setIsOpen(false);
    triggerRef.current?.focus();
  };

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
      close();
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
    close();
  };

  return (
    <span className="info-popover-anchor" ref={anchorRef}>
      <button
        ref={triggerRef}
        type="button"
        className="info-popover-trigger"
        aria-label={label}
        aria-expanded={isOpen}
        onClick={() => setIsOpen((open) => !open)}
      >
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5}>
          <circle cx={8} cy={8} r={6.25} />
          <path d="M8 7.2v4M8 5h.01" strokeLinecap="round" />
        </svg>
      </button>
      <div className="info-popover" role="dialog" aria-label={title} hidden={!isOpen}>
        <p>{children}</p>
        {onLearnMore && (
          <button type="button" className="info-popover-learn-more" onClick={handleLearnMore}>
            Learn more
            <span aria-hidden="true">→</span>
          </button>
        )}
      </div>
    </span>
  );
}
