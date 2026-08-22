// ============================================================================
// SkillDetailPanel - Detail view for a selected skill
// ============================================================================

import { useEffect, useState } from "react";
import { SkillDetailHeader } from "./SkillDetailHeader";
import { SkillContent } from "./SkillContent";
import { InstallControls } from "./InstallControls";
import { getSkillDetails } from "../../lib/skill-api";
import type { SkillWithStatus } from "../../lib/skill-types";

interface SkillDetailPanelProps {
  skill: SkillWithStatus;
  onClose: () => void;
  onInstallStart: (skillName: string) => void;
  onInstallComplete: (result: { success: boolean; error?: string; skillName?: string }) => void;
  onRemoveComplete: () => void;
}

export function SkillDetailPanel({
  skill,
  onClose,
  onInstallStart,
  onInstallComplete,
  onRemoveComplete,
}: SkillDetailPanelProps) {
  const [resolvedTopSource, setResolvedTopSource] = useState<string | null>(
    skill.top_source ?? null,
  );

  // Resolve top_source - either from skill prop or by fetching details
  useEffect(() => {
    if (skill.top_source) {
      setResolvedTopSource(skill.top_source);
      return;
    }

    let cancelled = false;
    getSkillDetails(skill.name)
      .then((details) => {
        if (!cancelled) {
          setResolvedTopSource(details.top_source ?? null);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setResolvedTopSource(null);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [skill.name, skill.top_source]);

  return (
    <div className="skill-detail-panel">
      <SkillDetailHeader skill={skill} resolvedTopSource={resolvedTopSource} onClose={onClose} />
      <SkillContent skill={skill} resolvedTopSource={resolvedTopSource} />
      <div className="skill-detail-divider" />
      <InstallControls
        skill={skill}
        resolvedTopSource={resolvedTopSource}
        onInstallStart={onInstallStart}
        onInstallComplete={onInstallComplete}
        onRemoveComplete={onRemoveComplete}
      />
    </div>
  );
}
