// ============================================================================
// useRowCursor - Roving-tabIndex keyboard navigation for the Skills and Home
// grids: j/k and arrow keys move the cursor row, Enter opens it, Space/x
// toggles it, Shift+Up/Down extends the selection, `.`/Shift+F10 opens the
// row's ⋯ menu, and ArrowLeft/ArrowRight collapse or expand its group.
// `useRowCursorWindowEntry` is the window-level counterpart: j/k/arrows focus
// the cursor row from anywhere in the active view.
// ============================================================================

import { useEffect, useEffectEvent, useRef, useState } from "react";
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
  /** Seeds the cursor on this key (when present in `keys`) and focuses its row once - e.g. the row
   * a skill was opened from, so Escape from its detail page returns focus there instead of
   * resetting to the first row. */
  initialKey?: string | null;
  /** Whether the caller's view is on screen right now - defaults to `true`. Pass `false` while the
   * view is kept mounted but hidden behind another page (an open skill's page, kept alive so the
   * back button is instant); flipping back to `true` re-focuses `initialKey`'s row, without
   * scrolling if it's already in view. */
  active?: boolean;
  /** For a virtualized caller: scrolls an unmounted row's key into view - e.g. a virtualizer's
   * `scrollToIndex`. Once the row's element attaches, its ref callback focuses it. Omit this for a
   * caller (like `HomeView`) that mounts every row up front. */
  scrollToKey?: (key: string) => void;
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
  active: activeProp,
  scrollToKey,
}: UseRowCursorOptions): RowCursor {
  const active = activeProp ?? true;
  const [cursorKey, setCursorKey] = useState<string | null>(
    (initialKey && keys.includes(initialKey) ? initialKey : keys[0]) ?? null,
  );
  const rowsRef = useRef(new Map<string, HTMLDivElement>());
  const gridRef = useRef<HTMLDivElement | null>(null);
  /** Set by `scrollAndFocus` when a key's row isn't mounted yet - the row's own `rowRef` callback
   * focuses it once it attaches, then clears this. */
  const pendingFocusKeyRef = useRef<string | null>(null);
  // State, not a ref: the fallback below reads it during render, and a ref's `current` can't be
  // read there.
  const [lastIndex, setLastIndex] = useState(0);
  const pendingExpandRef = useRef<string | null>(null);
  const [statusText, setStatusText] = useState("");
  const statusTimerRef = useRef<number | undefined>(undefined);

  // The cursor survives a re-sort or filter change when its key is still visible; otherwise it
  // falls back to the nearest row by its previous index, so the cursor never silently vanishes -
  // derived on every render instead of written back into state, since it's fully determined by
  // `cursorKey`, `keys`, and the last moved-to index.
  const effectiveCursorKey =
    keys.length === 0
      ? null
      : cursorKey !== null && keys.includes(cursorKey)
        ? cursorKey
        : (keys[Math.min(lastIndex, keys.length - 1)] ?? null);

  function scrollAndFocus(key: string) {
    const el = rowsRef.current.get(key);
    if (!el) {
      // Not mounted (virtualized out) - ask the caller to scroll it into range, then focus it once
      // its ref callback attaches it below.
      if (scrollToKey) {
        pendingFocusKeyRef.current = key;
        scrollToKey(key);
      }
      return;
    }
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
    setLastIndex(index);
    setCursorKey(key);
    scrollAndFocus(key);
    announce(key);
    if (extend) onExtend?.(key);
  }

  // A group expanded by ArrowRight: its rows only join `keys` on this render, so the move into it
  // has to wait for that render to land. `currentKeys` is passed in explicitly (rather than closed
  // over) so the effect keeps its real `[keys]` dependency instead of one the linter can't see.
  const focusExpandedGroup = useEffectEvent((currentKeys: string[]) => {
    const groupId = pendingExpandRef.current;
    if (groupId === null) return;
    pendingExpandRef.current = null;
    const firstInGroup = currentKeys.find(
      (key) => rowsRef.current.get(key)?.closest(`[data-group="${groupId}"]`) != null,
    );
    if (firstInGroup) moveTo(firstInGroup, false);
  });
  useEffect(() => {
    focusExpandedGroup(keys);
  }, [keys]);

  // Focuses the seeded row whenever the caller's view becomes active, after its ref has attached -
  // this is what returns focus to a row when a kept-alive list reappears from behind a skill page
  // that just closed, with `initialKey` set to that skill's name. `initialKey` is read through the
  // effect event so it re-runs only on activation, not on every `initialKey` change within one.
  const focusSeededRow = useEffectEvent(() => {
    if (initialKey) scrollAndFocus(initialKey);
  });
  useEffect(() => {
    if (!active) return;
    const frame = requestAnimationFrame(() => focusSeededRow());
    return () => cancelAnimationFrame(frame);
  }, [active]);

  function groupIdFor(rowEl: HTMLElement): string | null {
    return rowEl.closest("[data-group]")?.getAttribute("data-group") ?? null;
  }

  function focusCursor() {
    scrollAndFocus(effectiveCursorKey ?? keys[0]);
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

    if (effectiveCursorKey === null) return;
    const cursorKey = effectiveCursorKey;
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
    cursorKey: effectiveCursorKey,
    rowRef: (key) => (el) => {
      if (el) {
        rowsRef.current.set(key, el);
        if (pendingFocusKeyRef.current === key) {
          pendingFocusKeyRef.current = null;
          el.focus({ preventScroll: true });
        }
      } else rowsRef.current.delete(key);
    },
    containerRef: (el) => {
      gridRef.current = el;
    },
    tabIndexFor: (key) => (key === effectiveCursorKey ? 0 : -1),
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
