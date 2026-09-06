// ============================================================================
// RemoveDeploymentsDialog - the confirm step for the Locations card's trash
// action. `remove_skill` works per scope, not per harness, so this names the
// scope it is about to clear rather than pretending a single harness can be
// deleted on its own.
// ============================================================================

import { useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@skill-studio/ui";
import { removeSkill } from "../../lib/skill-api";
import {
  skillDeploymentRemovalPreview,
  skillRemovalAvailability,
  skillRemovalDescription,
} from "../../lib/skill-lifecycle-target";
import { useAppStore } from "../../store/appStore";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";

export function RemoveDeploymentsDialog({
  skill,
  scopeLabel,
  projectPath,
  deployment,
  onClose,
}: {
  skill: InstalledSkill;
  /** "Global" or the project folder's name - what the section this came from is called. */
  scopeLabel: string;
  /** `null` for the global scope, otherwise the project directory to remove from. */
  projectPath: string | null;
  /** Exact independent Copy selected from a Locations row. */
  deployment?: Deployment;
  onClose: () => void;
}) {
  const [isRemoving, setIsRemoving] = useState(false);
  const addToast = useAppStore((state) => state.addToast);
  const skillName = skill.name;
  const removalAvailability = deployment
    ? { available: true as const, preview: skillDeploymentRemovalPreview(skill, deployment) }
    : skillRemovalAvailability(skill, {
        skillName,
        scope: projectPath ? "project" : "global",
        projectPath,
      });

  const handleRemove = () => {
    if (!removalAvailability.available) return;
    setIsRemoving(true);
    removeSkill(removalAvailability.preview.target)
      .then((result) => {
        if (!result.success) throw new Error(result.error ?? "The removal did not complete.");
        onClose();
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't remove",
          message: err instanceof Error ? err.message : "Unknown error",
        });
      })
      .finally(() => setIsRemoving(false));
  };

  return (
    <AlertDialog open onOpenChange={(open) => !open && onClose()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            Remove {skillName} from {scopeLabel}?
          </AlertDialogTitle>
          <AlertDialogDescription>
            {removalAvailability.available
              ? skillRemovalDescription(removalAvailability.preview)
              : removalAvailability.reason}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={onClose} disabled={isRemoving}>
            Cancel
          </AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            onClick={handleRemove}
            disabled={isRemoving || !removalAvailability.available}
          >
            {isRemoving ? "Removing…" : `Remove from ${scopeLabel}`}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
