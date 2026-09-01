// ============================================================================
// SkillDetailPanel - Detail view for a selected skill
// ============================================================================

import { useEffect, useState } from "react";
import { SkillDetailHeader } from "./SkillDetailHeader";
import { SkillContent } from "./SkillContent";
import { InstallControls } from "./InstallControls";
import { getSkillDetails } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import type { SkillDetails, SkillWithStatus } from "@skill-studio/lib";

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
  const [details, setDetails] = useState<SkillDetails | null>(null);
  const [isLoadingDetails, setIsLoadingDetails] = useState(false);
  const resolvedTopSource = details?.source ?? skill.top_source ?? null;
  const openAddSkillSheet = useAppStore((state) => state.openAddSkillSheet);

  // Escape closes the panel, mirroring the backdrop's click-to-dismiss.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  // Fetch the full skill details (source + SKILL.md body) by the skill's
  // full owner/repo/slug id.
  useEffect(() => {
    let cancelled = false;

    setIsLoadingDetails(true);
    getSkillDetails(skill.id)
      .then((fetched) => {
        if (!cancelled) {
          setDetails(fetched);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setDetails(null);
        }
      })
      .finally(() => {
        if (!cancelled) {
          setIsLoadingDetails(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [skill.id]);

  return (
    <div className="fixed top-0 right-0 bottom-0 z-(--z-drawer) flex w-[560px] animate-[slideInDrawer_0.2s_ease] flex-col overflow-y-auto border-l border-border bg-bg-secondary">
      <SkillDetailHeader skill={skill} resolvedTopSource={resolvedTopSource} onClose={onClose} />
      <SkillContent
        skill={skill}
        skillMd={details?.skill_md ?? null}
        isLoading={isLoadingDetails}
      />
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
