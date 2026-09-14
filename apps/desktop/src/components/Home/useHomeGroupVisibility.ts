// ============================================================================
// useHomeGroupVisibility - Home's stat-tile filter and per-group collapse
// state, plus the visible/expanded checks every group and the row-cursor
// wiring read off them.
// ============================================================================

import { useState } from "react";
import type { GroupId, HomeFilter } from "./home-inbox-data";

/**
 * Owns Home's one-at-a-time stat-tile filter and its groups' collapsed set,
 * and derives `isGroupVisible`/`isGroupExpanded` from them - pulled out of
 * `HomeView` since every piece here reads the same two pieces of state.
 */
export function useHomeGroupVisibility() {
  const [filter, setFilter] = useState<HomeFilter | null>(null);
  // "Not used in the last 30 days" starts collapsed - the other groups need attention right away.
  const [collapsedGroups, setCollapsedGroups] = useState<Set<GroupId>>(() => new Set(["unused"]));

  const toggleFilter = (id: HomeFilter) => setFilter((cur) => (cur === id ? null : id));
  const isGroupVisible = (id: GroupId) => filter === null || filter === id;
  const isGroupExpanded = (id: GroupId) => !collapsedGroups.has(id) || filter === id;
  const toggleGroup = (id: GroupId) =>
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return {
    filter,
    setFilter,
    toggleFilter,
    collapsedGroups,
    setCollapsedGroups,
    isGroupVisible,
    isGroupExpanded,
    toggleGroup,
  };
}
