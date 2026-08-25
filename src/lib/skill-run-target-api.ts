// ============================================================================
// Skill Studio - skill-run-target-api
// Tauri IPC wrappers for preparing, diffing, applying, and discarding a
// "Test" run's working directory
// ============================================================================

import { invoke } from "@tauri-apps/api/core";
import type { SkillRunTarget, SkillRunTargetRequest } from "./skill-run-target-types";

/** Prepares the working directory for a "Test" run, per `request.kind`. */
export async function prepareSkillRunTarget(
  request: SkillRunTargetRequest,
): Promise<SkillRunTarget> {
  return invoke("prepare_skill_run_target", { request });
}

/**
 * The unified diff for everything that changed in `target`'s working
 * directory since it was prepared. Empty string when the tree is clean.
 */
export async function skillRunTargetDiff(target: SkillRunTarget): Promise<string> {
  return invoke("skill_run_target_diff", { target });
}

/** Applies a Worktree target's diff onto `projectPath` with `git apply --3way`. */
export async function applySkillRunTargetDiff(
  target: SkillRunTarget,
  projectPath: string,
): Promise<void> {
  return invoke("apply_skill_run_target_diff", { target, projectPath });
}

/** Reverts (InPlace) or removes (Worktree/Scratch) a prepared run target. */
export async function discardSkillRunTarget(target: SkillRunTarget): Promise<void> {
  return invoke("discard_skill_run_target", { target });
}

/** Reveals a Scratch target's folder in Finder. */
export async function revealSkillRunTarget(target: SkillRunTarget): Promise<void> {
  return invoke("reveal_skill_run_target", { target });
}
