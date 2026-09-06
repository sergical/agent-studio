// ============================================================================
// Skill Studio - skill-add-operation-types
// Wire types for a background Add Skill operation: start/status/cancel/trust
// ============================================================================

import type { AddSkillOutcome, AddSkillResult } from "./skill-types";

/** One phase of a background Add Skill operation. */
export type AddSkillOperationPhase =
  | "queued"
  | "validating"
  | "fetching"
  | "installing"
  | "finalizing"
  | "reconciling"
  | "needs-trust"
  | "completed"
  | "failed"
  | "cancelled"
  | "timed-out";

/** Batch item progress: 1-based `current` of `total`. */
export interface AddSkillItemProgress {
  current: number;
  total: number;
  name: string;
}

/** Normalized repository identity that needs explicit trust. */
export interface AddSkillUntrustedSource {
  identity: string;
}

/** One status event or catch-up snapshot for an Add Skill operation. */
export interface AddSkillOperationEvent {
  operation_id: string;
  sequence: number;
  phase: AddSkillOperationPhase;
  message: string;
  item?: AddSkillItemProgress;
  result?: AddSkillResult;
  outcomes?: AddSkillOutcome[];
  error?: string;
  untrusted_source?: AddSkillUntrustedSource;
  retry_of?: string;
}
