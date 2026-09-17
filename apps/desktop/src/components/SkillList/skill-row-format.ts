// ============================================================================
// skill-row-format - non-component helpers shared by the row cells: the
// selected-row treatment and the table's sort order. Split out of
// SkillRowCells.tsx so that file exports components only (react-refresh).
// ============================================================================

import type { InstalledSkill, SkillInvocationStats } from "@skill-studio/lib";
import type { SortMode } from "../../lib/skill-list-sort";

/** Selected-row treatment shared by the row: an accent border, softer fill, and a left accent bar. */
export function selectedRowClass(selected: boolean): string {
  return selected
    ? "border-accent bg-accent-softer shadow-[inset_2px_0_0_var(--color-accent)]"
    : "";
}

/** Rows in the table's sort order: `name` and `used` order as the Sort select says; `size`
 * (the "largest" option) orders by the full SKILL.md token count, ties broken by name. */
export function sortRows(
  skills: InstalledSkill[],
  sort: SortMode,
  statsBySkill: Map<string, SkillInvocationStats>,
): InstalledSkill[] {
  const rows = [...skills];
  if (sort === "name") {
    rows.sort((a, b) => a.name.localeCompare(b.name));
  } else if (sort === "used") {
    rows.sort(
      (a, b) =>
        (statsBySkill.get(b.name)?.last_30_days ?? 0) -
        (statsBySkill.get(a.name)?.last_30_days ?? 0),
    );
  } else {
    rows.sort((a, b) => b.skill_md_tokens - a.skill_md_tokens || a.name.localeCompare(b.name));
  }
  return rows;
}
