// ============================================================================
// Skill Studio - Activity restore safety tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { SkillEvent } from "@skill-studio/lib";
import { canRestoreSkillEvent, shouldOfferForceRestore } from "./skill-history-restore-policy";

function event(forceRestorable: boolean): SkillEvent {
  return {
    id: "event",
    ts: "2026-09-05T00:00:00Z",
    kind: "explode_shared_dir",
    skill: "find-bugs",
    status: "done",
    restorable: true,
    force_restorable: forceRestorable,
  };
}

describe("shouldOfferForceRestore", () => {
  it("does not offer force when an independent copy boundary makes it unsafe", () => {
    expect(shouldOfferForceRestore(event(false), "/root has changed since the event")).toBe(false);
  });

  it("offers force for an ordinary drift refusal when the backend allows it", () => {
    expect(shouldOfferForceRestore(event(true), "/root has changed since the event")).toBe(true);
  });
});

describe("canRestoreSkillEvent", () => {
  it("hides restore for a non-restorable independent-copy undo event", () => {
    expect(canRestoreSkillEvent({ ...event(false), kind: "restore", restorable: false })).toBe(
      false,
    );
  });
});
