// ============================================================================
// Skill Studio - useNativeShell
// Document-level listeners that make the webview behave like native chrome
// instead of a web page: no browser context menu outside editable text, no
// reload/zoom shortcuts or pinch zoom.
// ============================================================================

import { useEffect } from "react";

/** Elements where a right-click should still open the browser's own context menu (text entry, or an explicit opt-in). */
const CONTEXT_MENU_ALLOWED_SELECTOR =
  "input, textarea, [contenteditable], .monaco-editor, [data-allow-context-menu]";

/** Keys a native app shell reserves - reload and zoom - that a web page would otherwise handle itself. */
const RESERVED_KEYS = new Set(["r", "R", "=", "+", "-", "0"]);

function handleContextMenu(event: MouseEvent): void {
  const target = event.target;
  if (target instanceof Element && target.closest(CONTEXT_MENU_ALLOWED_SELECTOR)) return;
  event.preventDefault();
}

function handleReservedKeyDown(event: KeyboardEvent): void {
  if (!(event.metaKey || event.ctrlKey)) return;
  if (!RESERVED_KEYS.has(event.key)) return;
  event.preventDefault();
}

function handlePinchWheel(event: WheelEvent): void {
  // WebKit reports a trackpad pinch as a wheel event with ctrlKey set - there's
  // no other way to distinguish it from a real ctrl+scroll.
  if (event.ctrlKey) event.preventDefault();
}

function handleGestureEvent(event: Event): void {
  event.preventDefault();
}

/**
 * Registers the listeners that stop the app from feeling like a web page in
 * a window: right-click is blocked outside text entry, reload/zoom shortcuts
 * and pinch-to-zoom are swallowed inside the Tauri webview (dev-server
 * browser sessions keep them, so reload still works there).
 */
export function useNativeShell(): void {
  useEffect(() => {
    document.addEventListener("contextmenu", handleContextMenu);

    const isTauri = "__TAURI_INTERNALS__" in window;
    if (isTauri) {
      document.addEventListener("keydown", handleReservedKeyDown, { capture: true });
    }

    document.addEventListener("wheel", handlePinchWheel, { passive: false });
    // `gesturestart`/`gesturechange` are WebKit-only pinch events with no
    // TypeScript lib.dom typing, hence the plain `Event` handler and string
    // event names instead of a typed `GestureEvent`.
    document.addEventListener("gesturestart", handleGestureEvent);
    document.addEventListener("gesturechange", handleGestureEvent);

    return () => {
      document.removeEventListener("contextmenu", handleContextMenu);
      if (isTauri) {
        document.removeEventListener("keydown", handleReservedKeyDown, { capture: true });
      }
      document.removeEventListener("wheel", handlePinchWheel);
      document.removeEventListener("gesturestart", handleGestureEvent);
      document.removeEventListener("gesturechange", handleGestureEvent);
    };
  }, []);
}
