// ============================================================================
// useHomeRowCursor - Wires Home's inbox rows into the shared roving-cursor
// hook: seeds it on the just-closed skill's row, resolves a row's key to its
// open action, and keeps window-level j/k/arrow entry working.
// ============================================================================

import { useRowCursor, useRowCursorWindowEntry } from "../../hooks/useRowCursor";
import type { RowCursor } from "../../hooks/useRowCursor";
import { useAppStore } from "../../store/appStore";
import type { GroupId } from "./home-inbox-data";

interface UseHomeRowCursorParams {
  visibleKeys: string[];
  openByKey: Map<string, () => void>;
  active: boolean;
  setCollapsedGroups: (updater: (prev: Set<GroupId>) => Set<GroupId>) => void;
}

/**
 * Home's `useRowCursor` wiring - pulled out of `HomeView` since every piece here (the initial-key
 * lookup, the group-id casts) closes over the same `visibleKeys`/`openByKey` this render computed.
 */
export function useHomeRowCursor({
  visibleKeys,
  openByKey,
  active,
  setCollapsedGroups,
}: UseHomeRowCursorParams): RowCursor {
  // Home's keys are namespaced by group (e.g. `"rec:some-skill"`), so the just-closed skill's
  // plain name is matched as one `:`-delimited segment, not the whole key.
  const lastClosedSkillName = useAppStore((state) => state.lastClosedSkillName);
  const initialCursorKey = lastClosedSkillName
    ? visibleKeys.find((key) => key.split(":").includes(lastClosedSkillName))
    : undefined;

  const cursor = useRowCursor({
    keys: visibleKeys,
    initialKey: initialCursorKey,
    active,
    onOpen: (key) => openByKey.get(key)?.(),
    onCollapseGroup: (groupId) =>
      setCollapsedGroups((prev) =>
        // SAFETY: `groupId` only ever comes from this file's own `data-group` attributes, which
        // are always one of the five `GroupId` values.
        new Set(prev).add(groupId as GroupId),
      ),
    onExpandGroup: (groupId) =>
      setCollapsedGroups((prev) => {
        const next = new Set(prev);
        // SAFETY: `groupId` only ever comes from this file's own `data-group` attributes, which
        // are always one of the five `GroupId` values.
        next.delete(groupId as GroupId);
        return next;
      }),
  });
  useRowCursorWindowEntry(active, cursor.focusCursor);

  return cursor;
}
