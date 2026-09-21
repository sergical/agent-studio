// ============================================================================
// Skill Studio - FirstRunScreen
// Shown once, before the app's normal chrome, when the registry's
// `harnesses` key is absent (see App.tsx). Detects harnesses off the UI
// thread (useFirstRun.ts's `useFirstRunScreen`), lets the user keep or
// remove a row and choose whether to search harness history for project
// folders, and saves the choice so the next launch skips this screen. Nothing
// here decides what a row means - the state label and the value shown for
// each signal come straight from the core's `HarnessDetection`; this
// component only renders it and collects the keep/remove choice.
// ============================================================================

import { useEffect } from "react";
import { Button, Checkbox, Switch } from "@skill-studio/ui";
import type { HarnessDetection } from "@skill-studio/lib";
import { continueIsBlocked, useFirstRunGate, useFirstRunScreen } from "../../hooks/useFirstRun";

interface FirstRunScreenProps {
  onComplete: () => void;
}

const STATE_LABEL = {
  not_found: "Not found",
  data_only: "Data only",
  installed: "Installed",
  configured: "Configured",
  used: "Used",
} satisfies Record<HarnessDetection["state"], string>;

/** Renders nothing until the saved-choice check resolves, and skips straight
 * to `onComplete` when a choice already exists, so a returning user never
 * sees this screen flash on launch. */
export function FirstRunGate({ onComplete }: FirstRunScreenProps) {
  const { showScreen } = useFirstRunGate();

  useEffect(() => {
    if (showScreen === false) onComplete();
  }, [showScreen, onComplete]);

  if (showScreen !== true) return null;
  return <FirstRunScreenBody onComplete={onComplete} />;
}

function FirstRunScreenBody({ onComplete }: FirstRunScreenProps) {
  const {
    rows,
    kept,
    toggleRow,
    searchProjectFolders,
    setSearchProjectFolders,
    error,
    saving,
    continue: onContinue,
  } = useFirstRunScreen(onComplete);

  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-6 p-8">
      <div className="w-full max-w-lg space-y-6">
        <div className="space-y-1">
          <h1 className="text-xl font-semibold">Welcome to Skill Studio</h1>
          <p className="text-sm text-muted-foreground">Here is what we found on this Mac.</p>
        </div>

        {error != null && <p className="text-sm text-destructive">{error}</p>}

        {rows == null && error == null && (
          <p className="text-sm text-muted-foreground">Detecting harnesses...</p>
        )}

        {rows != null && (
          <ul className="divide-y divide-border rounded-md border border-border">
            {rows.map((row) => (
              <li key={row.id} className="flex flex-col gap-1 px-4 py-3">
                <div className="flex items-center justify-between gap-3">
                  <label className="flex items-center gap-3">
                    <Checkbox
                      checked={kept.has(row.id)}
                      onCheckedChange={(checked) => toggleRow(row.id, checked === true)}
                    />
                    <span className="text-sm font-medium">{row.display_name}</span>
                  </label>
                  <span className="text-sm text-muted-foreground">
                    {STATE_LABEL[row.state]}
                    {row.version.value != null ? ` · ${row.version.value}` : ""}
                  </span>
                </div>
                {row.state === "data_only" && (
                  <p className="pl-8 text-sm text-muted-foreground">
                    Settings or history found, but its command is not on your PATH.
                  </p>
                )}
              </li>
            ))}
          </ul>
        )}

        <label className="flex items-center justify-between gap-3">
          <span className="text-sm">Search harness history for project folders</span>
          <Switch checked={searchProjectFolders} onCheckedChange={setSearchProjectFolders} />
        </label>

        <Button
          onClick={onContinue}
          disabled={continueIsBlocked({ rows, error, saving })}
          className="w-full"
        >
          {saving ? "Saving..." : "Continue"}
        </Button>
      </div>
    </div>
  );
}
