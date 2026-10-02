// ============================================================================
// useSkillPageActions - Every action the skill page's header offers (primary
// button, overflow menu, and the park/fork/remove flows that used to live in
// InstalledSkillHeader's icon row and SkillDetailActions): reveal, open in
// editor, copy path, park/unpark, fork/un-fork/pull upstream, update, and
// remove. One hook so the header stays a thin render of this state.
// ============================================================================

import { useState } from "react";
import type { ReactNode } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import {
  forkSkill,
  openSkillPath,
  parkSkill,
  pullForkUpstream,
  removeSkill,
  unforkSkill,
  unparkSkill,
} from "../../lib/skill-api";
import {
  lifecycleTargetForDeployment,
  lifecycleTargetForPark,
  lifecycleTargetForSkill,
  pullUpstreamToast,
  skillCanPark,
  skillRemovalBlockedReason,
  skillRemovalChoices,
  skillRemovalEmptiesSkill,
} from "../../lib/skill-lifecycle-target";
import type { SkillRemovalChoice } from "../../lib/skill-lifecycle-target";
import type { InstalledSkill, Toast } from "@skill-studio/lib";
import { useAppStore } from "../../store/appStore";
import { useGuardedSkillUpdate } from "../../hooks/useGuardedSkillUpdate";

/**
 * The one deployment `forkSkill` will accept: the shared-folder copy at
 * `~/.agents/skills/<name>`, or the Claude Code whole-dir symlink to it.
 * `undefined` when neither is deployed, in which case Fork has nothing
 * forkable to point at.
 */
function sharedFolderDeployment(skill: InstalledSkill) {
  return skill.deployments.find(
    (d) => d.path.includes("/.agents/skills/") || d.path.includes("/.claude/skills/"),
  );
}

type AddToast = ReturnType<typeof useAppStore.getState>["addToast"];

/**
 * The header's update button: a fork pulls upstream, any other kind with an update runs the update.
 * Any kind, not only dotagents/skills-sh: a skill with a plugin copy reports `plugin` as its kind
 * while its skills.sh copy still has an update, and the header is the page's only Update.
 */
export function headerUpdateLabel(
  skill: Pick<InstalledSkill, "source_kind" | "update_owner_ids">,
): "Pull latest" | "Update" | null {
  if (skill.update_owner_ids.length === 0) return null;
  return skill.source_kind === "fork" ? "Pull latest" : "Update";
}

/**
 * The header Remove button's success toast: "Removed" plus the skill name,
 * not "Updated N deployments" (unit 3.9b) - `removeSkill` takes exactly one
 * deployment off disk, so a deployment count has nothing to count.
 */
export function removeSuccessToast(skillName: string): Omit<Toast, "id"> {
  return { type: "success", title: "Removed", message: skillName };
}

/**
 * Runs `fn` with `setBusy` bracketing it, and reports a thrown error as an
 * error toast titled `errorTitle`. Every action below is this same shape.
 */
async function runAction(
  addToast: AddToast,
  setBusy: (busy: boolean) => void,
  errorTitle: string,
  fn: () => Promise<void>,
) {
  setBusy(true);
  try {
    await fn();
  } catch (err) {
    addToast({
      type: "error",
      title: errorTitle,
      message: err instanceof Error ? err.message : "Unknown error",
    });
  } finally {
    setBusy(false);
  }
}

export interface SkillPageAction {
  label: string;
  run: () => void;
  busy: boolean;
  /** Present for the primary button when it needs a non-default title. */
  title?: string;
}

export interface SkillPageActions {
  path: string | undefined;
  copied: boolean;
  reveal: () => void;
  openEditor: () => void;
  copyPath: () => void;
  /** The one primary action for the header - "Pull latest" or "Update" - `null` when there is none. */
  primaryAction: SkillPageAction | null;
  /** Park or Unpark - `null` when the skill has no Global Universal folder to move. */
  parkAction: SkillPageAction | null;
  /** Fork (when forkable) or Un-fork (when already forked) - `null` when neither applies. */
  forkAction: SkillPageAction | null;
  /** One entry per scope with an exact mutable removal target, global first. */
  removeActions: (SkillPageAction & { key: string })[];
  /** Why there is no Remove, for a skill whose files the app must not delete. */
  removeBlockedReason: string | null;
  /** The "Update will replace your edits" dialog; render it once beside the header. */
  updateDialog: ReactNode;
}

/**
 * Consolidates every header/overflow-menu action for `skill` into one hook,
 * so `InstalledSkillHeader` only has to render menu items and one primary
 * button. No behaviour changes from the old `InstalledSkillHeader` icon row
 * and `SkillDetailActions`, aside from the update button always targeting
 * global scope (the page header has no scope picker) and Remove gaining the
 * same `ask()` confirm the other destructive actions here already use.
 *
 * `skill` is nullable so `SkillPage` can call this once, unconditionally,
 * even on the "no longer installed" render - every hook above still runs
 * every time, only the returned actions collapse to no-ops.
 */
export function useSkillPageActions(
  skill: InstalledSkill | null,
  onRemoveComplete: () => void,
): SkillPageActions {
  const addToast = useAppStore((state) => state.addToast);
  const [copied, setCopied] = useState(false);
  const [isParking, setIsParking] = useState(false);
  const [isForking, setIsForking] = useState(false);
  const [isPulling, setIsPulling] = useState(false);
  const [isUnforking, setIsUnforking] = useState(false);
  const [isUpdating, setIsUpdating] = useState(false);
  const [isRemoving, setIsRemoving] = useState(false);
  const guard = useGuardedSkillUpdate();

  if (!skill) {
    return {
      path: undefined,
      copied,
      reveal: () => undefined,
      openEditor: () => undefined,
      copyPath: () => undefined,
      primaryAction: null,
      parkAction: null,
      forkAction: null,
      removeActions: [],
      removeBlockedReason: null,
      updateDialog: null,
    };
  }

  const path = skill.deployments[0]?.path ?? skill.skill_path;

  const reveal = () => {
    if (!path) return;
    openSkillPath(path, "reveal").catch((err) => {
      addToast({
        type: "error",
        title: "Couldn't reveal in Finder",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    });
  };

  const openEditor = () => {
    if (!path) return;
    openSkillPath(path, "editor").catch((err) => {
      addToast({
        type: "error",
        title: "Couldn't open in editor",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    });
  };

  const copyPath = () => {
    if (!path) return;
    navigator.clipboard.writeText(path).then(() => {
      setCopied(true);
      addToast({ type: "success", title: "Copied path" });
      setTimeout(() => setCopied(false), 1500);
    });
  };

  const togglePark = () =>
    runAction(
      addToast,
      setIsParking,
      skill.parked ? "Couldn't unpark skill" : "Couldn't park skill",
      async () => {
        if (skill.parked) {
          await unparkSkill(lifecycleTargetForPark(skill));
          addToast({ type: "success", title: `Unparked ${skill.name}` });
        } else {
          await parkSkill(lifecycleTargetForPark(skill));
          addToast({ type: "success", title: `Parked ${skill.name}` });
        }
      },
    );

  const forkDeployment = sharedFolderDeployment(skill);

  const doFork = () => {
    if (!forkDeployment) return;
    return runAction(addToast, setIsForking, "Fork failed", async () => {
      await forkSkill(lifecycleTargetForDeployment(forkDeployment));
      addToast({ type: "success", title: "Forked", message: `${skill.name} is now yours to edit` });
    });
  };

  const doUnfork = async () => {
    const origin = skill.fork?.origin_source ?? "its origin";
    const confirmed = await ask(
      `Discard your changes and reinstall ${skill.name} from ${origin}?`,
      {
        title: "Un-fork skill",
        kind: "warning",
      },
    );
    if (!confirmed) return;
    await runAction(addToast, setIsUnforking, "Un-fork failed", async () => {
      await unforkSkill(lifecycleTargetForSkill(skill, "global"));
      addToast({
        type: "success",
        title: "Un-forked",
        message: `${skill.name} is reinstalled from ${origin}`,
      });
    });
  };

  const doPullUpstream = () =>
    runAction(addToast, setIsPulling, "Pull upstream failed", async () => {
      const result = await pullForkUpstream(lifecycleTargetForSkill(skill, "global"));
      addToast(pullUpstreamToast(result));
    });

  const doUpdate = () =>
    runAction(addToast, setIsUpdating, "Update failed", () => guard.requestUpdate(skill));

  const doRemove = async (choice: SkillRemovalChoice) => {
    const confirmed = await ask(choice.confirmMessage, {
      title: choice.confirmTitle,
      kind: "warning",
      okLabel: choice.label,
    });
    if (!confirmed) return;
    await runAction(addToast, setIsRemoving, "Remove failed", async () => {
      await removeSkill(choice.preview.target);
      if (skillRemovalEmptiesSkill(skill, choice.selection)) onRemoveComplete();
      addToast(removeSuccessToast(skill.name));
    });
  };

  let primaryAction: SkillPageAction | null = null;
  const updateLabel = headerUpdateLabel(skill);
  if (updateLabel === "Pull latest") {
    primaryAction = { label: updateLabel, run: doPullUpstream, busy: isPulling };
  } else if (updateLabel === "Update") {
    primaryAction = { label: updateLabel, run: doUpdate, busy: isUpdating || guard.isResolving };
  }

  let forkAction: SkillPageAction | null = null;
  if (skill.source_kind === "fork") {
    forkAction = { label: "Un-fork", run: doUnfork, busy: isUnforking };
  } else if (forkDeployment) {
    forkAction = { label: "Fork", run: doFork, busy: isForking };
  }

  const removeActions = skillRemovalChoices(skill).map((choice) => ({
    key: choice.key,
    label: choice.label,
    run: () => void doRemove(choice),
    busy: isRemoving,
  }));

  return {
    path,
    copied,
    reveal,
    openEditor,
    copyPath,
    primaryAction,
    parkAction: skillCanPark(skill)
      ? { label: skill.parked ? "Unpark" : "Park", run: togglePark, busy: isParking }
      : null,
    forkAction,
    removeActions,
    removeBlockedReason: skillRemovalBlockedReason(skill),
    updateDialog: guard.dialog,
  };
}
