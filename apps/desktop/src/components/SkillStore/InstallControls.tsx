// ============================================================================
// InstallControls - routes store skills to install or installed lifecycle UI
// ============================================================================

import { InstalledSkillLifecycleActions } from "./InstalledSkillLifecycleActions";
import { SkillStoreInstallFlow } from "./SkillStoreInstallFlow";
import type { AddSkillRequest, SkillWithStatus } from "@skill-studio/lib";

/** Result reported after a skills.sh install or update attempt. */
export interface SkillInstallCompletion {
  success: boolean;
  error?: string;
  skillName?: string;
  warning?: string;
}

interface InstallControlsProps {
  skill: SkillWithStatus;
  resolvedTopSource: string | null;
  isInstalling: boolean;
  onInstallStart: (skillName: string, request: AddSkillRequest) => void;
  onInstallComplete: (result: SkillInstallCompletion) => void;
  onRemoveComplete: () => void;
}

/** Shows installation controls or deployment-targeted lifecycle actions for one store skill. */
export function InstallControls({
  skill,
  resolvedTopSource,
  isInstalling,
  onInstallStart,
  onInstallComplete,
  onRemoveComplete,
}: InstallControlsProps) {
  if (!skill.is_installed) {
    return (
      <SkillStoreInstallFlow
        skill={skill}
        resolvedTopSource={resolvedTopSource}
        isInstalling={isInstalling}
        onInstallStart={onInstallStart}
        onInstallComplete={onInstallComplete}
      />
    );
  }

  return (
    <InstalledSkillLifecycleActions
      skill={skill}
      onInstallComplete={onInstallComplete}
      onRemoveComplete={onRemoveComplete}
    />
  );
}
