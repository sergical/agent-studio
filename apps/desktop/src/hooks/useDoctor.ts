// ============================================================================
// Skill Studio - useDoctor
// Talks to skill-api.ts for the Settings "Doctor" card (unit 5.3) so the
// component itself stays presentational - see the layering rule in
// .oxlintrc.json (components go through the store or a hook, not skill-api
// directly).
// ============================================================================

import { useEffect, useState } from "react";
import type { DoctorReport } from "@skill-studio/lib";
import { invokeErrorMessage, onDoctorReport, runDoctor } from "../lib/skill-api";

interface DoctorReportSubscription {
  isCancelled: () => boolean;
  listen: typeof onDoctorReport;
  onReport: (report: DoctorReport) => void;
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
}: DoctorReportSubscription): Promise<(() => void) | undefined> {
  try {
    const unlisten = await listen((candidate) => {
      if (!isCancelled()) onReport(candidate);
    });
    if (isCancelled()) {
      unlisten();
      return undefined;
    }
    return unlisten;
  } catch {
    // No live event backend (e.g. the dev harness) - `run` still works.
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
 * state, so the card shows whichever pass finished most recently. */
export function useDoctor(): UseDoctorResult {
  const [report, setReport] = useState<DoctorReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    void startDoctorReportSubscription({
      isCancelled: () => cancelled,
      listen: onDoctorReport,
      onReport: setReport,
    }).then((registeredUnlisten) => {
      if (cancelled) registeredUnlisten?.();
      else unlisten = registeredUnlisten;
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  function run() {
    setRunning(true);
    setError(null);
    runDoctor()
      .then(setReport)
      .catch((cause: unknown) => setError(invokeErrorMessage(cause)))
      .finally(() => setRunning(false));
  }

  return { report, error, running, run };
}
