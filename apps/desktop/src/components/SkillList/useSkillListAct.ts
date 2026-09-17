// ============================================================================
// useSkillListAct - Row action dispatch for SkillListTable: Park/Unpark act
// on the deployment target `HomeView` uses, every other fix (Fix YAML, Fix
// link, Compare, Convert, Keep, Pull latest) opens the skill's own detail,
// since those flows live there.
// ============================================================================

import { useAppStore } from "../../store/appStore";
import { parkSkill, unparkSkill } from "../../lib/skill-api";
import { lifecycleTargetForPark } from "../../lib/skill-lifecycle-target";
import type { InstalledSkill } from "@skill-studio/lib";

export function useSkillListAct(
  onSelectSkill: (name: string, deploymentPath?: string) => void,
  deploymentPathForSkill: ((skill: InstalledSkill) => string | undefined) | undefined,
): (label: string, skill: InstalledSkill) => Promise<void> {
  const addToast = useAppStore((state) => state.addToast);

  return async function handleAct(label: string, skill: InstalledSkill) {
    if (label !== "Park" && label !== "Unpark") {
      onSelectSkill(skill.name, deploymentPathForSkill?.(skill));
      return;
    }
    // Hoisted out of the try/catch below - the compiler can't optimize a conditional expression
    // computed inside a try/catch statement.
    const successTitle = label === "Park" ? `Parked ${skill.name}` : `Unparked ${skill.name}`;
    const failureTitle = label === "Park" ? "Couldn't park skill" : "Couldn't unpark skill";
    try {
      if (label === "Park") await parkSkill(lifecycleTargetForPark(skill));
      else await unparkSkill(lifecycleTargetForPark(skill));
      addToast({ type: "success", title: successTitle });
    } catch (err) {
      addToast({
        type: "error",
        title: failureTitle,
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };
}
