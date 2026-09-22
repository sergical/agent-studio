// ============================================================================
// Skill Studio - FirstRunScreen
// Shown once, before the app's normal chrome, when the registry's
// `harnesses` key is absent (see App.tsx). Detects harnesses off the UI
// thread (useFirstRun.ts's `useFirstRunScreen`), lets the user keep or
// remove a row and choose whether to search harness history for project
// folders, and saves the choice so the next launch skips this screen. Nothing
// here decides what a row means - the `HarnessDetection` state comes
// straight from the core; this component only renders it and collects the
// keep/remove choice.
// ============================================================================

import { useEffect } from "react";
import { Button, Checkbox, Switch } from "@skill-studio/ui";
import type { HarnessDetection } from "@skill-studio/lib";
import { continueIsBlocked, useFirstRunGate, useFirstRunScreen } from "../../hooks/useFirstRun";

interface FirstRunScreenProps {
  onComplete: () => void;
}

/** The only thing a user needs from a row's state: will Activity show
 * anything for this harness. `used` already answers that by being kept, so
 * it gets no secondary text. */
function secondaryText(state: HarnessDetection["state"]): string | null {
  switch (state) {
    case "used":
      return null;
    case "installed":
    case "configured":
      return "No activity yet";
    case "data_only":
      return "Settings found, command not on PATH";
    case "not_found":
      return "Not found";
  }
}

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
          <p className="text-sm text-muted-foreground">
            Pick the agents Skill Studio manages. It installs and syncs skills for them and shows
            how they use them. These are on this Mac:
          </p>
        </div>

        {error != null && <p className="text-sm text-destructive">{error}</p>}

        {rows == null && error == null && (
          <p className="text-sm text-muted-foreground">Detecting harnesses...</p>
        )}

        {rows != null && (
          <ul className="divide-y divide-border rounded-md border border-border">
            {rows.map((row) => {
              const secondary = secondaryText(row.state);
              const secondaryId = secondary != null ? `${row.id}-secondary` : undefined;
              return (
                <li key={row.id} className="flex items-center justify-between gap-3 px-4 py-3">
                  <label className="flex items-center gap-3">
                    <Checkbox
                      checked={kept.has(row.id)}
                      onCheckedChange={(checked) => toggleRow(row.id, checked === true)}
                      aria-describedby={secondaryId}
                    />
                    <span className="text-sm font-medium">{row.display_name}</span>
                  </label>
                  {secondary != null && (
                    <span id={secondaryId} className="text-sm text-muted-foreground">
                      {secondary}
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        )}

        <label className="flex items-center justify-between gap-3">
          <span className="text-sm">Find my projects from agent history</span>
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
