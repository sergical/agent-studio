// ============================================================================
// useSkillListAct - Row action dispatch for SkillListTable: Park/Unpark and
// Fix act on the deployment target `HomeView` uses, every other fix (Fix
// YAML, Fix link, Compare, Convert, Keep, Pull latest) opens the skill's own
// detail, since those flows live there.
// ============================================================================

import { useAppStore } from "../../store/appStore";
import { fixSkill, openConflictPaths, parkSkill, unparkSkill } from "../../lib/skill-api";
import { lifecycleTargetForPark } from "../../lib/skill-lifecycle-target";
import type { InstalledSkill, Toast } from "@skill-studio/lib";

/** Reports what `fix_skill` actually did as a toast: a conflict opens the
 * editor rather than writing anything, so it gets its own message. */
async function reportFixOutcome(
  skill: InstalledSkill,
  addToast: (toast: Omit<Toast, "id">) => string,
): Promise<void> {
  const outcome = await fixSkill(skill.name);
  if (outcome.conflicts.length > 0) {
    const first = outcome.conflicts[0];
    await openConflictPaths([first.path_a, first.path_b]);
    addToast({
      type: "info",
      title: `${skill.name} has a conflict`,
      message: `${first.message} Opened both copies in your editor.`,
    });
    return;
  }
  if (outcome.applied.length > 0) {
    addToast({ type: "success", title: `Fixed ${skill.name}` });
    return;
  }
  const [first] = outcome.unrepaired;
  addToast({
    type: "error",
    title: `Couldn't fix ${skill.name}`,
    message: first?.message ?? "Nothing to repair.",
  });
}

export function useSkillListAct(
  onSelectSkill: (name: string, deploymentPath?: string) => void,
  deploymentPathForSkill: ((skill: InstalledSkill) => string | undefined) | undefined,
): (label: string, skill: InstalledSkill) => Promise<void> {
  const addToast = useAppStore((state) => state.addToast);

  return async function handleAct(label: string, skill: InstalledSkill) {
    if (label === "Fix") {
      try {
        await reportFixOutcome(skill, addToast);
      } catch (err) {
        addToast({
          type: "error",
          title: `Couldn't fix ${skill.name}`,
          message: err instanceof Error ? err.message : "Unknown error",
        });
      }
      return;
    }
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
