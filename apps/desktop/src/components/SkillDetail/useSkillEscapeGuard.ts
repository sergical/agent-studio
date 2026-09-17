// ============================================================================
// useSkillEscapeGuard - Escape-to-back for SkillPage: guarded by a discard-
// changes dialog while an edit is dirty, skipped while Escape belongs to an
// input, menu, listbox, or dialog on the page instead.
// ============================================================================

import { useEffect, useEffectEvent, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { useAppStore } from "../../store/appStore";

export interface UseSkillEscapeGuard {
  pendingDiscard: (() => void) | null;
  setPendingDiscard: Dispatch<SetStateAction<(() => void) | null>>;
}

export function useSkillEscapeGuard(
  isEditing: boolean,
  isEditorDirty: boolean,
  onBack: () => void,
): UseSkillEscapeGuard {
  // Set while the discard-changes guard is waiting on the user - runs on
  // confirm, cleared on cancel. The old native confirm() prompt made this a
  // synchronous check; the dialog makes it async instead.
  const [pendingDiscard, setPendingDiscard] = useState<(() => void) | null>(null);

  // Reads the latest isEditing/isEditorDirty/onBack without making the
  // listener effect below re-subscribe every time one of them changes.
  const onEscapeBack = useEffectEvent(() => {
    if (isEditing && isEditorDirty) {
      setPendingDiscard(() => onBack);
      return;
    }
    onBack();
  });

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      // The command palette owns Escape while it's open - it can outlive its own dialog's
      // ownership of `target` briefly (e.g. right after opening, before focus moves into it).
      if (useAppStore.getState().commandPaletteOpen) return;
      const target = event.target;
      // Escape typed into an input, textarea, contenteditable region, an open
      // menu, listbox, or dialog belongs to that widget - the local handler
      // (if any) deals with it, not page navigation. Popovers are role=dialog.
      // `[data-open]` is not a signal: open Collapsible panels carry it too.
      if (
        target instanceof HTMLElement &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable ||
          target.closest("dialog") !== null ||
          target.closest('[role="dialog"]') !== null ||
          target.closest('[role="menu"]') !== null ||
          target.closest('[role="listbox"]') !== null)
      ) {
        return;
      }
      onEscapeBack();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return { pendingDiscard, setPendingDiscard };
}
