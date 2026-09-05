// ============================================================================
// InstalledSkillLifecycleActions - deployment-owner update and removal actions
// ============================================================================

import { useState } from "react";
import { RefreshCw, Trash2 } from "lucide-react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Button,
} from "@skill-studio/ui";
import { ProjectDirectorySelect } from "./ProjectDirectorySelect";
import { ScopeToggleGroup } from "./ScopeToggleGroup";
import { removeSkill, updateSkill } from "../../lib/skill-api";
import {
  skillLifecycleScopeSelection,
  skillMutableLifecycleScopes,
  skillRemovalAvailability,
  skillRemovalDescription,
  skillUpdateAvailability,
} from "../../lib/skill-lifecycle-target";
import type { SkillInstallCompletion } from "./InstallControls";
import type { SkillLifecycleScopeSelection } from "../../lib/skill-lifecycle-target";
import type { InstallScope, SkillWithStatus } from "@skill-studio/lib";

const ACTION_BUTTON_CLASS =
  "h-(--control-height) w-full justify-center gap-2 rounded-md px-3.5 text-body font-medium";

interface InstalledSkillLifecycleActionsProps {
  skill: SkillWithStatus;
  onInstallComplete: (result: SkillInstallCompletion) => void;
  onRemoveComplete: () => void;
}

/** Updates or removes the explicitly selected deployment owner for an installed skill. */
export function InstalledSkillLifecycleActions({
  skill,
  onInstallComplete,
  onRemoveComplete,
}: InstalledSkillLifecycleActionsProps) {
  const installedSkill = skill.installed_info;
  const [lifecycleScope, setLifecycleScope] = useState<SkillLifecycleScopeSelection | null>(() =>
    installedSkill ? skillLifecycleScopeSelection(installedSkill) : null,
  );
  const [isRemoving, setIsRemoving] = useState(false);
  const [isUpdating, setIsUpdating] = useState(false);
  const [showRemoveConfirm, setShowRemoveConfirm] = useState(false);
  const mutableLifecycleScopes = installedSkill ? skillMutableLifecycleScopes(installedSkill) : [];
  const selectedLifecycleScope = installedSkill
    ? skillLifecycleScopeSelection(installedSkill, lifecycleScope)
    : null;
  const mutableProjectPaths = mutableLifecycleScopes.flatMap((selection) =>
    selection.scope === "project" && selection.projectPath ? [selection.projectPath] : [],
  );
  const hasGlobalLifecycleScope = mutableLifecycleScopes.some(
    (selection) => selection.scope === "global",
  );
  const hasProjectLifecycleScope = mutableProjectPaths.length > 0;
  const removalAvailability =
    installedSkill && selectedLifecycleScope
      ? skillRemovalAvailability(installedSkill, selectedLifecycleScope)
      : null;
  const removalPreview = removalAvailability?.available ? removalAvailability.preview : null;
  const removalDisabledReason =
    removalAvailability && !removalAvailability.available ? removalAvailability.reason : null;
  const updateAvailability =
    installedSkill && selectedLifecycleScope
      ? skillUpdateAvailability(installedSkill, selectedLifecycleScope)
      : null;
  const updateDisabledReason =
    updateAvailability && !updateAvailability.available ? updateAvailability.reason : null;

  const handleLifecycleScopeChange = (scope: InstallScope) => {
    const next = mutableLifecycleScopes.find((selection) => selection.scope === scope);
    if (next) setLifecycleScope(next);
  };

  const handleLifecycleProjectChange = (projectPath: string) => {
    const next = mutableLifecycleScopes.find(
      (selection) => selection.scope === "project" && selection.projectPath === projectPath,
    );
    if (next) setLifecycleScope(next);
  };

  const handleRemove = () => {
    setIsRemoving(true);
    if (!removalPreview) {
      setIsRemoving(false);
      return Promise.resolve();
    }
    return removeSkill(removalPreview.target)
      .then((result) => {
        if (result.success) onRemoveComplete();
      })
      .finally(() => {
        setIsRemoving(false);
        setShowRemoveConfirm(false);
      });
  };

  const handleUpdate = () => {
    setIsUpdating(true);
    if (!updateAvailability?.available) {
      setIsUpdating(false);
      return Promise.resolve();
    }
    return updateSkill(updateAvailability.target)
      .then((result) => {
        if (result.success) {
          onInstallComplete({ success: true, skillName: skill.name });
        } else {
          onInstallComplete({
            success: false,
            error: result.error ?? "Update did not complete",
            skillName: skill.name,
          });
        }
      })
      .catch((error) => {
        onInstallComplete({
          success: false,
          error: error instanceof Error ? error.message : "Unknown error",
          skillName: skill.name,
        });
      })
      .finally(() => {
        setIsUpdating(false);
      });
  };

  return (
    <div className="mt-auto flex flex-col gap-2 p-5">
      {selectedLifecycleScope && mutableLifecycleScopes.length > 1 && (
        <div className="mb-2">
          <h4 className="m-0 mb-2 text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
            Manage scope
          </h4>
          {hasGlobalLifecycleScope && hasProjectLifecycleScope && (
            <ScopeToggleGroup
              scope={selectedLifecycleScope.scope}
              onScopeChange={handleLifecycleScopeChange}
              ariaLabel="Manage scope"
            />
          )}
          {selectedLifecycleScope.scope === "project" && (
            <div className={hasGlobalLifecycleScope ? "mt-2" : undefined}>
              <ProjectDirectorySelect
                projects={mutableProjectPaths}
                value={selectedLifecycleScope.projectPath ?? undefined}
                onChange={handleLifecycleProjectChange}
                ariaLabel="Installed project directory"
              />
            </div>
          )}
        </div>
      )}
      {(installedSkill?.update_owner_ids.length ?? 0) > 0 && (
        <Button
          className={`${ACTION_BUTTON_CLASS} bg-accent text-text-on-accent hover:bg-accent-hover`}
          onClick={handleUpdate}
          disabled={isUpdating || !updateAvailability?.available}
        >
          {isUpdating ? (
            <>
              <span className="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
              Updating…
            </>
          ) : (
            <>
              <RefreshCw size={16} />
              Update Skill
            </>
          )}
        </Button>
      )}
      {updateDisabledReason && (installedSkill?.update_owner_ids.length ?? 0) > 0 && (
        <p className="m-0 text-caption text-text-tertiary">{updateDisabledReason}</p>
      )}
      <Button
        className={`${ACTION_BUTTON_CLASS} bg-error-soft text-error hover:bg-error hover:text-white`}
        onClick={() => setShowRemoveConfirm(true)}
        disabled={isRemoving || !removalPreview}
      >
        <Trash2 size={16} />
        Remove Skill
      </Button>
      {removalDisabledReason && (
        <p className="m-0 text-caption text-text-tertiary">{removalDisabledReason}</p>
      )}

      <AlertDialog open={showRemoveConfirm} onOpenChange={setShowRemoveConfirm}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove {skill.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              {removalPreview ? skillRemovalDescription(removalPreview) : "Removal is unavailable."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={isRemoving}>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={handleRemove} disabled={isRemoving}>
              {isRemoving ? "Removing…" : "Confirm Remove"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
