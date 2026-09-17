// ============================================================================
// useActivityEscape - Escape closes the open day's details, unless Escape
// belongs to an input, menu, listbox, or dialog on the page instead (each
// handles its own Escape and calls stopPropagation), or the command palette
// or skill detail page is what's actually showing.
// ============================================================================

import { useEffect } from "react";
import { useAppStore } from "../../store/appStore";

export function useActivityEscape(onEscape: () => void) {
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      const state = useAppStore.getState();
      if (state.commandPaletteOpen || state.activeView.kind !== "activity") return;
      const target = e.target;
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
      onEscape();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onEscape]);
}
