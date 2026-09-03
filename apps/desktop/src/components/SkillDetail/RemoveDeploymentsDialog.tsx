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
import { useAppStore } from "../../store/appStore";

export function RemoveDeploymentsDialog({
  skillName,
  scopeLabel,
  projectPath,
  onClose,
}: {
  skillName: string;
  /** "Global" or the project folder's name - what the section this came from is called. */
  scopeLabel: string;
  /** `null` for the global scope, otherwise the project directory to remove from. */
  projectPath: string | null;
  onClose: () => void;
}) {
  const [isRemoving, setIsRemoving] = useState(false);
  const addToast = useAppStore((state) => state.addToast);

  const handleRemove = () => {
    setIsRemoving(true);
    removeSkill(skillName, projectPath)
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
            Every {scopeLabel} copy goes with it, including the links each harness reads it through.
            Copies in other scopes stay where they are. Reinstalling brings it back.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={onClose} disabled={isRemoving}>
            Cancel
          </AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={handleRemove} disabled={isRemoving}>
            {isRemoving ? "Removing…" : `Remove from ${scopeLabel}`}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
