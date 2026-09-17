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
import type {
  SkillLifecycleScopeSelection,
  SkillRemovalPreview,
  SkillUpdateAvailability,
} from "../../lib/skill-lifecycle-target";
import type { InstallScope, SkillWithStatus } from "@skill-studio/lib";

const ACTION_BUTTON_CLASS =
  "h-(--control-height) w-full justify-center gap-2 rounded-md px-3.5 text-body font-medium";

interface InstalledSkillLifecycleActionsProps {
  skill: SkillWithStatus;
  onInstallComplete: (result: SkillInstallCompletion) => void;
  onRemoveComplete: () => void;
}

/** Every value about `installedSkill` derived purely from it and the
 * selected scope - pulled out of the component so the scope-selection and
 * removal/update-availability chain isn't also part of its own body. */
function deriveLifecycleTargets(
  installedSkill: SkillWithStatus["installed_info"],
  lifecycleScope: SkillLifecycleScopeSelection | null,
) {
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

  return {
    mutableLifecycleScopes,
    selectedLifecycleScope,
    mutableProjectPaths,
    hasGlobalLifecycleScope,
    hasProjectLifecycleScope,
    removalPreview,
    removalDisabledReason,
    updateAvailability,
    updateDisabledReason,
  };
}

/** The Remove and Update `npx skills` calls, plus the busy flags they own -
 * pulled out of the component since both are the same shape (call, report
 * outcome, clear the busy flag) and neither shares state with the rest of
 * the component beyond the flag itself. */
function useSkillLifecycleMutations(
  skill: SkillWithStatus,
  removalPreview: SkillRemovalPreview | null,
  updateAvailability: SkillUpdateAvailability | null,
  onInstallComplete: (result: SkillInstallCompletion) => void,
  onRemoveComplete: () => void,
) {
  const [isRemoving, setIsRemoving] = useState(false);
  const [isUpdating, setIsUpdating] = useState(false);
  const [showRemoveConfirm, setShowRemoveConfirm] = useState(false);

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
            error: result.error ?? "Update command failed without an error message.",
            skillName: skill.name,
          });
        }
      })
      .catch((error) => {
        onInstallComplete({
          success: false,
          error: error instanceof Error ? error.message : "Update failed without an error message.",
          skillName: skill.name,
        });
      })
      .finally(() => {
        setIsUpdating(false);
      });
  };

  return {
    isRemoving,
    isUpdating,
    showRemoveConfirm,
    setShowRemoveConfirm,
    handleRemove,
    handleUpdate,
  };
}

/** The "Manage scope" section, shown only when there's more than one mutable
 * scope to choose between. */
function LifecycleScopePicker({
  selectedLifecycleScope,
  hasGlobalLifecycleScope,
  hasProjectLifecycleScope,
  mutableProjectPaths,
  onScopeChange,
  onProjectChange,
}: {
  selectedLifecycleScope: SkillLifecycleScopeSelection;
  hasGlobalLifecycleScope: boolean;
  hasProjectLifecycleScope: boolean;
  mutableProjectPaths: string[];
  onScopeChange: (scope: InstallScope) => void;
  onProjectChange: (projectPath: string) => void;
}) {
  return (
    <div className="mb-2">
      <h4 className="m-0 mb-2 text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        Manage scope
      </h4>
      {hasGlobalLifecycleScope && hasProjectLifecycleScope && (
        <ScopeToggleGroup
          scope={selectedLifecycleScope.scope}
          onScopeChange={onScopeChange}
          ariaLabel="Manage scope"
        />
      )}
      {selectedLifecycleScope.scope === "project" && (
        <div className={hasGlobalLifecycleScope ? "mt-2" : undefined}>
          <ProjectDirectorySelect
            projects={mutableProjectPaths}
            value={selectedLifecycleScope.projectPath ?? undefined}
            onChange={onProjectChange}
            ariaLabel="Installed project directory"
          />
        </div>
      )}
    </div>
  );
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
  const {
    mutableLifecycleScopes,
    selectedLifecycleScope,
    mutableProjectPaths,
    hasGlobalLifecycleScope,
    hasProjectLifecycleScope,
    removalPreview,
    removalDisabledReason,
    updateAvailability,
    updateDisabledReason,
  } = deriveLifecycleTargets(installedSkill, lifecycleScope);
  const {
    isRemoving,
    isUpdating,
    showRemoveConfirm,
    setShowRemoveConfirm,
    handleRemove,
    handleUpdate,
  } = useSkillLifecycleMutations(
    skill,
    removalPreview,
    updateAvailability,
    onInstallComplete,
    onRemoveComplete,
  );

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

  const hasUpdateOwner = (installedSkill?.update_owner_ids.length ?? 0) > 0;

  return (
    <div className="mt-auto flex flex-col gap-2 p-5">
      {selectedLifecycleScope && mutableLifecycleScopes.length > 1 && (
        <LifecycleScopePicker
          selectedLifecycleScope={selectedLifecycleScope}
          hasGlobalLifecycleScope={hasGlobalLifecycleScope}
          hasProjectLifecycleScope={hasProjectLifecycleScope}
          mutableProjectPaths={mutableProjectPaths}
          onScopeChange={handleLifecycleScopeChange}
          onProjectChange={handleLifecycleProjectChange}
        />
      )}
      {hasUpdateOwner && (
        <Button
          className={`${ACTION_BUTTON_CLASS} bg-accent-solid text-text-on-accent hover:bg-accent-solid-hover`}
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
      {updateDisabledReason && hasUpdateOwner && (
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
