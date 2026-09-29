// ============================================================================
// useSkillBulkActions - Runs one bulk action from the selection bar: park,
// unpark and invocation go to the backend as one batched call, remove goes
// skill by skill, progress shows while it runs, and one toast reports the
// outcome. The selection stays only when a skill failed, so the user can retry. The skill list refreshes from the backend's snapshot
// event, as after a single-row action.
// ============================================================================

import { useState } from "react";
import type { InstalledSkill } from "@skill-studio/lib";
import { removeSkill, updateAllSkills } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
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
import { runBatchAction } from "./skill-bulk-run";

async function runRemoval(skill: InstalledSkill): Promise<void> {
  const target = bulkRemovalTarget(skill);
  if (!target) throw new Error("No removable copy.");
  await removeSkill(target);
}

async function runUpdateBatch(skills: InstalledSkill[]): Promise<BulkRunResult> {
  const targets = skills.flatMap((skill) => bulkUpdateTarget(skill) ?? []);
  return bulkUpdateResult(skills, await updateAllSkills(targets));
}

interface UseSkillBulkActions {
  /** "Parking 5 skills…" while an action runs, otherwise `null`. */
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
      if (action.kind === "update") result = await runUpdateBatch(plan.applicable);
      else if (action.kind === "remove")
        result = await runBulkSequentially(plan.applicable, runRemoval, (current, total) =>
          setProgress(bulkProgressLabel(action, current, total)),
        );
      else result = await runBatchAction(action, plan.applicable);
    } catch (error) {
      // A batched call can reject as a whole; every skill in it failed.
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
