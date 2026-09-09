// ============================================================================
// Skill Studio - skill-run-history-types
// TS mirrors of the Rust skill_run_history DTOs: one completed Ask/Audit/Test
// run against a skill, and the cheap per-skill summary shown on the skill
// page and list
// ============================================================================

import type { HarnessId, SkillLoaded } from "./skill-agent-types";
import type { SkillRunTargetKind } from "./skill-run-target-types";

/** Which assistant action produced a `SkillRunRecord`. */
export type SkillRunAction = "ask" | "audit" | "test";

/** One run's judge-turn verdict, when the action ran one. */
export interface SkillRunJudge {
  passed: boolean;
  sentence: string;
}

/** One completed run against a skill: ask, audit, or test. */
export interface SkillRunRecord {
  id: string;
  skill_name: string;
  harness: HarnessId;
  action: SkillRunAction;
  target_kind?: SkillRunTargetKind;
  started_at: string;
  duration_ms: number;
  ok: boolean;
  /**
   * `true` when the user cancelled the run. The Runs-history row and detail
   * view render "Cancelled" off this flag rather than `final_text`, so a
   * genuine failure whose last assistant message happened to be "Cancelled"
   * can't be misread as a cancel. Older records without this field arrive as
   * `false` (forward-only: pre-fix cancels keep their prior "Failed" label).
   */
  cancelled: boolean;
  skill_loaded: SkillLoaded;
  judge?: SkillRunJudge;
  cost_usd?: number;
  final_text: string;
  transcript_path: string;
}

/** The cheap per-skill index shown on the skill page header and list column. */
export interface SkillRunSummary {
  at: string;
  harness: HarnessId;
  passed?: boolean;
}
