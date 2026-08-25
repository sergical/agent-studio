// ============================================================================
// Skill Studio - skill-agent-api
// Tauri IPC wrappers for the local harness runner: start/cancel a run, and
// create/remove the scratch directory it runs in
// ============================================================================

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { SkillAgentEvent, SkillAgentRunRequest } from "./skill-agent-types";

/** Event name every `SkillAgentEvent` is emitted on. */
export const SKILL_AGENT_EVENT = "skill-agent://event";

/**
 * Start a headless harness run for one skill. Resolves with the run id used
 * to filter `onSkillAgentEvent` and to call `cancelSkillAgentRun`.
 */
export async function startSkillAgentRun(request: SkillAgentRunRequest): Promise<string> {
  return invoke("start_skill_agent_run", { request });
}

/**
 * Kill a run's child process. The runner always emits the run's terminating
 * `finished` event itself, even after cancellation.
 */
export async function cancelSkillAgentRun(runId: string): Promise<void> {
  return invoke("cancel_skill_agent_run", { runId });
}

/**
 * Create a scratch directory containing only the given skills, each copied
 * to `.agents/skills/<name>` and symlinked into `.claude/skills` and
 * `.pi/skills`. Returns the scratch directory's path.
 */
export async function createSkillScratchDir(
  skills: [name: string, folderPath: string][],
): Promise<string> {
  return invoke("create_skill_scratch_dir", { skills });
}

/** Remove a scratch directory created by `createSkillScratchDir`. */
export async function removeSkillScratchDir(path: string): Promise<void> {
  return invoke("remove_skill_scratch_dir", { path });
}

/**
 * Subscribe to `skill-agent://event`, emitted for every line of every run's
 * transcript. Returns a promise for the unlisten function, matching Tauri's
 * own `listen` signature.
 */
export function onSkillAgentEvent(cb: (event: SkillAgentEvent) => void): Promise<UnlistenFn> {
  return listen<SkillAgentEvent>(SKILL_AGENT_EVENT, (event) => cb(event.payload));
}
