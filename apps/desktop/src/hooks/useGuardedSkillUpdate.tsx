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

/**
 * `requestUpdate(skill, overwrite)` runs `overwrite` right away when the skill
 * has no local edits, and otherwise opens the dialog. `overwrite` is the
 * caller's plain update. Render `dialog` once next to the buttons.
 */
export function useGuardedSkillUpdate() {
  const addToast = useAppStore((state) => state.addToast);
  const [pending, setPending] = useState<PendingUpdate | null>(null);
  const [isResolving, setIsResolving] = useState(false);

  const requestUpdate = async (
    skill: InstalledSkill,
    overwrite: () => Promise<void>,
    scopeTarget?: LifecycleTarget,
  ) => {
    const edited = await skillsWithLocalEdits(
      [skill],
      skillLocalEdits,
      scopeTarget ? () => [scopeTarget] : undefined,
    );
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
    } finally {
      setIsResolving(false);
    }
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
