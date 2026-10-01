// ============================================================================
// UpdateOverwritesEditsDialog - the confirm step before Update replaces a
// skills.sh skill the user edited. Offers a fork (keep the edits, merge the
// update in) ahead of the overwrite. Used for one skill and for "Update all".
// ============================================================================

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

/** The pending "Update will replace your edits" choice: the dialog is open while `skillNames` is not empty. */
export interface UpdatePrompt {
  skillNames: string[];
  /** False when a fork is impossible here (no Global Universal copy), so the dialog offers only Overwrite and Cancel. */
  canFork: boolean;
  fork: () => void;
  overwrite: () => void;
  cancel: () => void;
}

function forkCopy(skillNames: string[], isBulk: boolean): string {
  if (!isBulk) {
    return `You changed ${skillNames[0]} after you installed it. Fork it to keep your edits and merge the update in, or overwrite your edits with the new version.`;
  }
  const pronoun = skillNames.length === 1 ? "it" : "them";
  return `You changed ${skillNames.join(", ")} after you installed ${pronoun}. Fork ${pronoun} to keep your edits and merge the update in, or overwrite your edits with the new version.`;
}

function noForkCopy(skillNames: string[], isBulk: boolean): string {
  const names = isBulk ? skillNames.join(", ") : skillNames[0];
  return `You changed ${names} after you installed ${isBulk && skillNames.length > 1 ? "them" : "it"}. ${isBulk && skillNames.length > 1 ? "They" : "It"} cannot be forked here, so the update would overwrite your edits.`;
}

export function UpdateOverwritesEditsDialog({
  skillNames,
  isBulk,
  canFork,
  onFork,
  onOverwrite,
  onCancel,
}: {
  /** The edited skills the update would replace; the dialog is closed while this is empty. */
  skillNames: string[];
  /** True for "Update all": the buttons say "all" and the description lists every name. */
  isBulk: boolean;
  canFork: boolean;
  onFork: () => void;
  onOverwrite: () => void;
  onCancel: () => void;
}) {
  return (
    <AlertDialog open={skillNames.length > 0} onOpenChange={(open) => !open && onCancel()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Update will replace your edits</AlertDialogTitle>
          <AlertDialogDescription>
            {canFork ? forkCopy(skillNames, isBulk) : noForkCopy(skillNames, isBulk)}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={onOverwrite}>
            {isBulk ? "Overwrite all" : "Overwrite edits"}
          </AlertDialogAction>
          {canFork && (
            <AlertDialogAction onClick={onFork}>
              {isBulk ? "Fork edited and update all" : "Fork and update"}
            </AlertDialogAction>
          )}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
