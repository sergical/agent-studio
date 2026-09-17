// ============================================================================
// useSkillListScrollMargin - Finds the ancestor scroll container SkillListTable's
// virtualizer measures against, and keeps `scrollMargin` (the grid's offset
// from the top of that container's content) in sync with whatever sits above
// the grid - filter chips, a partial-scan banner.
// ============================================================================

import { useEffect, useRef, useState } from "react";

export interface UseSkillListScrollMargin {
  scrollElement: HTMLElement | null;
  scrollMargin: number;
  /** Resolves the ancestor scroll container once the grid element attaches - `PageShell`'s
   * `overflow-y-auto` content area, an ancestor of the grid rather than the grid itself. */
  setGridElement: (el: HTMLDivElement | null) => void;
}

export function useSkillListScrollMargin(): UseSkillListScrollMargin {
  /** The scroll container the virtualizer measures against - the nearest ancestor with its own
   * scrollbar (`PageShell`'s content area), found by walking up from the grid element. */
  const [scrollElement, setScrollElement] = useState<HTMLElement | null>(null);
  const gridElRef = useRef<HTMLDivElement | null>(null);
  /** How far the grid sits below the top of the scroll container's content - `useVirtualizer`'s
   * `scrollMargin`, recomputed whenever content above the grid (filter chips, a scan banner)
   * resizes. */
  const [scrollMargin, setScrollMargin] = useState(0);

  function setGridElement(el: HTMLDivElement | null) {
    gridElRef.current = el;
    if (!el) return;
    let node: HTMLElement | null = el.parentElement;
    while (node) {
      const { overflowY } = window.getComputedStyle(node);
      if (overflowY === "auto" || overflowY === "scroll") {
        setScrollElement(node);
        return;
      }
      node = node.parentElement;
    }
  }

  useEffect(() => {
    if (!scrollElement) return;
    function recompute() {
      const gridEl = gridElRef.current;
      if (!gridEl || !scrollElement) return;
      const gridRect = gridEl.getBoundingClientRect();
      // The grid is `hidden` behind an open skill's page - keep the last real margin instead of
      // collapsing it to 0.
      if (gridRect.width === 0 && gridRect.height === 0) return;
      const scrollRect = scrollElement.getBoundingClientRect();
      setScrollMargin(gridRect.top - scrollRect.top + scrollElement.scrollTop);
    }
    recompute();
    const contentEl = scrollElement.firstElementChild;
    if (!contentEl) return;
    const observer = new ResizeObserver(recompute);
    observer.observe(contentEl);
    return () => observer.disconnect();
  }, [scrollElement]);

  return { scrollElement, scrollMargin, setGridElement };
}
