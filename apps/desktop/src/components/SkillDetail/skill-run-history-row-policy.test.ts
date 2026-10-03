// ============================================================================
// Skill Studio - skill run history row policy tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { SkillRunRecord } from "@skill-studio/lib";
import { skillRunHistoryRowOutcome } from "./skill-run-history-row-policy";

function fixtureRecord(overrides: Partial<SkillRunRecord> = {}): SkillRunRecord {
  return {
    id: "r",
    skill_name: "demo",
    harness: "claude-code",
    action: "ask",
    target_kind: undefined,
    started_at: "2026-01-01T00:00:00Z",
    duration_ms: 0,
    ok: false,
    cancelled: false,
    skill_loaded: "unknown",
    judge: undefined,
    cost_usd: undefined,
    final_text: "",
    transcript_path: "r.events.jsonl",
    ...overrides,
  };
}

describe("skillRunHistoryRowOutcome", () => {
  it("labels a cancelled Ask run as Cancelled (neutral), not Failed", () => {
    const cancelled = fixtureRecord({
      action: "ask",
      cancelled: true,
      ok: false,
      final_text: "Cancelled",
    });
    expect(skillRunHistoryRowOutcome(cancelled)).toEqual({ label: "Cancelled", tone: "tertiary" });
  });

  it("labels a cancelled Test run (no judge) as Cancelled, not Failed", () => {
    const cancelledTest = fixtureRecord({
      action: "test",
      cancelled: true,
      ok: false,
      judge: undefined,
      final_text: "Cancelled",
    });
    expect(skillRunHistoryRowOutcome(cancelledTest)).toEqual({
      label: "Cancelled",
      tone: "tertiary",
    });
  });

  it("labels a genuine failure as Failed (red), distinct from cancel", () => {
    const failed = fixtureRecord({ action: "ask", ok: false, final_text: "" });
    expect(skillRunHistoryRowOutcome(failed)).toEqual({ label: "Failed", tone: "error" });
  });

  it("does not treat a failure whose final_text happens to be 'Cancelled' as a cancel", () => {
    // The path-(c) collision the magic-string check would have hit: a genuine
    // failure (cancelled: false) whose harness last message was "Cancelled"
    // stays "Failed" because the flag, not the text, decides.
    const collision = fixtureRecord({ ok: false, cancelled: false, final_text: "Cancelled" });
    expect(skillRunHistoryRowOutcome(collision)).toEqual({ label: "Failed", tone: "error" });
  });

  it("a judge verdict overrides ok/cancelled for Test rows", () => {
    const passed = fixtureRecord({
      action: "test",
      ok: true,
      judge: { passed: true, sentence: "good" },
    });
    expect(skillRunHistoryRowOutcome(passed)).toEqual({ label: "Passed", tone: "success" });
    const failed = fixtureRecord({
      action: "test",
      ok: false,
      judge: { passed: false, sentence: "bad" },
    });
    expect(skillRunHistoryRowOutcome(failed)).toEqual({ label: "Failed", tone: "error" });
  });

  it("labels a successful Ask run as OK", () => {
    const ok = fixtureRecord({ action: "ask", ok: true, final_text: "done" });
    expect(skillRunHistoryRowOutcome(ok)).toEqual({ label: "OK", tone: "success" });
  });

  it("reads a legacy record (cancelled absent -> false, ok:false) as Failed", () => {
    // After serde default, a record written before the `cancelled` field
    // arrives with cancelled: false, so a pre-fix cancel stays "Failed" -
    // the fix is forward-only and never keys off the final_text sentinel.
    const legacy = fixtureRecord({ ok: false, cancelled: false, final_text: "Cancelled" });
    expect(skillRunHistoryRowOutcome(legacy)).toEqual({ label: "Failed", tone: "error" });
  });
});
