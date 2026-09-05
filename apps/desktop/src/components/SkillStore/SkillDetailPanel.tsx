// ============================================================================
// SkillDetailPanel - Detail view for a selected skill
// ============================================================================

import { useEffect, useState } from "react";
import { DrawerContent } from "@skill-studio/ui";
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
  onInstallComplete: (result: {
    success: boolean;
    error?: string;
    skillName?: string;
    warning?: string;
  }) => void;
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

  // Fetch the full skill details (source + SKILL.md body) by the skill's
  // full owner/repo/slug id.
  useEffect(() => {
    let cancelled = false;

    // react-doctor-disable-next-line react-hooks-js/set-state-in-effect -- syncs from an external source: the getSkillDetails Tauri call below
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
    <DrawerContent
      side="right"
      className="w-[min(640px,92vw)] overflow-y-auto bg-bg-secondary"
      showCloseButton={false}
    >
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
    </DrawerContent>
  );
}
