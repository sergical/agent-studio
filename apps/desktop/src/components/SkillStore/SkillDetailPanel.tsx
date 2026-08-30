// ============================================================================
// SkillDetailPanel - Detail view for a selected skill
// ============================================================================

import { useEffect, useState } from "react";
import { SkillDetailHeader } from "./SkillDetailHeader";
import { SkillContent } from "./SkillContent";
import { InstallControls } from "./InstallControls";
import { getSkillDetails } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import type { SkillWithStatus } from "@skill-studio/lib";

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
  const openAddSkillSheet = useAppStore((state) => state.openAddSkillSheet);

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
    <div className="fixed top-0 right-0 bottom-0 z-(--z-drawer) flex w-[560px] animate-[slideInDrawer_0.2s_ease] flex-col overflow-y-auto border-l border-border bg-bg-secondary">
      <SkillDetailHeader skill={skill} resolvedTopSource={resolvedTopSource} onClose={onClose} />
      <SkillContent skill={skill} resolvedTopSource={resolvedTopSource} />
      <button
        type="button"
        className="mx-5 self-start border-0 bg-transparent p-0 text-small text-accent hover:underline"
        onClick={() =>
          openAddSkillSheet(resolvedTopSource ? `${resolvedTopSource}/${skill.name}` : skill.name)
        }
      >
        Add with more options…
      </button>
      <div className="my-2 h-px bg-border" />
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
