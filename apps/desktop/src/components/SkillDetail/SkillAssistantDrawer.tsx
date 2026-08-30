// ============================================================================
// SkillAssistantDrawer - Right-hand overlay drawer holding the assistant
// panel: scrim, slide-in panel, closes on Escape/scrim click/the header's
// close button, and returns focus to the trigger that opened it.
// ============================================================================

import { useEffect, useRef } from "react";
import type { RefObject } from "react";
import { X } from "lucide-react";

/** `aria-controls` target for the header's trigger button. */
export const SKILL_ASSISTANT_DRAWER_ID = "skill-assistant-drawer";

interface SkillAssistantDrawerProps {
  isOpen: boolean;
  onClose: () => void;
  /** The header button that opened the drawer, so closing returns focus to it. */
  triggerRef: RefObject<HTMLButtonElement | null>;
  children: React.ReactNode;
}

/**
 * Renders nothing while closed. While open, mounts a scrim (`--z-backdrop`)
 * and the drawer panel (`--z-drawer`, `role="dialog"`) so `SkillPage`'s own
 * Escape handler defers to it - see that handler's `[role="dialog"]` check.
 */
export function SkillAssistantDrawer({
  isOpen,
  onClose,
  triggerRef,
  children,
}: SkillAssistantDrawerProps) {
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!isOpen) return;
    panelRef.current?.focus();
    const trigger = triggerRef.current;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      trigger?.focus();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  return (
    <div
      className="fixed inset-0 z-(--z-backdrop) flex justify-end bg-scrim animate-[fadeInAssistantScrim_220ms_ease-out]"
      onMouseDown={onClose}
    >
      <div
        ref={panelRef}
        id={SKILL_ASSISTANT_DRAWER_ID}
        className="relative z-(--z-drawer) flex h-full w-[min(560px,92vw)] flex-col overflow-y-auto border-l border-border bg-bg-primary animate-[slideInAssistantDrawer_220ms_ease-out]"
        role="dialog"
        aria-modal="true"
        aria-label="Assistant"
        tabIndex={-1}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="flex items-center justify-between gap-3 border-b border-border p-4">
          <span className="text-emphasis font-semibold text-text-primary">Assistant</span>
          <button
            type="button"
            className="flex size-(--control-height) cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent text-text-secondary transition-colors hover:bg-bg-tertiary hover:text-text-primary"
            onClick={onClose}
            aria-label="Close assistant"
          >
            <X size={16} />
          </button>
        </div>
        <div className="flex flex-col p-4">{children}</div>
      </div>
    </div>
  );
}
