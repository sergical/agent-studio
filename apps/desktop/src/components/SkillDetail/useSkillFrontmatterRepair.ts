// ============================================================================
// useSkillFrontmatterRepair - Previews a malformed-YAML repair once per file
// state (deployment id + content hash) as soon as it's detected, and keeps
// that preview only while it still matches the file on screen. Reports when
// the preview has settled so the page never swaps one repair button for another.
// ============================================================================

import { useEffect, useState } from "react";
import { previewSkillFrontmatterRepair } from "../../lib/skill-api";
import { lifecycleTargetForDeployment } from "../../lib/skill-lifecycle-target";
import type { Deployment, FrontmatterRepairPreview } from "@skill-studio/lib";
import { frontmatterPreviewKey, hasMalformedYamlWarning } from "./skill-frontmatter-repair-policy";

interface UseSkillFrontmatterRepair {
  selectedFrontmatterRepair: FrontmatterRepairPreview | null;
  /** True when no backend preview is pending for the file on screen: it answered, failed, or does not apply. */
  isFrontmatterPreviewSettled: boolean;
  clearFrontmatterRepair: () => void;
}

interface PreviewAnswer {
  key: string;
  preview: FrontmatterRepairPreview | null;
}

export function useSkillFrontmatterRepair(
  deployment: Deployment | undefined,
): UseSkillFrontmatterRepair {
  const [answer, setAnswer] = useState<PreviewAnswer | null>(null);

  const key = frontmatterPreviewKey(deployment);
  const isMalformed = hasMalformedYamlWarning(deployment);

  useEffect(() => {
    if (!deployment || key === null || !isMalformed) return;
    let ignore = false;
    previewSkillFrontmatterRepair(lifecycleTargetForDeployment(deployment))
      .then((preview) => {
        if (ignore) return;
        setAnswer({ key, preview: preview.deployment_id === deployment.id ? preview : null });
      })
      .catch(() => {
        if (!ignore) setAnswer({ key, preview: null });
      });
    return () => {
      ignore = true;
    };
    // Keyed on the file state, not the deployment object, which changes on every snapshot.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [key, isMalformed]);

  const current = answer !== null && answer.key === key ? answer : null;

  return {
    selectedFrontmatterRepair: current?.preview ?? null,
    isFrontmatterPreviewSettled: !isMalformed || current !== null,
    clearFrontmatterRepair: () => key !== null && setAnswer({ key, preview: null }),
  };
}
