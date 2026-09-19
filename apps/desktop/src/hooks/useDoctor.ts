// ============================================================================
// Skill Studio - useDoctor
// Talks to skill-api.ts for the Settings "Doctor" card (unit 5.3) so the
// component itself stays presentational - see the layering rule in
// .oxlintrc.json (components go through the store or a hook, not skill-api
// directly).
// ============================================================================

import { useEffect, useRef, useState } from "react";
import type { DoctorReport } from "@skill-studio/lib";
import { invokeErrorMessage, onDoctorReport, runDoctor } from "../lib/skill-api";

interface DoctorReportSubscription {
  isCancelled: () => boolean;
  listen: typeof onDoctorReport;
  onReport: (report: DoctorReport) => void;
  /** Whether a report (startup or on-demand) has already landed by the time
   * the listener finishes registering. */
  hasReport: () => boolean;
  /** Runs an on-demand pass. Called once, only if `hasReport()` is still
   * false once the listener has settled (registered, or given up because
   * there is no live event backend) - a startup pass that emitted before
   * this listener registered, or no startup pass at all, must not leave the
   * card reading "Not run yet" forever. */
  runFallback: () => void;
}

/** Register the `skills://doctor` listener, then dispose it immediately if
 * unmount raced ahead of registration finishing - same shape as
 * `useSkillSnapshot`'s `startSkillSnapshotSubscription`, minus an initial
 * read: the startup pass this listens for is the only "initial" value, and
 * it arrives as an event, not a value `useDoctor` fetches itself. */
export async function startDoctorReportSubscription({
  isCancelled,
  listen,
  onReport,
  hasReport,
  runFallback,
}: DoctorReportSubscription): Promise<(() => void) | undefined> {
  try {
    const unlisten = await listen((candidate) => {
      if (!isCancelled()) onReport(candidate);
    });
    if (isCancelled()) {
      unlisten();
      return undefined;
    }
    if (!hasReport()) runFallback();
    return unlisten;
  } catch {
    // No live event backend (e.g. the dev harness) - fall back to an
    // on-demand run so the card still gets a report.
    if (!isCancelled() && !hasReport()) runFallback();
    return undefined;
  }
}

interface UseDoctorResult {
  /** `null` before any pass (startup or on demand) has reported back. */
  report: DoctorReport | null;
  error: string | null;
  running: boolean;
  run: () => void;
}

/** Subscribes to the automatic startup pass on mount, and exposes `run` for
 * the card's "Run doctor" button - both land in the same `report`/`error`
 * state, so the card shows whichever pass finished most recently. Falls
 * back to an on-demand run if no report has landed once the subscription
 * settles (Settings mounted after the startup pass already emitted its
 * event, or there is no live event backend at all), so the card never
 * reads "Not run yet" when a report already exists or is one call away. */
export function useDoctor(): UseDoctorResult {
  const [report, setReport] = useState<DoctorReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const hasReportRef = useRef(false);
  useEffect(() => {
    hasReportRef.current = report !== null;
  }, [report]);

  function run() {
    setRunning(true);
    setError(null);
    runDoctor()
      .then(setReport)
      .catch((cause: unknown) => setError(invokeErrorMessage(cause)))
      .finally(() => setRunning(false));
  }

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    void startDoctorReportSubscription({
      isCancelled: () => cancelled,
      listen: onDoctorReport,
      onReport: (candidate) => {
        // A report resolves any earlier failed manual run - a stale error
        // must not outlive the report that answers it.
        setError(null);
        setReport(candidate);
      },
      hasReport: () => hasReportRef.current,
      runFallback: run,
    }).then((registeredUnlisten) => {
      if (cancelled) registeredUnlisten?.();
      else unlisten = registeredUnlisten;
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- mount-only: registers once, and `run`/`hasReportRef` read current state through refs and setters that do not need to retrigger this effect.
  }, []);

  return { report, error, running, run };
}
