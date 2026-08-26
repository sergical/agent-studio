// ============================================================================
// SkillDetailActions - Remove/update controls for skills.sh skills, plus the
// update-only control for dotagents skills (reveal/open/copy/enable-disable
// live in InstalledSkillHeader; that header shows the "Update available"
// text but never its own button, so there's exactly one update control per
// skill).
// ============================================================================

import { useState } from "react";
import { RefreshCw } from "lucide-react";
import { InstallControls } from "../SkillStore/InstallControls";
import { updateSkill } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
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
 * Update-only control for a dotagents-managed skill: no lock-file metadata,
 * so `InstallControls`' remove flow doesn't apply, but the update flow
 * (`npx @sentry/dotagents add|install`) does.
 */
function DotagentsUpdateButton({ skill }: { skill: InstalledSkill }) {
  const [isUpdating, setIsUpdating] = useState(false);
  const addToast = useAppStore((state) => state.addToast);

  const handleUpdate = async () => {
    setIsUpdating(true);
    try {
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
    } catch (err) {
      addToast({
        type: "error",
        title: "Update failed",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsUpdating(false);
    }
  };

  return (
    <div className="skill-detail-actions">
      <button
        className="skill-action-button primary"
        onClick={handleUpdate}
        disabled={isUpdating}
        title="Dotagents-managed: re-pins to the latest commit, or re-syncs if this skill has no pinned entry"
      >
        {isUpdating ? (
          <>
            <span className="spinner" />
            Updating...
          </>
        ) : (
          <>
            <RefreshCw size={16} />
            Update Skill
          </>
        )}
      </button>
    </div>
  );
}

/**
 * Remove/update controls, via the shared `InstallControls`, for skills.sh
 * skills (which carry the lock-file metadata `InstallControls` needs), or
 * an update-only control for a dotagents skill with an update available.
 * Manual and plugin skills have no owning CLI to update through, so they
 * render nothing here.
 */
export function SkillDetailActions({ skill, onRemoveComplete }: SkillDetailActionsProps) {
  if (skill.source_kind === "skills-sh") {
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

  if (skill.source_kind === "dotagents" && skill.has_update) {
    return (
      <div className="skill-detail-section skill-detail-actions-row">
        <DotagentsUpdateButton skill={skill} />
      </div>
    );
  }

  return null;
}
