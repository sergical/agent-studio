// ============================================================================
// SkillListSelectionBar - The docked bar that appears once any row is
// checked: the selected count and Cancel. Pack creation is deferred (unit 4.3).
// ============================================================================

import { Button } from "@skill-studio/ui";

interface SkillListSelectionBarProps {
  count: number;
  onCancel: () => void;
}

export function SkillListSelectionBar({ count, onCancel }: SkillListSelectionBarProps) {
  return (
    // A zero-height wrapper so the sticky bar never reserves flow space of its own - checking a
    // row must not push any other row down. `sticky bottom-4` then docks the bar to the bottom of
    // the scroll area without an enter transition.
    <div className="pointer-events-none sticky inset-x-0 bottom-4 z-10 flex h-0 items-end justify-center">
      <div className="pointer-events-auto flex h-9 items-center gap-2 rounded-md border border-border bg-bg-secondary px-2 shadow">
        <span className="px-1 text-small text-text-secondary">{count} selected</span>
        <Button
          variant="outline"
          size="sm"
          className="rounded-sm text-text-tertiary"
          onClick={onCancel}
        >
          Cancel
        </Button>
      </div>
    </div>
  );
}
