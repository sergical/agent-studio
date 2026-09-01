// ============================================================================
// SkillAssistantDrawer - Right-hand overlay drawer holding the assistant
// panel: scrim, slide-in panel, closes on Escape/scrim click/the header's
// close button, and returns focus to the trigger that opened it.
// ============================================================================

import { useEffect, useEffectEvent, useRef } from "react";
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
 * Renders nothing while closed. While open, mounts a native `<dialog>`
 * (`--z-drawer`) via `showModal()`, which gives us the focus trap, Escape
 * handling, and top-layer stacking for free - `::backdrop` stands in for the
 * old scrim div. `SkillPage`'s own Escape handler still defers to it, so it
 * keeps its `role="dialog"` (implicit on `<dialog>`) for that `[role="dialog"]`
 * check.
 */
export function SkillAssistantDrawer({
  isOpen,
  onClose,
  triggerRef,
  children,
}: SkillAssistantDrawerProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (isOpen && !dialog.open) {
      dialog.showModal();
    } else if (!isOpen && dialog.open) {
      dialog.close();
    }
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    const trigger = triggerRef.current;
    return () => {
      trigger?.focus();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen]);

  // A mousedown that lands on the `<dialog>` element itself (not its content)
  // is a click on the backdrop area, since the dialog box is sized to the
  // panel - this replaces the old dedicated scrim div's `onMouseDown`.
  // Attached imperatively (not as a JSX prop) since `<dialog>` isn't an
  // interactive element.
  const onBackdropMouseDown = useEffectEvent((event: MouseEvent) => {
    if (event.target === dialogRef.current) onClose();
  });

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!isOpen || !dialog) return;
    dialog.addEventListener("mousedown", onBackdropMouseDown);
    return () => dialog.removeEventListener("mousedown", onBackdropMouseDown);
  }, [isOpen]);

  if (!isOpen) return null;

  return (
    <dialog
      ref={dialogRef}
      id={SKILL_ASSISTANT_DRAWER_ID}
      className="fixed top-0 right-0 bottom-0 z-(--z-drawer) m-0 h-full max-h-none w-[min(560px,92vw)] max-w-none flex-col overflow-y-auto rounded-none border-0 border-l border-border bg-bg-primary p-0 open:flex animate-[slideInAssistantDrawer_220ms_ease-out] backdrop:bg-scrim backdrop:animate-[fadeInAssistantScrim_220ms_ease-out]"
      aria-label="Assistant"
      // The native `cancel` event fires on Escape before `close`; handling it
      // here keeps the same single `onClose` path the backdrop click uses.
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
      onClose={onClose}
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
    </dialog>
  );
}
