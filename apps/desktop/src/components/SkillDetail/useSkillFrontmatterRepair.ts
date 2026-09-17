// ============================================================================
// useSkillFrontmatterRepair - Previews a malformed-YAML repair for the page's
// current deployment as soon as it's detected, and keeps that preview
// selected only while it still matches the deployment on screen.
// ============================================================================

import { useEffect, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { previewSkillFrontmatterRepair } from "../../lib/skill-api";
import { lifecycleTargetForDeployment } from "../../lib/skill-lifecycle-target";
import type { Deployment, FrontmatterRepairPreview } from "@skill-studio/lib";
import { hasMalformedYamlWarning } from "./skill-frontmatter-repair-policy";

export interface UseSkillFrontmatterRepair {
  selectedFrontmatterRepair: FrontmatterRepairPreview | null;
  setFrontmatterRepair: Dispatch<SetStateAction<FrontmatterRepairPreview | null>>;
}

export function useSkillFrontmatterRepair(
  deployment: Deployment | undefined,
): UseSkillFrontmatterRepair {
  const [frontmatterRepair, setFrontmatterRepair] = useState<FrontmatterRepairPreview | null>(null);

  useEffect(() => {
    if (!deployment || !hasMalformedYamlWarning(deployment)) return;
    let ignore = false;
    previewSkillFrontmatterRepair(lifecycleTargetForDeployment(deployment))
      .then((preview) => {
        if (!ignore && preview.deployment_id === deployment.id) setFrontmatterRepair(preview);
      })
      .catch(() => undefined);
    return () => {
      ignore = true;
    };
  }, [deployment]);

  const frontmatterRepairDeploymentId = frontmatterRepair?.deployment_id;
  const currentDeploymentId = deployment?.id;
  const selectedFrontmatterRepair =
    frontmatterRepairDeploymentId === currentDeploymentId ? frontmatterRepair : null;

  return { selectedFrontmatterRepair, setFrontmatterRepair };
}
