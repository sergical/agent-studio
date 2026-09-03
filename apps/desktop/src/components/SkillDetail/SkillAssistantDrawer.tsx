// ============================================================================
// SkillAssistantDrawer - Right-hand overlay drawer holding the assistant
// panel: scrim, slide-in panel, closes on Escape/scrim click/the header's
// close button, and returns focus to the trigger that opened it.
// ============================================================================

import type { RefObject } from "react";
import { Drawer, DrawerClose, DrawerContent } from "@skill-studio/ui";
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
 * The kit Drawer gives us the focus trap, Escape handling, and top-layer
 * stacking for free. `SkillPage`'s own Escape handler still defers to it via
 * `[role="dialog"]`, which Base UI's popup carries.
 */
export function SkillAssistantDrawer({
  isOpen,
  onClose,
  triggerRef,
  children,
}: SkillAssistantDrawerProps) {
  return (
    <Drawer open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DrawerContent
        side="right"
        className="w-[min(560px,92vw)] bg-bg-primary"
        aria-label="Assistant"
        id={SKILL_ASSISTANT_DRAWER_ID}
        finalFocus={triggerRef}
        showCloseButton={false}
      >
        <div className="flex items-center justify-between gap-3 border-b border-border p-4">
          <span className="text-emphasis font-semibold text-text-primary">Assistant</span>
          <DrawerClose
            className="flex size-(--control-height) cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent text-text-secondary transition-colors hover:bg-bg-tertiary hover:text-text-primary"
            aria-label="Close assistant"
          >
            <X size={16} />
          </DrawerClose>
        </div>
        <div className="flex flex-col p-4">{children}</div>
      </DrawerContent>
    </Drawer>
  );
}
