// ============================================================================
// useRowCursor - Roving-tabIndex keyboard navigation for the Skills and Home
// grids: j/k and arrow keys move the cursor row, Enter opens it, Space/x
// toggles it, Shift+Up/Down extends the selection, `.`/Shift+F10 opens the
// row's ⋯ menu, and ArrowLeft/ArrowRight collapse or expand its group.
// `useRowCursorWindowEntry` is the window-level counterpart: j/k/arrows focus
// the cursor row from anywhere in the active view.
// ============================================================================

import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

interface UseRowCursorOptions {
  /** Visible row keys, in rendered order - skips rows inside collapsed groups. */
  keys: string[];
  onOpen: (key: string) => void;
  onToggle?: (key: string) => void;
  onMenu?: (key: string, rowEl: HTMLElement) => void;
  /** Shift+ArrowUp/Down: called with the row the cursor moved onto, to extend the selection. */
  onExtend?: (key: string) => void;
  /** Escape: clears the selection, mirroring the list's own Cancel/Escape behaviour. */
  onEscape?: () => void;
  onCollapseGroup?: (groupId: string) => void;
  onExpandGroup?: (groupId: string) => void;
  /** Seeds the cursor on this key (when present in `keys`) and focuses its row once, on mount -
   * e.g. the row a skill was opened from, so Escape from its detail page returns focus there
   * instead of resetting to the first row. */
  initialKey?: string | null;
}

export interface RowCursor {
  cursorKey: string | null;
  /** Ref callback for one row - registers it so movement can focus and scroll it. */
  rowRef: (key: string) => (el: HTMLDivElement | null) => void;
  /** Ref callback for the grid element itself, for `[data-group-header]` lookups. */
  containerRef: (el: HTMLDivElement | null) => void;
  tabIndexFor: (key: string) => 0 | -1;
  onGridKeyDown: (e: ReactKeyboardEvent) => void;
  /** Focuses the current cursor row (or the first row) - the window-level entry point calls this. */
  focusCursor: () => void;
  /** Focuses one row by key - e.g. refocusing the row once its own menu (opened by mouse or `.`) closes. */
  focusRow: (key: string) => void;
  /** "12 of 80" - the visually-hidden `role="status"` text, debounced to the last move. */
  statusText: string;
}

/** Announce debounce - matches the app's short interaction delays elsewhere. */
const ANNOUNCE_DELAY_MS = 150;

/** Keys the window-level entry point reaches for - `Home`/`End` stay list-local, since they'd
 * otherwise fight the page's own scroll keys everywhere else in the app. */
const WINDOW_ENTRY_KEYS = new Set(["ArrowDown", "j", "ArrowUp", "k"]);

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return Boolean(
    target.tagName === "INPUT" ||
    target.tagName === "TEXTAREA" ||
    target.isContentEditable ||
    target.closest('[role="menu"], [role="listbox"], [role="dialog"], dialog'),
  );
}

export function useRowCursor({
  keys,
  onOpen,
  onToggle,
  onMenu,
  onExtend,
  onEscape,
  onCollapseGroup,
  onExpandGroup,
  initialKey,
}: UseRowCursorOptions): RowCursor {
  const [cursorKey, setCursorKey] = useState<string | null>(
    (initialKey && keys.includes(initialKey) ? initialKey : keys[0]) ?? null,
  );
  const rowsRef = useRef(new Map<string, HTMLDivElement>());
  const gridRef = useRef<HTMLDivElement | null>(null);
  const lastIndexRef = useRef(0);
  const pendingExpandRef = useRef<string | null>(null);
  const [statusText, setStatusText] = useState("");
  const statusTimerRef = useRef<number | undefined>(undefined);

  // The cursor survives a re-sort or filter change when its key is still visible; otherwise it
  // moves to the nearest row by its previous index, so the cursor never silently vanishes.
  useEffect(() => {
    if (keys.length === 0) {
      setCursorKey(null);
      return;
    }
    setCursorKey((current) => {
      if (current !== null && keys.includes(current)) return current;
      return keys[Math.min(lastIndexRef.current, keys.length - 1)];
    });
    // A group expanded by ArrowRight: its rows only join `keys` on this render.
    const groupId = pendingExpandRef.current;
    if (groupId === null) return;
    pendingExpandRef.current = null;
    const firstInGroup = keys.find(
      (key) => rowsRef.current.get(key)?.closest(`[data-group="${groupId}"]`) != null,
    );
    if (firstInGroup) moveTo(firstInGroup, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- moveTo reads the same render's keys.
  }, [keys]);

  // Focuses the seeded row once on mount, after its ref has attached - this is what returns
  // focus to a row when the caller remounts the grid with the previously-open skill's key.
  useEffect(() => {
    if (!initialKey) return;
    const frame = requestAnimationFrame(() => scrollAndFocus(initialKey));
    return () => cancelAnimationFrame(frame);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- runs once, keyed by mount only.
  }, []);

  function scrollAndFocus(key: string) {
    const el = rowsRef.current.get(key);
    if (!el) return;
    el.focus({ preventScroll: true });
    el.scrollIntoView({ block: "nearest" });
  }

  function announce(key: string) {
    const index = keys.indexOf(key);
    if (statusTimerRef.current !== undefined) window.clearTimeout(statusTimerRef.current);
    statusTimerRef.current = window.setTimeout(() => {
      setStatusText(`${index + 1} of ${keys.length}`);
    }, ANNOUNCE_DELAY_MS);
  }

  function moveTo(key: string | undefined, extend: boolean) {
    if (key === undefined) return;
    const index = keys.indexOf(key);
    if (index === -1) return;
    lastIndexRef.current = index;
    setCursorKey(key);
    scrollAndFocus(key);
    announce(key);
    if (extend) onExtend?.(key);
  }

  function groupIdFor(rowEl: HTMLElement): string | null {
    return rowEl.closest("[data-group]")?.getAttribute("data-group") ?? null;
  }

  function focusCursor() {
    scrollAndFocus(cursorKey ?? keys[0]);
  }

  /** Public: focuses one row without touching the announced status or extending the selection -
   * e.g. refocusing the row once its own menu (opened by mouse or `.`) closes. */
  function focusRow(key: string) {
    moveTo(key, false);
  }

  function onGridKeyDown(e: ReactKeyboardEvent) {
    const target = e.target;
    if (!(target instanceof HTMLElement)) return;

    // `ArrowRight` on a collapsed group's own header button expands it and moves into the group.
    const headerGroupId = target.getAttribute("data-group-header");
    if (headerGroupId !== null) {
      if (e.key === "ArrowRight") {
        e.preventDefault();
        pendingExpandRef.current = headerGroupId;
        onExpandGroup?.(headerGroupId);
      }
      return;
    }

    if (cursorKey === null) return;
    const currentIndex = keys.indexOf(cursorKey);

    switch (e.key) {
      case "ArrowDown":
      case "j":
        e.preventDefault();
        moveTo(keys[Math.min(currentIndex + 1, keys.length - 1)], e.shiftKey);
        break;
      case "ArrowUp":
      case "k":
        e.preventDefault();
        moveTo(keys[Math.max(currentIndex - 1, 0)], e.shiftKey);
        break;
      case "Home":
        e.preventDefault();
        moveTo(keys[0], false);
        break;
      case "End":
        e.preventDefault();
        moveTo(keys[keys.length - 1], false);
        break;
      case "Enter":
        e.preventDefault();
        onOpen(cursorKey);
        break;
      case " ":
      case "x":
        if (onToggle) {
          e.preventDefault();
          onToggle(cursorKey);
        }
        break;
      case ".":
        if (onMenu) {
          e.preventDefault();
          onMenu(cursorKey, target.closest('[role="row"]') ?? target);
        }
        break;
      case "F10":
        if (e.shiftKey && onMenu) {
          e.preventDefault();
          onMenu(cursorKey, target.closest('[role="row"]') ?? target);
        }
        break;
      case "ArrowLeft": {
        const rowEl = rowsRef.current.get(cursorKey);
        const groupId = rowEl ? groupIdFor(rowEl) : null;
        if (groupId && onCollapseGroup) {
          e.preventDefault();
          onCollapseGroup(groupId);
          const header = gridRef.current?.querySelector<HTMLElement>(
            `[data-group-header="${groupId}"]`,
          );
          if (header) requestAnimationFrame(() => header.focus());
        }
        break;
      }
      case "Escape":
        if (onEscape) {
          e.preventDefault();
          onEscape();
        }
        break;
      default:
        break;
    }
  }

  return {
    cursorKey,
    rowRef: (key) => (el) => {
      if (el) rowsRef.current.set(key, el);
      else rowsRef.current.delete(key);
    },
    containerRef: (el) => {
      gridRef.current = el;
    },
    tabIndexFor: (key) => (key === cursorKey ? 0 : -1),
    onGridKeyDown,
    focusCursor,
    focusRow,
    statusText,
  };
}

/**
 * Window-level entry point: while `active` (the owning view is on screen) and
 * focus isn't in an editable element, menu, listbox, or dialog, `j`/`k`/arrow
 * keys focus the list's cursor row.
 */
export function useRowCursorWindowEntry(active: boolean, focusCursor: () => void): void {
  useEffect(() => {
    if (!active) return;
    function onKeyDown(e: KeyboardEvent) {
      if (!WINDOW_ENTRY_KEYS.has(e.key)) return;
      if (isEditable(e.target)) return;
      const activeEl = document.activeElement;
      if (activeEl instanceof HTMLElement && activeEl.closest('[role="row"]')) return;
      e.preventDefault();
      focusCursor();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [active, focusCursor]);
}
