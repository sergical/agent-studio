// ============================================================================
// skill-list-model - Sorts a skill list and buckets it into SkillListTable's
// three state groups (attention, healthy, parked), in display order. The one
// pure computation between a SkillSnapshot's skills and what the Skills list
// renders.
// ============================================================================

import type { InstalledSkill, SkillInvocationStats } from "@skill-studio/lib";
import type { SortMode } from "./skill-list-sort";
import { sortRows } from "../components/SkillList/skill-row-format";
import { rowGroup, rowState } from "../components/SkillList/skill-row-state";
import type { RowGroup, RowState } from "../components/SkillList/skill-row-state";

export interface GroupedSkillRows {
  buckets: Record<RowGroup, InstalledSkill[]>;
  statesBySkill: Map<string, RowState | null>;
  /** The grouped display order: shift-click and `aria-rowindex` refer to this array, not the
   * caller's original `skills` order. */
  rows: InstalledSkill[];
}

export function groupSkillRows(
  skills: InstalledSkill[],
  sort: SortMode,
  stats: SkillInvocationStats[],
): GroupedSkillRows {
  const statsBySkill = new Map(stats.map((s) => [s.skill, s]));
  const sorted = sortRows(skills, sort, statsBySkill);
  const statesBySkill = new Map<string, RowState | null>();
  // SAFETY: each bucket starts empty; the loop below only ever pushes `InstalledSkill` values into it.
  const buckets = {
    attention: [] as InstalledSkill[],
    healthy: [] as InstalledSkill[],
    parked: [] as InstalledSkill[],
  };
  for (const skill of sorted) {
    const state = rowState(skill);
    statesBySkill.set(skill.name, state);
    buckets[rowGroup(skill, state)].push(skill);
  }
  const rows = [...buckets.attention, ...buckets.healthy, ...buckets.parked];
  return { buckets, statesBySkill, rows };
}
