// ============================================================================
// useSkillListSelection - Row selection state for SkillListTable: which
// deployments are checked, keeping the store's "selection mode" mirroring
// "at least one row checked", and shift-click range selection.
// ============================================================================

import { useRef } from "react";
import type { InstalledSkill } from "@skill-studio/lib";
import { useAppStore } from "../../store/appStore";

export interface UseSkillListSelection {
  selectedPaths: Set<string>;
  clearSkillSelection: () => void;
  selectSkills: (paths: string[]) => void;
  exitSelectionMode: () => void;
  syncSelectionMode: (nextSize: number) => void;
  handleRowCheckboxClick: (index: number, shiftKey: boolean) => void;
}

export function useSkillListSelection(
  rows: InstalledSkill[],
  rowPath: (skill: InstalledSkill) => string | undefined,
): UseSkillListSelection {
  const selectedPaths = useAppStore((state) => state.selectedSkillPaths);
  const toggleSkillSelection = useAppStore((state) => state.toggleSkillSelection);
  const clearSkillSelection = useAppStore((state) => state.clearSkillSelection);
  const selectSkills = useAppStore((state) => state.selectSkills);
  const selectionMode = useAppStore((state) => state.selectionMode);
  const enterSelectionMode = useAppStore((state) => state.enterSelectionMode);
  const exitSelectionMode = useAppStore((state) => state.exitSelectionMode);
  /** Index of the last row checked by click (not shift-click), for shift-click range-select. */
  const lastCheckedIndexRef = useRef<number | null>(null);

  /** The store's `selectionMode` mirrors "at least one row checked" - kept in sync here since a
   * checkbox now drives selection directly instead of a separate mode switch. */
  function syncSelectionMode(nextSize: number) {
    if (nextSize > 0 && !selectionMode) enterSelectionMode();
    else if (nextSize === 0 && selectionMode) exitSelectionMode();
  }

  /** Checkbox click for one row - shift-click selects every row between it and the last clicked one, in visible (grouped) order. */
  function handleRowCheckboxClick(index: number, shiftKey: boolean) {
    if (shiftKey && lastCheckedIndexRef.current !== null) {
      const [from, to] = [lastCheckedIndexRef.current, index].sort((a, b) => a - b);
      const range = rows.slice(from, to + 1).map((s) => rowPath(s));
      const next = new Set(selectedPaths);
      range.forEach((path) => path && next.add(path));
      selectSkills([...next]);
      syncSelectionMode(next.size);
    } else {
      const path = rowPath(rows[index]);
      if (path) {
        const next = new Set(selectedPaths);
        if (next.has(path)) next.delete(path);
        else next.add(path);
        toggleSkillSelection(path);
        syncSelectionMode(next.size);
      }
    }
    lastCheckedIndexRef.current = index;
  }

  return {
    selectedPaths,
    clearSkillSelection,
    selectSkills,
    exitSelectionMode,
    syncSelectionMode,
    handleRowCheckboxClick,
  };
}
