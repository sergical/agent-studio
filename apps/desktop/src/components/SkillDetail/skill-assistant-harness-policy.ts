// ============================================================================
// Skill Studio - assistant harness policy
// Keeps the Assistant runner limited to backend-supported harnesses while
// reporting whether each harness can see the installed skill.
// ============================================================================

import { skillVisibleToAgent } from "@skill-studio/lib";
import type { HarnessId, InstalledSkill } from "@skill-studio/lib";
import { HARNESS_LABELS } from "../../lib/harness-labels";

export interface SkillAssistantHarnessPolicy {
  defaultHarness: HarnessId;
  items: { value: HarnessId; label: string }[];
}

/** Builds supported Assistant choices and a truthful default for one installed skill. */
export function skillAssistantHarnessPolicy(skill: InstalledSkill): SkillAssistantHarnessPolicy {
  const visibleHarnesses = new Set<HarnessId>();
  const items = HARNESS_LABELS.map(([value, label]) => {
    const isVisible = skillVisibleToAgent(skill, value) !== "none";
    if (isVisible) visibleHarnesses.add(value);
    return {
      value,
      label: isVisible ? label : `${label} (doesn't see this skill)`,
    };
  });

  const defaultHarness = visibleHarnesses.has("claude-code")
    ? "claude-code"
    : (HARNESS_LABELS.find(([value]) => visibleHarnesses.has(value))?.[0] ?? "claude-code");

  return { defaultHarness, items };
}

/** Narrows a select-control value to a harness supported by the Assistant backend. */
export function isSkillAssistantHarness(value: string): value is HarnessId {
  return HARNESS_LABELS.some(([harness]) => harness === value);
}
