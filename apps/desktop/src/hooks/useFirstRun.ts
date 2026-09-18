// ============================================================================
// Skill Studio - useFirstRun
// Talks to skill-api.ts for the first-run harness screen (unit 3.2) so the
// component itself stays presentational - see the layering rule in
// .oxlintrc.json (components go through the store or a hook, not skill-api
// directly).
// ============================================================================

import { useEffect, useState } from "react";
import type { HarnessDetection, HarnessesChoice } from "@skill-studio/lib";
import {
  detectHarnesses,
  getHarnessesChoice,
  invokeErrorMessage,
  saveHarnessesChoice,
} from "../lib/skill-api";

interface FirstRunGateState {
  /** `null` while the saved-choice check is in flight. */
  showScreen: boolean | null;
}

/** Checks the registry for a saved choice once on mount. An unreadable
 * registry fails open to the screen rather than trap the user behind a
 * first run that can never complete. */
export function useFirstRunGate(): FirstRunGateState {
  const [showScreen, setShowScreen] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;
    getHarnessesChoice()
      .then((choice) => {
        if (!cancelled) setShowScreen(choice === null);
      })
      .catch(() => {
        if (!cancelled) setShowScreen(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return { showScreen };
}

interface FirstRunScreenState {
  rows: HarnessDetection[] | null;
  kept: Set<string>;
  toggleRow: (id: string, checked: boolean) => void;
  searchProjectFolders: boolean;
  setSearchProjectFolders: (value: boolean) => void;
  error: string | null;
  saving: boolean;
  continue: () => void;
}

/** Continue waits only for detection still in flight or a save in
 * progress. A detection error does not block it: the user continues with
 * an empty choice, and the next launch re-detects in the background, so a
 * failed probe can never trap them on this screen. */
export function continueIsBlocked(state: {
  rows: HarnessDetection[] | null;
  error: string | null;
  saving: boolean;
}): boolean {
  const detecting = state.rows == null && state.error == null;
  return detecting || state.saving;
}

/** Detects harnesses once on mount, tracks which rows the user keeps
 * (defaulting every non-"not_found" row to kept once detection resolves),
 * and exposes `continue` to persist the choice through
 * `saveHarnessesChoice`. */
export function useFirstRunScreen(onSaved: () => void): FirstRunScreenState {
  const [rows, setRows] = useState<HarnessDetection[] | null>(null);
  const [kept, setKept] = useState<Set<string>>(new Set());
  const [searchProjectFolders, setSearchProjectFolders] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    detectHarnesses()
      .then((report) => {
        if (cancelled) return;
        setRows(report.harnesses);
        const found = new Set<string>();
        for (const row of report.harnesses) {
          if (row.state !== "not_found") found.add(row.id);
        }
        setKept(found);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(invokeErrorMessage(cause));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  function toggleRow(id: string, checked: boolean) {
    const next = new Set(kept);
    if (checked) next.add(id);
    else next.delete(id);
    setKept(next);
  }

  function continueToApp() {
    setSaving(true);
    const choice: HarnessesChoice = {
      kept: Array.from(kept),
      search_project_folders: searchProjectFolders,
      saved_at: new Date().toISOString(),
    };
    saveHarnessesChoice(choice)
      .then(onSaved)
      .catch((cause: unknown) => {
        setError(invokeErrorMessage(cause));
        setSaving(false);
      });
  }

  return {
    rows,
    kept,
    toggleRow,
    searchProjectFolders,
    setSearchProjectFolders,
    error,
    saving,
    continue: continueToApp,
  };
}
