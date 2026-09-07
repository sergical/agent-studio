// ============================================================================
// Skill Studio - skill agent transcript state tests
// ============================================================================

import { describe, expect, it } from "vitest";
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
  it("distinguishes failed runs from finished runs", () => {
    expect(skillAgentRunTerminalLabel("error")).toBe("Failed");
    expect(skillAgentRunTerminalLabel("finished")).toBe("Finished");
  });
});
