// ============================================================================
// useSkillFrontmatterRepair - Previews a frontmatter repair once per file
// state (deployment id + content hash) as soon as it's detected, and keeps
// that preview only while it still matches the file on screen. Reports when
// the preview has settled so the page never swaps one repair button for another.
// ============================================================================

import { useEffect, useState } from "react";
import { previewSkillFrontmatterRepair } from "../../lib/skill-api";
import { lifecycleTargetForDeployment } from "../../lib/skill-lifecycle-target";
import type { Deployment, FrontmatterRepairPreview } from "@skill-studio/lib";
import {
  frontmatterPreviewKey,
  frontmatterRepairKindsFor,
} from "./skill-frontmatter-repair-policy";

interface UseSkillFrontmatterRepair {
  /** One preview per repair kind the backend accepted; a refused kind is absent. */
  frontmatterRepairs: FrontmatterRepairPreview[];
  /** True when no backend preview is pending for the file on screen: each answered, failed, or does not apply. */
  isFrontmatterPreviewSettled: boolean;
  clearFrontmatterRepair: () => void;
}

interface PreviewAnswer {
  key: string;
  previews: FrontmatterRepairPreview[];
}

export function useSkillFrontmatterRepair(
  deployment: Deployment | undefined,
): UseSkillFrontmatterRepair {
  const [answer, setAnswer] = useState<PreviewAnswer | null>(null);

  const key = frontmatterPreviewKey(deployment);
  const kinds = frontmatterRepairKindsFor(deployment);
  const kindsKey = kinds.join(",");
  const hasRepairableViolation = kinds.length > 0;

  useEffect(() => {
    if (!deployment || key === null || kinds.length === 0) return;
    let ignore = false;
    const target = lifecycleTargetForDeployment(deployment);
    Promise.all(
      kinds.map((kind) =>
        previewSkillFrontmatterRepair(target, kind)
          .then((preview) => (preview.deployment_id === deployment.id ? preview : null))
          .catch(() => null),
      ),
    ).then((previews) => {
      if (ignore) return;
      setAnswer({
        key,
        previews: previews.filter((preview) => preview !== null),
      });
    });
    return () => {
      ignore = true;
    };
    // Keyed on the file state, not the deployment object, which changes on every snapshot.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [key, kindsKey]);

  const current = answer !== null && answer.key === key ? answer : null;

  return {
    frontmatterRepairs: current?.previews ?? [],
    isFrontmatterPreviewSettled: !hasRepairableViolation || current !== null,
    clearFrontmatterRepair: () => key !== null && setAnswer({ key, previews: [] }),
  };
}
