// ============================================================================
// useGuardedSkillUpdate - the one entry point for updating a single skill.
// Every single-skill Update button goes through `requestUpdate`, so an update
// that would replace local edits to a skills.sh skill always asks first:
// fork and merge, overwrite, or cancel.
// ============================================================================

import { useState } from "react";
import { forkSkill, pullForkUpstream, skillLocalEdits, updateSkill } from "../lib/skill-api";
import {
  forkEditedAndUpdate,
  forkableDeployment,
  pullUpstreamToast,
  skillUpdateToast,
  skillsWithLocalEdits,
  updateSkillOwners,
} from "../lib/skill-lifecycle-target";
import type { InstalledSkill, LifecycleTarget } from "@skill-studio/lib";
import { useAppStore } from "../store/appStore";
import { UpdateOverwritesEditsDialog } from "../components/SkillDetail/UpdateOverwritesEditsDialog";

function canForkPending({ skill, scopeTarget }: PendingUpdate): boolean {
  const deployment = forkableDeployment(skill);
  if (!deployment) return false;
  return !scopeTarget || scopeTarget.owner_id === deployment.owner_id;
}

interface PendingUpdate {
  skill: InstalledSkill;
  overwrite: () => Promise<void>;
  /** Set when the update covers one owner only; the fork then replaces that owner alone. */
  scopeTarget?: LifecycleTarget;
}

/** How a single-owner update ended, for callers that report it their own way. */
export interface UpdateFinish {
  success: boolean;
  error?: string;
}

interface UpdateRequestOptions {
  /** Update this one owner instead of every owner of the skill. */
  scopeTarget?: LifecycleTarget;
  /** Called with the outcome of a `scopeTarget` update; without it the hook toasts. */
  onFinished?: (finish: UpdateFinish) => void;
}

/**
 * `requestUpdate(skill, options)` updates the skill right away when it has no
 * local edits, and otherwise opens the dialog. The hook owns the update
 * commands, so no caller can reach them without the check. Render `dialog`
 * once next to the buttons.
 */
export function useGuardedSkillUpdate() {
  const addToast = useAppStore((state) => state.addToast);
  const [pending, setPending] = useState<PendingUpdate | null>(null);
  const [isResolving, setIsResolving] = useState(false);

  const overwriteFor =
    (skill: InstalledSkill, { scopeTarget, onFinished }: UpdateRequestOptions) =>
    async () => {
      if (scopeTarget) {
        let finish: UpdateFinish;
        try {
          const result = await updateSkill(scopeTarget);
          finish = {
            success: result.success,
            error: result.error ?? "Update command failed without an error message.",
          };
        } catch (error) {
          finish = {
            success: false,
            error:
              error instanceof Error ? error.message : "Update failed without an error message.",
          };
        }
        onFinished?.(finish);
        return;
      }
      const summary = await updateSkillOwners(skill, updateSkill);
      addToast(skillUpdateToast(skill.name, summary));
    };

  const requestUpdate = async (skill: InstalledSkill, options: UpdateRequestOptions = {}) => {
    const { scopeTarget } = options;
    const edited = await skillsWithLocalEdits(
      [skill],
      skillLocalEdits,
      scopeTarget ? () => [scopeTarget] : undefined,
    );
    const overwrite = overwriteFor(skill, options);
    if (edited.length > 0) {
      setPending({ skill, overwrite, scopeTarget });
      return;
    }
    await overwrite();
  };

  const resolve = async (title: string, run: (update: PendingUpdate) => Promise<void>) => {
    const update = pending;
    setPending(null);
    if (!update) return;
    setIsResolving(true);
    try {
      await run(update);
    } catch (error) {
      addToast({
        type: "error",
        title,
        message: error instanceof Error ? error.message : "Unknown error",
      });
    }
    setIsResolving(false);
  };

  const overwrite = () => resolve("Update failed", (update) => update.overwrite());

  const forkAndUpdate = () =>
    resolve("Fork and update failed", async ({ skill, scopeTarget }) => {
      const { pull, others } = await forkEditedAndUpdate(
        skill,
        { fork: forkSkill, pullFork: pullForkUpstream, updateOwner: updateSkill },
        { updateOthers: scopeTarget === undefined },
      );
      addToast(pullUpstreamToast(pull));
      if (others.attempted > 0) addToast(skillUpdateToast(skill.name, others));
    });

  const dialog = (
    <UpdateOverwritesEditsDialog
      skillNames={pending ? [pending.skill.name] : []}
      isBulk={false}
      canFork={pending !== null && canForkPending(pending)}
      onFork={() => void forkAndUpdate()}
      onOverwrite={() => void overwrite()}
      onCancel={() => setPending(null)}
    />
  );

  return { requestUpdate, isResolving, dialog };
}
