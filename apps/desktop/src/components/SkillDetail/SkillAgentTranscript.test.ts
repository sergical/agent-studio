// ============================================================================
// Skill Studio - skill agent transcript state tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { SkillAgentEvent } from "@skill-studio/lib";
import type { SkillAgentRunState } from "../../hooks/useSkillAgentRun";
import {
  skillAgentRunHasTranscript,
  skillAgentRunTerminalLabel,
  unreportedSkillAgentRunError,
} from "./skill-agent-transcript-policy";

function fixtureRunState(overrides: Partial<SkillAgentRunState> = {}): SkillAgentRunState {
  return {
    status: "idle",
    runId: undefined,
    events: [],
    finalText: undefined,
    sessionId: undefined,
    costUsd: undefined,
    durationMs: undefined,
    skillLoaded: undefined,
    errorMessage: undefined,
    ...overrides,
  };
}

describe("Assistant run errors", () => {
  it("shows a harness start failure even when no event was streamed", () => {
    const state = fixtureRunState({
      status: "error",
      runId: "run-1",
      errorMessage: "Claude Code executable was not found",
    });

    expect(skillAgentRunHasTranscript(state)).toBe(true);
    expect(unreportedSkillAgentRunError(state)).toBe("Claude Code executable was not found");
  });

  it("does not duplicate an error already present in the streamed transcript", () => {
    const state = fixtureRunState({
      status: "error",
      errorMessage: "Run failed",
      events: [
        {
          run_id: "run-1",
          seq: 1,
          at: "2026-01-01T00:00:00Z",
          kind: { kind: "error", message: "Run failed" },
        },
      ],
    });

    expect(unreportedSkillAgentRunError(state)).toBeUndefined();
  });
});

describe("Assistant terminal footer", () => {
  it("distinguishes failed, cancelled, and finished runs", () => {
    expect(skillAgentRunTerminalLabel("error")).toBe("Failed");
    expect(skillAgentRunTerminalLabel("cancelled")).toBe("Cancelled");
    expect(skillAgentRunTerminalLabel("finished")).toBe("Finished");
    expect(skillAgentRunTerminalLabel("running")).toBeUndefined();
    expect(skillAgentRunTerminalLabel("idle")).toBeUndefined();
  });
});

describe("Cancelled runs", () => {
  // The runner's cancel branch emits `Finished { ok: false, cancelled: true,
  // final_text: "Cancelled" }` and no `Error` event; `applyEvent` maps that to
  // `status: "cancelled"` with `errorMessage: undefined`. A genuine failure
  // (spawn error, or exit non-zero with no final text) keeps
  // `status: "error"` and a non-empty errorMessage or an error event. These
  // fixtures mirror each `SkillAgentRunState` so the policy can prove the two
  // are no longer collapsed to the same "Failed" label.
  const started: SkillAgentEvent = {
    run_id: "r",
    seq: 1,
    at: "2026-01-01T00:00:00Z",
    kind: { kind: "started", command: "claude-code --print ...", session_id: "s" },
  };

  function cancelledState(): SkillAgentRunState {
    const cancelledFinished: SkillAgentEvent = {
      run_id: "r",
      seq: 2,
      at: "2026-01-01T00:00:01Z",
      kind: {
        kind: "finished",
        ok: false,
        cancelled: true,
        final_text: "Cancelled",
        session_id: "s",
        cost_usd: undefined,
        duration_ms: 1234,
        skill_loaded: "unknown",
      },
    };
    return fixtureRunState({
      status: "cancelled", // applyEvent: cancelled:true -> "cancelled"
      runId: "r",
      events: [started, cancelledFinished],
      finalText: "Cancelled",
      sessionId: "s",
      durationMs: 1234,
      skillLoaded: "unknown",
      errorMessage: undefined, // no Error event on the cancel branch
    });
  }

  function failedState(): SkillAgentRunState {
    const err: SkillAgentEvent = {
      run_id: "r2",
      seq: 1,
      at: "2026-01-01T00:00:00Z",
      kind: { kind: "error", message: "exited with code 1" },
    };
    const failedFinished: SkillAgentEvent = {
      run_id: "r2",
      seq: 2,
      at: "2026-01-01T00:00:01Z",
      kind: {
        kind: "finished",
        ok: false,
        cancelled: false,
        final_text: "",
        session_id: undefined,
        cost_usd: undefined,
        duration_ms: 99,
        skill_loaded: "no",
      },
    };
    return fixtureRunState({
      status: "error",
      runId: "r2",
      events: [err, failedFinished],
      finalText: "",
      durationMs: 99,
      skillLoaded: "no",
      errorMessage: "exited with code 1",
    });
  }

  it("labels a cancelled run as Cancelled, not Failed", () => {
    const cancelled = cancelledState();
    expect(skillAgentRunTerminalLabel(cancelled.status)).toBe("Cancelled");
    expect(skillAgentRunHasTranscript(cancelled)).toBe(true);
    expect(unreportedSkillAgentRunError(cancelled)).toBeUndefined();
  });

  it("still labels a genuine failure as Failed - cancel and crash are now distinct", () => {
    const failed = failedState();
    expect(skillAgentRunTerminalLabel(failed.status)).toBe("Failed");
    // A genuine failure carries a streamed `error` event (rendered as a red
    // block); a cancel emits no error event at all - the visible distinction.
    expect(failed.events.some((e) => e.kind.kind === "error")).toBe(true);
  });

  it("a cancelled run emits no error event - its only terminal is the cancel Finished", () => {
    const cancelled = cancelledState();
    expect(cancelled.events.some((e) => e.kind.kind === "error")).toBe(false);
    expect(cancelled.events.some((e) => e.kind.kind === "finished" && e.kind.cancelled)).toBe(true);
  });

  it("a cancelled run and a genuine failure produce different terminal labels", () => {
    expect(skillAgentRunTerminalLabel(cancelledState().status)).not.toBe(
      skillAgentRunTerminalLabel(failedState().status),
    );
  });

  it("Runs-history detail (record.cancelled -> status cancelled) renders Cancelled, not Failed", () => {
    // `stateFromRecord` sets status from record.cancelled; a recorded cancel
    // has cancelled: true, final_text: "Cancelled", and no errorMessage.
    const fromRecord = fixtureRunState({
      status: "cancelled",
      runId: "r",
      events: [started],
      finalText: "Cancelled",
      durationMs: 1234,
      skillLoaded: "unknown",
      errorMessage: undefined,
    });
    expect(skillAgentRunTerminalLabel(fromRecord.status)).toBe("Cancelled");
    expect(unreportedSkillAgentRunError(fromRecord)).toBeUndefined();
    expect(fromRecord.finalText).toBe("Cancelled");
  });
});
