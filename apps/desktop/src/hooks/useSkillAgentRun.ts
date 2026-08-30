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
  // Resolved with the run's terminal state once it reaches "finished" or
  // "error" - either from the streamed Finished event, or immediately if
  // `startSkillAgentRun` itself rejects. `waitForFinish` lets a caller await
  // one run's outcome without threading its own effect off `state.status`.
  const finishResolverRef = useRef<((finalState: SkillAgentRunState) => void) | null>(null);

  const resolveFinish = (finalState: SkillAgentRunState) => {
    finishResolverRef.current?.(finalState);
    finishResolverRef.current = null;
  };

  useEffect(() => {
    const unlistenPromise = onSkillAgentEvent((event) => {
      if (event.run_id !== runIdRef.current) return;
      setState((prev) => {
        const next = applyEvent(prev, event);
        if (next.status === "finished" || next.status === "error") resolveFinish(next);
        return next;
      });
    });
    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  const run = useCallback(async (request: SkillAgentRunRequest) => {
    setState({ ...IDLE_STATE, status: "running" });
    // Generated here and set before the invoke resolves, so an event that
    // arrives while the harness is still spawning isn't missed by the
    // `event.run_id !== runIdRef.current` filter above.
    const runId = crypto.randomUUID();
    runIdRef.current = runId;
    setState((prev) => ({ ...prev, runId }));
    try {
      await startSkillAgentRun(request, runId);
    } catch (err) {
      runIdRef.current = undefined;
      const errorMessage = err instanceof Error ? err.message : String(err);
      setState((prev) => {
        const next: SkillAgentRunState = { ...prev, runId, status: "error", errorMessage };
        resolveFinish(next);
        return next;
      });
    }
  }, []);

  /** Resolves with the current run's terminal state once it finishes,
   * errors, or the start itself rejected. A fresh `run()` call replaces
   * whichever resolver is pending, same as `runIdRef` replacing the run id. */
  const waitForFinish = useCallback((): Promise<SkillAgentRunState> => {
    return new Promise((resolve) => {
      finishResolverRef.current = resolve;
    });
  }, []);

  const cancel = useCallback(async () => {
    if (!state.runId) return;
    await cancelSkillAgentRun(state.runId);
  }, [state.runId]);

  const reset = useCallback(async () => {
    if (runIdRef.current) {
      await cancelSkillAgentRun(runIdRef.current).catch(() => {});
    }
    runIdRef.current = undefined;
    setState(IDLE_STATE);
  }, []);

  useEffect(() => {
    return () => {
      // Fire-and-forget: the component is gone, nothing left to await into.
      if (runIdRef.current) cancelSkillAgentRun(runIdRef.current).catch(() => {});
    };
  }, []);

  return { run, cancel, reset, state, waitForFinish };
}
