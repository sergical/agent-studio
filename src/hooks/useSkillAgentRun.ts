// ============================================================================
// useSkillAgentRun - Runs one skill's local harness invocation and exposes
// its streamed transcript, subscribing to the shared "skill-agent://event"
// once per mount and filtering by this hook's own run id
// ============================================================================

import { useCallback, useEffect, useRef, useState } from "react";
import { cancelSkillAgentRun, onSkillAgentEvent, startSkillAgentRun } from "../lib/skill-agent-api";
import type { SkillAgentEvent, SkillAgentRunRequest, SkillLoaded } from "../lib/skill-agent-types";

export type SkillAgentRunStatus = "idle" | "running" | "finished" | "error";

/** Everything the transcript UI needs to render one run's progress. */
export interface SkillAgentRunState {
  status: SkillAgentRunStatus;
  runId: string | undefined;
  events: SkillAgentEvent[];
  finalText: string | undefined;
  sessionId: string | undefined;
  costUsd: number | undefined;
  durationMs: number | undefined;
  skillLoaded: SkillLoaded | undefined;
  errorMessage: string | undefined;
}

const IDLE_STATE: SkillAgentRunState = {
  status: "idle",
  runId: undefined,
  events: [],
  finalText: undefined,
  sessionId: undefined,
  costUsd: undefined,
  durationMs: undefined,
  skillLoaded: undefined,
  errorMessage: undefined,
};

/** Folds one incoming `SkillAgentEvent` into the running state. */
function applyEvent(state: SkillAgentRunState, event: SkillAgentEvent): SkillAgentRunState {
  const events = [...state.events, event].sort((a, b) => a.seq - b.seq);
  switch (event.kind.kind) {
    case "finished":
      return {
        ...state,
        events,
        status: event.kind.ok ? "finished" : "error",
        finalText: event.kind.final_text,
        sessionId: event.kind.session_id,
        costUsd: event.kind.cost_usd,
        durationMs: event.kind.duration_ms,
        skillLoaded: event.kind.skill_loaded,
      };
    case "error":
      return { ...state, events, errorMessage: event.kind.message };
    default:
      return { ...state, events };
  }
}

/**
 * `run(request)` starts a fresh run and streams its events into `state`;
 * `cancel()` kills the current run; `reset()` clears the transcript and
 * session without starting a new run (harness switch, "New session").
 */
export function useSkillAgentRun() {
  const [state, setState] = useState<SkillAgentRunState>(IDLE_STATE);
  const runIdRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    const unlistenPromise = onSkillAgentEvent((event) => {
      if (event.run_id !== runIdRef.current) return;
      setState((prev) => applyEvent(prev, event));
    });
    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  const run = useCallback(async (request: SkillAgentRunRequest) => {
    setState({ ...IDLE_STATE, status: "running" });
    const runId = await startSkillAgentRun(request);
    runIdRef.current = runId;
    setState((prev) => ({ ...prev, runId }));
  }, []);

  const cancel = useCallback(async () => {
    if (!state.runId) return;
    await cancelSkillAgentRun(state.runId);
  }, [state.runId]);

  const reset = useCallback(() => {
    runIdRef.current = undefined;
    setState(IDLE_STATE);
  }, []);

  return { run, cancel, reset, state };
}
