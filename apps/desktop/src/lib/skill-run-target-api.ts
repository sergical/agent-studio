// ============================================================================
// Skill Studio - skill-run-target-api
// Tauri IPC wrappers for preparing, diffing, applying, and discarding a
// "Test" run's working directory. Every operation past `prepareSkillRunTarget`
// is keyed by the id it returned - the backend owns the cwd, cleanup path,
// and project path, none of which the frontend ever sees again.
// ============================================================================

import { invoke } from "@tauri-apps/api/core";
import type { SkillRunTargetInfo, SkillRunTargetRequest } from "./skill-run-target-types";

/** Prepares the working directory for a "Test" run, per `request.kind`. */
export async function prepareSkillRunTarget(
  request: SkillRunTargetRequest,
): Promise<SkillRunTargetInfo> {
  return invoke("prepare_skill_run_target", { request });
}

/**
 * The unified diff for everything that changed in the target's working
 * directory since it was prepared. Empty string when the tree is clean.
 */
export async function skillRunTargetDiff(targetId: string): Promise<string> {
  return invoke("skill_run_target_diff", { targetId });
}

/** Applies a Worktree target's diff onto its project with `git apply --3way`. */
export async function applySkillRunTargetDiff(targetId: string): Promise<void> {
  return invoke("apply_skill_run_target_diff", { targetId });
}

/** Reverts (InPlace) or removes (Worktree/Scratch) a prepared run target. */
export async function discardSkillRunTarget(targetId: string): Promise<void> {
  return invoke("discard_skill_run_target", { targetId });
}

/** Reveals a Scratch target's folder in Finder. */
export async function revealSkillRunTarget(targetId: string): Promise<void> {
  return invoke("reveal_skill_run_target", { targetId });
}
