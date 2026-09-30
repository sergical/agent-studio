// ============================================================================
// Skill Studio - useSkillSplit
// IPC side of the "Split into harness folders…" dialog: resolves the folder
// each harness copy would go to, and runs the split followed by a rescan.
// ============================================================================

import { useEffect, useState } from "react";
import type { AgentId, LifecycleTarget, SplitCopy } from "@skill-studio/lib";
import { requestSkillRescan, splitSkill, splitSkillTargets } from "../lib/skill-api";
import { useAppStore } from "../store/appStore";

/** `targets` is `null` until the folders for `harnesses` resolve. */
export function useSkillSplit(skillName: string, projectPath: string | null, harnesses: AgentId[]) {
  const addToast = useAppStore((state) => state.addToast);
  const [targets, setTargets] = useState<SplitCopy[] | null>(null);

  useEffect(() => {
    let ignore = false;
    splitSkillTargets(skillName, projectPath, harnesses)
      .then((copies) => {
        if (!ignore) setTargets(copies);
      })
      .catch((err) => {
        if (ignore) return;
        addToast({
          type: "error",
          title: "Couldn't find the harness folders",
          message: err instanceof Error ? err.message : String(err),
        });
      });
    return () => {
      ignore = true;
    };
  }, [skillName, projectPath, harnesses, addToast]);

  const split = async (target: LifecycleTarget, keep: AgentId[]) => {
    await splitSkill(target, keep);
    await requestSkillRescan();
  };

  return { targets, split };
}
