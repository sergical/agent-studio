// ============================================================================
// Skill Studio - skill-run-history-api
// Tauri IPC wrappers for recording and reading back Ask/Audit/Test run
// history
// ============================================================================

import { invoke } from "@tauri-apps/api/core";
import type { SkillAgentEvent } from "./skill-agent-types";
import type { SkillRunRecord } from "./skill-run-history-types";

/** Records one run's summary and transcript, trimming older runs for the skill. */
export async function recordSkillRun(
  record: SkillRunRecord,
  events: SkillAgentEvent[],
): Promise<void> {
  return invoke("record_skill_run", { record, events });
}

/** Every run recorded for `skillName`, newest first, without transcripts. */
export async function listSkillRuns(skillName: string): Promise<SkillRunRecord[]> {
  return invoke("list_skill_runs", { skillName });
}

/** The transcript events recorded for run `id` against `skillName`. */
export async function readSkillRunEvents(
  skillName: string,
  id: string,
): Promise<SkillAgentEvent[]> {
  return invoke("read_skill_run_events", { skillName, id });
}
