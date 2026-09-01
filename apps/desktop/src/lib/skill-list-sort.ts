// ============================================================================
// skill-list-sort - The Skills list's sort mode: the type, the Sort select's
// items (rendered by SkillListFilterBar), and the value guard (applied by
// SkillListTable). Split out of SkillListTable.tsx so that file only exports
// the component (Fast Refresh).
// ============================================================================

export type SortMode = "name" | "used" | "size";

/** The Sort select's items - rendered by `SkillListFilterBar`, applied by `SkillListTable`. */
export const SORT_ITEMS: { value: SortMode; label: string }[] = [
  { value: "name", label: "Name" },
  { value: "used", label: "Most used" },
  { value: "size", label: "Largest" },
];

export function isSortMode(value: string): value is SortMode {
  return value === "name" || value === "used" || value === "size";
}
