// ============================================================================
// skillBatchUpdates - the batch update commands, wired to the IPC layer. Both
// callers run `skillsWithLocalEdits` and the edits dialog first.
// ============================================================================

import { forkSkill, pullForkUpstream, updateAllSkillsWithProgress } from "../lib/skill-api";
import { updateAllOutdatedSkills } from "../components/Home/home-inbox-data";
import { runBulkUpdate } from "../components/SkillList/skill-bulk-actions";
import type { InstalledSkill } from "@skill-studio/lib";

/** Home's "Update all"; `forkNames` are the edited skills to fork and merge instead of overwrite. */
export function runHomeUpdateAll(
  updates: InstalledSkill[],
  onProgress: (done: number, total: number) => void,
  forkNames?: ReadonlySet<string>,
) {
  return updateAllOutdatedSkills(
    updates,
    pullForkUpstream,
    (targets, onOwnerDone) => updateAllSkillsWithProgress(targets, ({ done }) => onOwnerDone(done)),
    onProgress,
    forkNames && { names: forkNames, fork: forkSkill },
  );
}

/** The list's bulk Update; `forkNames` as for `runHomeUpdateAll`. */
export function runListUpdate(
  skills: InstalledSkill[],
  forkNames: ReadonlySet<string>,
  onProgress: (done: number, total: number) => void,
) {
  return runBulkUpdate(
    skills,
    forkNames,
    {
      fork: forkSkill,
      pullFork: pullForkUpstream,
      updateAll: (targets, onUpdateProgress) =>
        updateAllSkillsWithProgress(targets, ({ done, total }) => onUpdateProgress(done, total)),
    },
    onProgress,
  );
}
