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

import { useEffect, useSyncExternalStore } from "react";
import { Dithering } from "@paper-design/shaders-react";
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
    telemetryEnabled,
    setTelemetryEnabled,
    error,
    saveError,
    saving,
    continue: onContinue,
  } = useFirstRunScreen(onComplete);

  return (
    <div className="flex h-full w-full">
      <WelcomeArt />
      <div className="flex min-w-0 flex-1 flex-col overflow-y-auto">
        <div data-tauri-drag-region className="h-9 shrink-0" />
        <div className="flex flex-1 items-center justify-center px-10 pb-10">
          <div className="w-full max-w-md space-y-6">
            <div className="space-y-1">
              <h1 className="text-xl font-semibold">Welcome to Skill Studio</h1>
              <p className="text-sm text-muted-foreground">
                Pick the agents Skill Studio manages. It installs and syncs skills for them and
                shows how they use them. These are on this Mac:
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

            <div className="space-y-1">
              <label className="flex items-center justify-between gap-3">
                <span className="text-sm">Send crash reports and timings to Skill Studio</span>
                <Switch
                  checked={telemetryEnabled}
                  onCheckedChange={setTelemetryEnabled}
                  aria-describedby="telemetry-help"
                />
              </label>
              <p id="telemetry-help" className="text-sm text-muted-foreground">
                Skill Studio removes everything sensitive first: no skill names, files, paths, or
                machine name. It uses this data only to improve the app. You can change this later
                in Settings.
              </p>
            </div>

            {saveError != null && <p className="text-sm text-destructive">{saveError}</p>}

            <Button
              onClick={onContinue}
              disabled={continueIsBlocked({ rows, error, saving })}
              className="w-full"
            >
              {saving ? "Saving..." : "Continue"}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

function subscribeReducedMotion(onChange: () => void) {
  const query = window.matchMedia(REDUCED_MOTION_QUERY);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}

function useReducedMotion() {
  return useSyncExternalStore(
    subscribeReducedMotion,
    () => window.matchMedia(REDUCED_MOTION_QUERY).matches,
    () => false,
  );
}

/** Brand panel. The window's traffic lights overlay its top-left corner,
 * so the whole panel doubles as a drag region; every child ignores pointer
 * events so the drag reaches it. */
function WelcomeArt() {
  const reduceMotion = useReducedMotion();
  return (
    <div data-tauri-drag-region className="relative w-[44%] shrink-0 overflow-hidden bg-[#150b2e]">
      <div className="pointer-events-none absolute inset-0">
        <Dithering
          width="100%"
          height="100%"
          colorBack="#150b2e"
          colorFront="#5a45c4"
          // oxlint-disable-next-line anti-slop/no-shape-in-symbol-names -- Paper Shaders' own prop name
          shape="warp"
          type="4x4"
          size={3}
          scale={0.8}
          speed={reduceMotion ? 0 : 0.35}
        />
      </div>
      <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(ellipse_60%_38%_at_50%_50%,#150b2e_40%,#150b2e00_100%)]" />
      <div className="pointer-events-none relative flex h-full flex-col items-center justify-center gap-4 p-10 text-center select-none">
        <img
          src="/skill-studio-logo.png"
          alt=""
          width={112}
          height={112}
          draggable={false}
          className="size-28 drop-shadow-[0_12px_24px_rgb(0_0_0/0.5)]"
        />
        <div className="space-y-1.5">
          <p className="text-xl font-semibold tracking-tight text-white">Skill Studio</p>
          <p className="max-w-64 text-sm text-balance text-white/80">
            Manage, sync, and test agent skills
          </p>
        </div>
      </div>
    </div>
  );
}
