// ============================================================================
// useSkillBulkActions - Runs one bulk action from the selection bar: park,
// unpark and invocation go to the backend as one batched call, remove goes
// skill by skill, progress shows while it runs, and one toast reports the
// outcome. The selection stays only when a skill failed, so the user can retry. The skill list refreshes from the backend's snapshot
// event, as after a single-row action.
// ============================================================================

import { useState } from "react";
import type { InstalledSkill } from "@skill-studio/lib";
import { removeSkill, skillLocalEdits } from "../../lib/skill-api";
import { runListUpdate } from "../../hooks/skillBatchUpdates";
import { forkableDeployment, skillsWithLocalEdits } from "../../lib/skill-lifecycle-target";
import { useAppStore } from "../../store/appStore";
import type { UpdatePrompt } from "../SkillDetail/UpdateOverwritesEditsDialog";
import {
  bulkActionToast,
  bulkProgressLabel,
  bulkUpdateProgressLabel,
  bulkRemovalTargets,
  planBulkAction,
  runBulkSequentially,
} from "./skill-bulk-actions";
import type { BulkAction, BulkPlan, BulkRunResult } from "./skill-bulk-actions";
import { runBatchAction } from "./skill-bulk-run";

async function runRemoval(skill: InstalledSkill): Promise<void> {
  const targets = bulkRemovalTargets(skill);
  if (targets.length === 0) throw new Error("No removable copy.");
  for (const target of targets) {
    // react-doctor-disable-next-line react-doctor/async-await-in-loop -- each core op takes an exclusive lease, so the calls must not overlap
    await removeSkill(target);
  }
}

interface UseSkillBulkActions {
  /** "Parking 5 skills…" while an action runs, otherwise `null`. */
  progress: string | null;
  run: (action: BulkAction, skills: InstalledSkill[]) => Promise<void>;
  /** Confirm step before an update replaces local edits; `null` while none is pending. */
  updatePrompt: UpdatePrompt | null;
}

interface PendingEditedUpdate {
  plan: BulkPlan;
  edited: InstalledSkill[];
}

/** `onFinished(hadFailures)` lets the caller clear the selection unless something failed. */
export function useSkillBulkActions(
  onFinished: (hadFailures: boolean) => void,
): UseSkillBulkActions {
  const addToast = useAppStore((state) => state.addToast);
  const [progress, setProgress] = useState<string | null>(null);

  const [pending, setPending] = useState<PendingEditedUpdate | null>(null);

  const execute = async (
    action: BulkAction,
    plan: BulkPlan,
    forkNames: ReadonlySet<string> = new Set(),
  ) => {
    setProgress(bulkProgressLabel(action, 1, plan.applicable.length));
    let result: BulkRunResult;
    try {
      if (action.kind === "update")
        result = await runListUpdate(plan.applicable, forkNames, (done, total) =>
          setProgress(bulkUpdateProgressLabel(done, total)),
        );
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

  const run = async (action: BulkAction, skills: InstalledSkill[]) => {
    const plan = planBulkAction(skills, action);
    if (plan.applicable.length === 0) return;
    if (action.kind === "update") {
      setProgress(bulkProgressLabel(action, 1, plan.applicable.length));
      // `skillsWithLocalEdits` treats a failed check as "no edits", so it never rejects.
      const edited = await skillsWithLocalEdits(plan.applicable, skillLocalEdits);
      if (edited.length > 0) {
        setProgress(null);
        setPending({ plan, edited });
        return;
      }
    }
    await execute(action, plan);
  };

  const updateAction: BulkAction = { kind: "update" };
  const updatePrompt: UpdatePrompt | null = pending && {
    skillNames: pending.edited.map((skill) => skill.name),
    canFork: pending.edited.every((skill) => forkableDeployment(skill) !== undefined),
    fork: () => {
      setPending(null);
      void execute(updateAction, pending.plan, new Set(pending.edited.map((skill) => skill.name)));
    },
    overwrite: () => {
      setPending(null);
      void execute(updateAction, pending.plan);
    },
    cancel: () => setPending(null),
  };

  return { progress, run, updatePrompt };
}
