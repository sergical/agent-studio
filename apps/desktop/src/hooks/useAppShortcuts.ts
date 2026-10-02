// ============================================================================
// useAppShortcuts - Document-level listener for the app's global shortcuts
// (`⌘K`, `⌘N`, `⌘,`, `⌘[`, `⌘]`, `/`) and the mouse back/forward buttons, defined once in `app-shortcuts.ts` so the palette,
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
 * input; `⌘N`/`⌘,`/`⌘[`/`⌘]` are blocked while a dialog, menu, or listbox is already open; `/` only fires on
 * the Skills view, and never while an editable element already has focus. `⌘[`/`⌘]` skip editable focus too
 * (indent in some editors); the mouse buttons still work from there. */
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
      if (meta && (event.key === "[" || event.key === "]") && !isEditable(event.target)) {
        event.preventDefault();
        const { goBack, goForward } = useAppStore.getState();
        if (event.key === "[") goBack();
        else goForward();
        return;
      }
      if (event.key === "/" && !isEditable(event.target)) {
        const { activeView, requestSkillSearchFocus } = useAppStore.getState();
        if (activeView.kind !== "skills") return;
        event.preventDefault();
        requestSkillSearchFocus();
      }
    }
    // Buttons 3 and 4 are the mouse's back and forward thumb buttons. Handled on `mouseup`;
    // `auxclick` is only default-prevented so it never also triggers the webview's own handling.
    function onMouseUp(event: MouseEvent) {
      if (event.button !== 3 && event.button !== 4) return;
      event.preventDefault();
      if (useAppStore.getState().commandPaletteOpen || isModalSurfaceOpen()) return;
      const { goBack, goForward } = useAppStore.getState();
      if (event.button === 3) goBack();
      else goForward();
    }
    function onAuxClick(event: MouseEvent) {
      if (event.button === 3 || event.button === 4) event.preventDefault();
    }
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("mouseup", onMouseUp);
    window.addEventListener("auxclick", onAuxClick);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("mouseup", onMouseUp);
      window.removeEventListener("auxclick", onAuxClick);
    };
  }, []);
}
