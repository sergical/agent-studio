// ============================================================================
// useSkillPageActions - Every action the skill page's header offers (primary
// button, overflow menu, and the park/fork/remove flows that used to live in
// InstalledSkillHeader's icon row and SkillDetailActions): reveal, open in
// editor, copy path, park/unpark, fork/un-fork/pull upstream, update, and
// remove. One hook so the header stays a thin render of this state.
// ============================================================================

import { useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import {
  forkSkill,
  keepSkillTrial,
  openSkillPath,
  parkSkill,
  pullForkUpstream,
  removeSkill,
  unforkSkill,
  unparkSkill,
  updateSkill,
} from "../../lib/skill-api";
import type { InstalledSkill } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";

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
  parkAction: SkillPageAction;
  /** Fork (when forkable) or Un-fork (when already forked) - `null` when neither applies. */
  forkAction: SkillPageAction | null;
  /** Only skills.sh-managed skills carry lock-file metadata `removeSkill` needs. */
  removeAction: SkillPageAction | null;
  keepTrial: SkillPageAction;
}

/**
 * Consolidates every header/overflow-menu action for `skill` into one hook,
 * so `InstalledSkillHeader` only has to render menu items and one primary
 * button. No behaviour changes from the old `InstalledSkillHeader` icon row
 * and `SkillDetailActions`, aside from the update button always targeting
 * global scope (the page header has no scope picker) and Remove gaining the
 * same `ask()` confirm the other destructive actions here already use.
 */
export function useSkillPageActions(
  skill: InstalledSkill,
  onRemoveComplete: () => void,
): SkillPageActions {
  const addToast = useAppStore((state) => state.addToast);
  const [copied, setCopied] = useState(false);
  const [isParking, setIsParking] = useState(false);
  const [isKeeping, setIsKeeping] = useState(false);
  const [isForking, setIsForking] = useState(false);
  const [isPulling, setIsPulling] = useState(false);
  const [isUnforking, setIsUnforking] = useState(false);
  const [isUpdating, setIsUpdating] = useState(false);
  const [isRemoving, setIsRemoving] = useState(false);

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
          await unparkSkill(skill.name);
          addToast({ type: "success", title: `Unparked ${skill.name}` });
        } else {
          await parkSkill(skill.name);
          addToast({ type: "success", title: `Parked ${skill.name}` });
        }
      },
    );

  const keepTrial = () =>
    runAction(addToast, setIsKeeping, "Couldn't keep skill", async () => {
      await keepSkillTrial(skill.name, skill.trial?.scope ?? "global", skill.trial?.project_path);
      addToast({ type: "success", title: `Kept ${skill.name}` });
    });

  const forkPath = sharedFolderDeployment(skill)?.path;

  const doFork = () => {
    if (!forkPath) return;
    return runAction(addToast, setIsForking, "Fork failed", async () => {
      await forkSkill(skill.name, forkPath);
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
      await unforkSkill(skill.name);
      addToast({
        type: "success",
        title: "Un-forked",
        message: `${skill.name} is reinstalled from ${origin}`,
      });
    });
  };

  const doPullUpstream = () =>
    runAction(addToast, setIsPulling, "Pull upstream failed", async () => {
      const result = await pullForkUpstream(skill.name);
      if (result.message) {
        addToast({ type: "info", title: result.message });
      } else if (result.conflicts.length > 0) {
        addToast({
          type: "warning",
          title: `${result.conflicts.length} conflicts — open the editor to resolve`,
          message: result.conflicts.join(", "),
        });
      } else {
        const mergedCount = result.merged.length + result.added.length + result.removed.length;
        addToast({ type: "success", title: `Merged ${mergedCount} files` });
      }
    });

  const doUpdate = () =>
    runAction(addToast, setIsUpdating, "Update failed", async () => {
      const result = await updateSkill(skill.name, true);
      if (result.success) {
        addToast({
          type: "success",
          title: "Skill updated",
          message: result.tool ? `Ran ${result.tool} for ${skill.name}` : undefined,
        });
      } else {
        addToast({ type: "error", title: "Update failed", message: result.error });
      }
    });

  const doRemove = async () => {
    const confirmed = await ask(`Remove ${skill.name}?`, {
      title: "Remove skill",
      kind: "warning",
    });
    if (!confirmed) return;
    await runAction(addToast, setIsRemoving, "Remove failed", async () => {
      const result = await removeSkill(skill.name, true);
      if (result.success) {
        onRemoveComplete();
      } else {
        addToast({ type: "error", title: "Remove failed", message: result.error });
      }
    });
  };

  let primaryAction: SkillPageAction | null = null;
  if (skill.source_kind === "fork" && skill.has_update) {
    primaryAction = { label: "Pull latest", run: doPullUpstream, busy: isPulling };
  } else if (
    (skill.source_kind === "dotagents" || skill.source_kind === "skills-sh") &&
    skill.has_update
  ) {
    primaryAction = { label: "Update", run: doUpdate, busy: isUpdating };
  }

  let forkAction: SkillPageAction | null = null;
  if (skill.source_kind === "fork") {
    forkAction = { label: "Un-fork", run: doUnfork, busy: isUnforking };
  } else if (forkPath) {
    forkAction = { label: "Fork", run: doFork, busy: isForking };
  }

  const removeAction: SkillPageAction | null =
    skill.source_kind === "skills-sh" ? { label: "Remove", run: doRemove, busy: isRemoving } : null;

  return {
    path,
    copied,
    reveal,
    openEditor,
    copyPath,
    primaryAction,
    parkAction: {
      label: skill.parked ? "Unpark" : "Park",
      run: togglePark,
      busy: isParking,
    },
    forkAction,
    removeAction,
    keepTrial: { label: "Keep", run: keepTrial, busy: isKeeping },
  };
}
