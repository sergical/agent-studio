// ============================================================================
// SplitSkillDialog - confirms "Split into harness folders…" from the
// Universal row of the Locations card: pick the harnesses that keep the
// skill, see the folders each copy goes to, then split. The Universal folder
// and its links go away; Activity holds the undo.
// ============================================================================

import { useState } from "react";
import {
  Button,
  Checkbox,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@skill-studio/ui";
import { deploymentLabelFromAgentId, homeRelativePath } from "@skill-studio/lib";
import type { AgentId, LifecycleTarget } from "@skill-studio/lib";
import { useSkillSplit } from "../../hooks/useSkillSplit";
import { useAppStore } from "../../store/appStore";
import { SPLIT_HARNESSES, SPLIT_UPDATE_NOTE, splitFolderRows } from "./skill-split-model";

interface SplitSkillDialogProps {
  skillName: string;
  /** The Universal deployment to split. */
  target: LifecycleTarget;
  /** `null` for the Global Universal folder. */
  projectPath: string | null;
  /** Harnesses that read the Universal folder now - checked when the dialog opens. */
  readers: AgentId[];
  onClose: () => void;
}

export function SplitSkillDialog({
  skillName,
  target,
  projectPath,
  readers,
  onClose,
}: SplitSkillDialogProps) {
  const addToast = useAppStore((state) => state.addToast);
  const [checked, setChecked] = useState<ReadonlySet<AgentId>>(() => new Set(readers));
  const [isSplitting, setIsSplitting] = useState(false);
  const { targets, split } = useSkillSplit(skillName, projectPath, SPLIT_HARNESSES);

  const folders = targets ? splitFolderRows(targets, checked) : [];

  const toggle = (harness: AgentId, on: boolean) => {
    setChecked((previous) => {
      const next = new Set(previous);
      if (on) next.add(harness);
      else next.delete(harness);
      return next;
    });
  };

  const handleSplit = () => {
    setIsSplitting(true);
    split(
      target,
      SPLIT_HARNESSES.filter((harness) => checked.has(harness)),
    )
      .then(onClose)
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't split",
          message: err instanceof Error ? err.message : String(err),
        });
      })
      .finally(() => setIsSplitting(false));
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Split {skillName} into agent folders?</DialogTitle>
          <DialogDescription>
            Each agent you pick gets its own copy of {skillName}. The Universal folder and its links
            are removed. Agents you don't pick lose this skill. Activity records this change and
            provides the undo action.
          </DialogDescription>
        </DialogHeader>
        <ul className="flex flex-col gap-2">
          {SPLIT_HARNESSES.map((harness) => (
            <li key={harness}>
              <label className="flex items-center gap-2 text-body text-text-primary">
                <Checkbox
                  checked={checked.has(harness)}
                  onCheckedChange={(on) => toggle(harness, on === true)}
                  disabled={isSplitting}
                />
                {deploymentLabelFromAgentId(harness)}
              </label>
            </li>
          ))}
        </ul>
        <div className="flex flex-col gap-1">
          <p className="text-small text-text-secondary">Folders to write</p>
          {targets === null ? (
            <p className="text-small text-text-tertiary">Finding folders…</p>
          ) : folders.length === 0 ? (
            <p className="text-small text-text-tertiary">No agent picked.</p>
          ) : (
            <ul className="flex flex-col gap-1">
              {folders.map((folder) => (
                <li
                  key={folder.harness}
                  className="truncate font-mono text-small text-text-primary"
                >
                  {homeRelativePath(folder.path)}
                </li>
              ))}
            </ul>
          )}
        </div>
        <p className="text-small text-text-tertiary">{SPLIT_UPDATE_NOTE}</p>
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={isSplitting}>
            Cancel
          </Button>
          <Button onClick={handleSplit} disabled={isSplitting || checked.size === 0}>
            {isSplitting ? "Splitting…" : "Split"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
