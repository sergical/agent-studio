// ============================================================================
// SkillDetailActions - Remove/update controls for skills.sh skills
// (reveal/open/copy/enable-disable now live in InstalledSkillHeader)
// ============================================================================

import { InstallControls } from "../SkillStore/InstallControls";
import type { InstalledSkill, SkillWithStatus } from "../../lib/skill-types";

interface SkillDetailActionsProps {
  skill: InstalledSkill;
  onRemoveComplete: () => void;
}

/** Wraps an `InstalledSkill` in the shape `InstallControls` expects. */
function toSkillWithStatus(skill: InstalledSkill): SkillWithStatus {
  return {
    id: skill.name,
    name: skill.name,
    installs: 0,
    is_installed: true,
    installed_info: skill,
  };
}

/**
 * Remove/update controls, via the shared `InstallControls`. Only skills.sh
 * skills carry lock-file metadata `InstallControls` needs; other source
 * kinds render nothing here.
 */
export function SkillDetailActions({ skill, onRemoveComplete }: SkillDetailActionsProps) {
  if (skill.source_kind !== "skills-sh") return null;

  return (
    <div className="skill-detail-section skill-detail-actions-row">
      <InstallControls
        skill={toSkillWithStatus(skill)}
        resolvedTopSource={null}
        onInstallStart={() => {}}
        onInstallComplete={() => {}}
        onRemoveComplete={onRemoveComplete}
      />
    </div>
  );
}
