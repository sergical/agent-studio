// ============================================================================
// Skill Studio - skill-agent-types
// TS mirrors of the Rust skill_agent_runner DTOs: one headless harness run
// against one skill, and the streamed transcript events it produces
// ============================================================================

// `HarnessId` is Rust's `skill_agent_runner::HarnessId` (see skill-types.ts) -
// re-exported here instead of redeclared so the two can't drift apart.
import type { HarnessId } from "./skill-types";
export type { HarnessId };

/** Whether a run may write to its scratch directory or only read it. */
export type WriteAccess = "read_only" | "workspace";

/** Request to start one headless harness run against one skill. */
export interface SkillAgentRunRequest {
  harness: HarnessId;
  prompt: string;
  cwd: string;
  skill_name: string;
  write_access: WriteAccess;
  /** Set to continue an earlier run's session/thread. */
  session_id?: string;
}

/** Whether the run loaded the skill under test. */
export type SkillLoaded = "yes" | "no" | "unknown";

/** The discriminated union of everything a run can report, tagged on `kind`. */
export type SkillAgentEventKind =
  | { kind: "started"; command: string; session_id?: string }
  | { kind: "assistant_text"; text: string; is_delta: boolean }
  | { kind: "tool_call"; name: string; summary: string; detail?: string }
  | { kind: "tool_result"; name: string; summary: string }
  | {
      kind: "finished";
      ok: boolean;
      final_text: string;
      session_id?: string;
      cost_usd?: number;
      duration_ms: number;
      skill_loaded: SkillLoaded;
    }
  | { kind: "error"; message: string };

/** One line of a run's transcript, in emission order (`seq`). */
export interface SkillAgentEvent {
  run_id: string;
  seq: number;
  at: string;
  kind: SkillAgentEventKind;
}
