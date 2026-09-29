// ============================================================================
// useSkillBulkActions - Runs one bulk action from the selection bar: the
// per-skill calls go one after another, progress shows while it runs, and one
// toast reports the outcome. The selection stays only when a skill failed, so
// the user can retry. The skill list refreshes from the backend's snapshot
// event, as after a single-row action.
// ============================================================================

import { useState } from "react";
import type { InstalledSkill } from "@skill-studio/lib";
import { parkSkill, removeSkill, unparkSkill, updateAllSkills } from "../../lib/skill-api";
import { lifecycleTargetForPark } from "../../lib/skill-lifecycle-target";
import { useAppStore } from "../../store/appStore";
import { setInvocationForFile } from "../SkillDetail/skill-location-actions";
import { invocationFilesForSkill } from "../SkillDetail/skill-location-status";
import {
  bulkActionToast,
  bulkProgressLabel,
  bulkRemovalTarget,
  bulkUpdateResult,
  bulkUpdateTarget,
  planBulkAction,
  runBulkSequentially,
} from "./skill-bulk-actions";
import type { BulkAction, BulkRunResult } from "./skill-bulk-actions";

async function runPerSkill(action: BulkAction, skill: InstalledSkill): Promise<void> {
  switch (action.kind) {
    case "park":
      return parkSkill(lifecycleTargetForPark(skill));
    case "unpark":
      return unparkSkill(lifecycleTargetForPark(skill));
    case "invocation": {
      for (const file of invocationFilesForSkill(skill)) {
        if (!file.editable) continue;
        // react-doctor-disable-next-line react-doctor/async-await-in-loop -- each core op takes an exclusive lease, so the writes must not overlap
        await setInvocationForFile(skill, file, action.policy);
      }
      return;
    }
    case "remove": {
      const target = bulkRemovalTarget(skill);
      if (!target) throw new Error("No removable copy.");
      await removeSkill(target);
      return;
    }
    case "update":
      throw new Error("Update runs as one batch.");
  }
}

async function runUpdateBatch(skills: InstalledSkill[]): Promise<BulkRunResult> {
  const targets = skills.flatMap((skill) => bulkUpdateTarget(skill) ?? []);
  return bulkUpdateResult(skills, await updateAllSkills(targets));
}

interface UseSkillBulkActions {
  /** "Parking 2 of 5…" while an action runs, otherwise `null`. */
  progress: string | null;
  run: (action: BulkAction, skills: InstalledSkill[]) => Promise<void>;
}

/** `onFinished(hadFailures)` lets the caller clear the selection unless something failed. */
export function useSkillBulkActions(
  onFinished: (hadFailures: boolean) => void,
): UseSkillBulkActions {
  const addToast = useAppStore((state) => state.addToast);
  const [progress, setProgress] = useState<string | null>(null);

  const run = async (action: BulkAction, skills: InstalledSkill[]) => {
    const plan = planBulkAction(skills, action);
    if (plan.applicable.length === 0) return;
    setProgress(bulkProgressLabel(action, 1, plan.applicable.length));
    let result: BulkRunResult;
    try {
      result =
        action.kind === "update"
          ? await runUpdateBatch(plan.applicable)
          : await runBulkSequentially(
              plan.applicable,
              (skill) => runPerSkill(action, skill),
              (current, total) => setProgress(bulkProgressLabel(action, current, total)),
            );
    } catch (error) {
      // Only the batched update can reject as a whole; every skill in it failed.
      const message = error instanceof Error ? error.message : "Unknown error";
      result = {
        succeeded: [],
        failed: plan.applicable.map((skill) => ({ skill, error: message })),
      };
    }
    setProgress(null);
    addToast(bulkActionToast(action, plan, result));
    onFinished(result.failed.length > 0);
  };

  return { progress, run };
}
