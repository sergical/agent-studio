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

type ChoiceRead = { ok: true; choice: HarnessesChoice | null } | { ok: false };

/** An unreadable registry (`ok: false`) skips the screen: the main app
 * already runs on a corrupt registry via `read_fork_registry_or_default`,
 * so a first-run screen whose Continue can never save would trap the user
 * instead of protecting them. */
export function showScreenForChoiceRead(read: ChoiceRead): boolean {
  return read.ok && read.choice === null;
}

/** Checks the registry for a saved choice once on mount. */
export function useFirstRunGate(): FirstRunGateState {
  const [showScreen, setShowScreen] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;
    getHarnessesChoice()
      .then((choice) => {
        if (!cancelled) setShowScreen(showScreenForChoiceRead({ ok: true, choice }));
      })
      .catch(() => {
        if (!cancelled) setShowScreen(showScreenForChoiceRead({ ok: false }));
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
 * progress. Neither a detection error nor a save error blocks it: the
 * user continues with an empty or partial choice rather than being
 * trapped on a screen a failed probe or a failed write can never let
 * them leave. */
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
        // eslint-disable-next-line no-console
        console.error(invokeErrorMessage(cause));
        onSaved();
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
