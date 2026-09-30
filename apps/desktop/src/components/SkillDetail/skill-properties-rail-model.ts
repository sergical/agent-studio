// ============================================================================
// Skill Studio - properties rail harness model
// Which harnesses the rail's Harnesses popover lists, which location row each
// switch drives, and the action a reader row's switch sends.
// ============================================================================

import type { AgentId, InstalledSkill } from "@skill-studio/lib";
import { DEFAULT_HARNESS_LIST, whereFacts } from "../SkillList/skill-row-state";
import type { AgentLocationRow, LocationAction, ScopeGroup } from "./skill-location-status";

interface RailHarnessEntry {
  harness: AgentId;
  label: string;
  row: AgentLocationRow | null;
}

function rowRank(row: AgentLocationRow, isGlobal: boolean): number {
  return (isGlobal ? 0 : 2) + (row.hasSwitch ? 0 : 1);
}

/** The Global row wins over a project row, so a project copy never drives the switch that stands for the Global state. */
function rowForHarness(groups: ScopeGroup[], harness: AgentId): AgentLocationRow | null {
  let best: { row: AgentLocationRow; rank: number } | null = null;
  for (const group of groups) {
    for (const row of group.rows) {
      if (row.kind === "shared" || row.harness !== harness) continue;
      const rank = rowRank(row, group.isGlobal);
      if (!best || rank < best.rank) best = { row, rank };
    }
  }
  return best?.row ?? null;
}

/** Harnesses with their own deployment, plus the ones that read the skill from the Universal folder. */
export function railHarnessEntries(
  skill: InstalledSkill,
  groups: ScopeGroup[],
): RailHarnessEntry[] {
  return whereFacts(skill, DEFAULT_HARNESS_LIST).harnesses.flatMap((h) => {
    const row = rowForHarness(groups, h.harness);
    return h.reached || row ? [{ harness: h.harness, label: h.label, row }] : [];
  });
}

export function readerToggleAction(
  row: AgentLocationRow,
  enabled: boolean,
): Extract<LocationAction, { kind: "set-reader-enabled" }> | null {
  if (row.kind !== "reader") return null;
  return { kind: "set-reader-enabled", target: row.lifecycleTarget, agent: row.harness, enabled };
}
