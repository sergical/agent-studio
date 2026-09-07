// ============================================================================
// Skill Studio - skill agent transcript policy
// Includes run-start errors that arrive before the event stream begins.
// ============================================================================

import type { SkillAgentRunState, SkillAgentRunStatus } from "../../hooks/useSkillAgentRun";

/** Returns the terminal transcript label for a completed or failed skill agent run. */
export function skillAgentRunTerminalLabel(status: SkillAgentRunStatus): string | undefined {
  if (status === "error") return "Failed";
  if (status === "finished") return "Finished";
  return undefined;
}

/** Whether a run has transcript content or a start failure that the Assistant must show. */
export function skillAgentRunHasTranscript(state: SkillAgentRunState): boolean {
  return state.events.length > 0 || state.errorMessage !== undefined;
}

/** Returns a run error only when no streamed error event already reports the same message. */
export function unreportedSkillAgentRunError(state: SkillAgentRunState): string | undefined {
  if (!state.errorMessage) return undefined;
  const hasMatchingEvent = state.events.some(
    (event) => event.kind.kind === "error" && event.kind.message === state.errorMessage,
  );
  return hasMatchingEvent ? undefined : state.errorMessage;
}
