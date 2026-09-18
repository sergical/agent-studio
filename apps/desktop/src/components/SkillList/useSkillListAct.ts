// ============================================================================
// useSkillListAct - Row action dispatch for SkillListTable: Park/Unpark and
// Fix act on the deployment target `HomeView` uses, every other fix (Fix
// link, Compare, Convert, Keep, Pull latest) opens the skill's own detail,
// since those flows live there. Fix now covers the repairable spec
// violations too (invalid YAML frontmatter); any issue `fix_skill` leaves
// unrepaired falls through to the detail page, which has its own card for
// a link issue and shows the raw message otherwise.
// ============================================================================

import { useAppStore } from "../../store/appStore";
import { fixSkill, openConflictPaths, parkSkill, unparkSkill } from "../../lib/skill-api";
import { lifecycleTargetForPark } from "../../lib/skill-lifecycle-target";
import type { InstalledSkill, Toast } from "@skill-studio/lib";

/** The two IPC calls `reportFixOutcome` makes, as a real interface rather
 * than an import a test would have to mock: production code gets the
 * default `skill-api.ts` wrappers below, a test hands in its own faithful
 * implementation. */
export interface FixSkillDeps {
  fixSkill: typeof fixSkill;
  openConflictPaths: typeof openConflictPaths;
}

const defaultFixSkillDeps: FixSkillDeps = { fixSkill, openConflictPaths };

/** Reports what `fix_skill` actually did as a toast, and opens the skill's
 * detail page whenever an issue is left unrepaired: a link issue lands on
 * the `SkillRepairCard` that already knows how to fix it, and every other
 * unrepaired issue (a spec violation `fix_skill` has no repair for, a
 * frontmatter repair that failed to apply) lands on the same page so the
 * user can edit the file directly instead of getting a dead-end toast. A
 * conflict opens the editor rather than writing anything, so it gets its
 * own message. */
export async function reportFixOutcome(
  skill: InstalledSkill,
  addToast: (toast: Omit<Toast, "id">) => string,
  openDetail: () => void,
  deps: FixSkillDeps = defaultFixSkillDeps,
): Promise<void> {
  const outcome = await deps.fixSkill(skill.name);
  if (outcome.conflicts.length > 0) {
    const first = outcome.conflicts[0];
    await deps.openConflictPaths([first.path_a, first.path_b]);
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
  if (outcome.unrepaired.length > 0) {
    openDetail();
    return;
  }
  addToast({
    type: "error",
    title: `Couldn't fix ${skill.name}`,
    message: "Nothing to repair.",
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
        await reportFixOutcome(skill, addToast, () =>
          onSelectSkill(skill.name, deploymentPathForSkill?.(skill)),
        );
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
