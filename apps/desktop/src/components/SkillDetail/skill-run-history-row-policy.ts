// ============================================================================
// Skill Studio - skill run history row policy
// The outcome label and color tone for one Runs-history row, decided off the
// recorded `SkillRunRecord`. Extracted from `SkillRunHistory` so the
// cancel-vs-failure distinction is pure and unit-testable - mirroring how
// `skill-agent-transcript-policy` was split out for the transcript footer.
// ============================================================================

import type { SkillRunRecord } from "@skill-studio/lib";

export type SkillRunRowTone = "success" | "error" | "tertiary";

export interface SkillRunRowOutcome {
  label: string;
  tone: SkillRunRowTone;
}

/**
 * The outcome label and color tone for one run-history row. A judge verdict
 * is authoritative for "Test" rows; otherwise a user-initiated cancel is
 * "Cancelled" (neutral grey), a successful run is "OK" (green), and anything
 * else is "Failed" (red). The cancel distinction keys off the record's first-
 * class `cancelled` flag, never the `final_text` magic string, so it can't
 * collide with a genuine failure whose last assistant message was "Cancelled".
 */
export function skillRunHistoryRowOutcome(record: SkillRunRecord): SkillRunRowOutcome {
  if (record.judge) {
    return record.judge.passed
      ? { label: "Passed", tone: "success" }
      : { label: "Failed", tone: "error" };
  }
  if (record.cancelled) return { label: "Cancelled", tone: "tertiary" };
  if (record.ok) return { label: "OK", tone: "success" };
  return { label: "Failed", tone: "error" };
}
