// ============================================================================
// Skill Studio - Batched bulk actions
// Park, unpark and invocation each go to the backend as one call for the whole
// selection, so it can refresh the skill list once instead of once per skill.
// The per-target results map back onto the selected skills for the one toast.
// ============================================================================

import type { BulkTargetResult, InstalledSkill, InvocationTarget } from "@skill-studio/lib";
import { parkSkills, setSkillsInvocation, unparkSkills } from "../../lib/skill-api";
import { lifecycleTargetForPark } from "../../lib/skill-lifecycle-target";
import { forkBeforeInvocationEdit } from "../SkillDetail/skill-location-actions";
import { invocationFilesForSkill } from "../SkillDetail/skill-location-status";
import type { BulkAction, BulkRunResult } from "./skill-bulk-actions";

export interface BatchApi {
  parkSkills: typeof parkSkills;
  unparkSkills: typeof unparkSkills;
  setSkillsInvocation: typeof setSkillsInvocation;
  forkBeforeInvocationEdit: typeof forkBeforeInvocationEdit;
}

const realBatchApi: BatchApi = {
  parkSkills,
  unparkSkills,
  setSkillsInvocation,
  forkBeforeInvocationEdit,
};

/**
 * Folds per-target results into a run result. `owners[i]` is the selected
 * skill target `i` belongs to; a skill fails on its first failing target. A
 * skill in `earlyFailures` never reached the batch.
 */
function bulkBatchResult(
  skills: InstalledSkill[],
  owners: InstalledSkill[],
  results: BulkTargetResult[],
  earlyFailures: Map<InstalledSkill, string>,
): BulkRunResult {
  const errors = new Map(earlyFailures);
  for (const [index, owner] of owners.entries()) {
    const result = results[index];
    const error = result ? result.error : "The batch returned no result for this skill.";
    if (error !== null && !errors.has(owner)) errors.set(owner, error);
  }
  const outcome: BulkRunResult = { succeeded: [], failed: [] };
  for (const skill of skills) {
    const error = errors.get(skill);
    if (error === undefined) outcome.succeeded.push(skill);
    else outcome.failed.push({ skill, error });
  }
  return outcome;
}

async function runInvocationBatch(
  skills: InstalledSkill[],
  policy: Extract<BulkAction, { kind: "invocation" }>["policy"],
  api: BatchApi,
): Promise<BulkRunResult> {
  const targets: InvocationTarget[] = [];
  const owners: InstalledSkill[] = [];
  const earlyFailures = new Map<InstalledSkill, string>();
  for (const skill of skills) {
    const files = invocationFilesForSkill(skill).filter((file) => file.editable);
    try {
      for (const file of files) {
        // react-doctor-disable-next-line react-doctor/async-await-in-loop -- each fork takes an exclusive lease, so the forks must not overlap
        await api.forkBeforeInvocationEdit(file);
      }
    } catch (error) {
      earlyFailures.set(skill, error instanceof Error ? error.message : "Unknown error");
      continue;
    }
    for (const file of files) {
      targets.push({ name: skill.name, path: `${file.path}/SKILL.md` });
      owners.push(skill);
    }
  }
  const results = targets.length > 0 ? await api.setSkillsInvocation(targets, policy) : [];
  return bulkBatchResult(skills, owners, results, earlyFailures);
}

/** Runs park, unpark or invocation on `skills` as one backend call. */
export async function runBatchAction(
  action: Extract<BulkAction, { kind: "park" | "unpark" | "invocation" }>,
  skills: InstalledSkill[],
  api: BatchApi = realBatchApi,
): Promise<BulkRunResult> {
  if (action.kind === "invocation") return runInvocationBatch(skills, action.policy, api);
  const targets = skills.map((skill) => lifecycleTargetForPark(skill));
  const results =
    action.kind === "park" ? await api.parkSkills(targets) : await api.unparkSkills(targets);
  return bulkBatchResult(skills, skills, results, new Map());
}
