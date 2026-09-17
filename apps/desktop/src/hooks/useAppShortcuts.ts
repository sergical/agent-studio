// ============================================================================
// useAppShortcuts - Document-level listener for the app's global shortcuts
// (`⌘K`, `⌘N`, `⌘,`, `/`), defined once in `app-shortcuts.ts` so the palette,
// tooltips, and this handler never drift. Follows the `useNativeShell.ts`
// pattern: one hook, mounted once in `App.tsx`.
// ============================================================================

import { useEffect } from "react";
import { isModalSurfaceOpen } from "../lib/app-shortcuts";
import { useAppStore } from "../store/appStore";

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return Boolean(
    target.tagName === "INPUT" ||
    target.tagName === "TEXTAREA" ||
    target.isContentEditable ||
    target.closest('[role="dialog"], [role="menu"], [role="listbox"]'),
  );
}

/** Registers the global shortcut listener. `⌘K` always toggles the palette, even from inside an
 * input; `⌘N`/`⌘,` are blocked while a dialog, menu, or listbox is already open; `/` only fires on
 * the Skills view, and never while an editable element already has focus. */
export function useAppShortcuts(): void {
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const meta = event.metaKey || event.ctrlKey;
      if (meta && event.key.toLowerCase() === "k") {
        event.preventDefault();
        const { commandPaletteOpen, setCommandPaletteOpen } = useAppStore.getState();
        setCommandPaletteOpen(!commandPaletteOpen);
        return;
      }
      if (isModalSurfaceOpen()) return;
      if (meta && event.key.toLowerCase() === "n") {
        event.preventDefault();
        useAppStore.getState().openAddSkillSheet();
        return;
      }
      if (meta && event.key === ",") {
        event.preventDefault();
        useAppStore.getState().setActiveView({ kind: "settings" });
        return;
      }
      if (event.key === "/" && !isEditable(event.target)) {
        const { activeView, requestSkillSearchFocus } = useAppStore.getState();
        if (activeView.kind !== "skills") return;
        event.preventDefault();
        requestSkillSearchFocus();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}
