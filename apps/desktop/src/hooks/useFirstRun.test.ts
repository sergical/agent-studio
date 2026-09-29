import { describe, expect, it, vi } from "vitest";
import {
  buildFirstRunSave,
  continueIsBlocked,
  FIRST_RUN_TELEMETRY_DEFAULT,
  saveChoiceThenOpenApp,
  showScreenForChoiceRead,
} from "./useFirstRun";

describe("showScreenForChoiceRead", () => {
  it("an unreadable registry opens the app instead of trapping the user on the first-run screen", () => {
    expect(showScreenForChoiceRead({ ok: false })).toBe(false);
  });

  it("no saved choice shows the screen and a saved choice skips it", () => {
    expect(showScreenForChoiceRead({ ok: true, choice: null })).toBe(true);
    expect(
      showScreenForChoiceRead({
        ok: true,
        choice: { kept: [], search_project_folders: true, saved_at: "2026-01-01T00:00:00.000Z" },
      }),
    ).toBe(false);
  });
});

describe("continueIsBlocked", () => {
  it("a detection error still lets the user continue with an empty choice or names the screen that traps them", () => {
    expect(continueIsBlocked({ rows: null, error: "probe failed", saving: false })).toBe(false);
  });

  it("detection in flight blocks continue until rows or an error arrive", () => {
    expect(continueIsBlocked({ rows: null, error: null, saving: false })).toBe(true);
    expect(continueIsBlocked({ rows: [], error: null, saving: false })).toBe(false);
  });

  it("a save in progress blocks a second continue", () => {
    expect(continueIsBlocked({ rows: [], error: null, saving: true })).toBe(true);
  });
});

describe("FIRST_RUN_TELEMETRY_DEFAULT", () => {
  it("the welcome screen starts the telemetry switch on or the registry's off default silently opts users out", () => {
    expect(FIRST_RUN_TELEMETRY_DEFAULT).toBe(true);
  });
});

describe("saveChoiceThenOpenApp", () => {
  it("a failed welcome save keeps the screen and names the reason instead of opening the app", async () => {
    const onSaved = vi.fn();
    const onSaveFailed = vi.fn();
    await saveChoiceThenOpenApp(() => Promise.reject(new Error("registry is read-only")), {
      onSaved,
      onSaveFailed,
    });
    expect(onSaved).not.toHaveBeenCalled();
    expect(onSaveFailed).toHaveBeenCalledOnce();
    expect(onSaveFailed.mock.calls[0]?.[0]).toContain("registry is read-only");
  });

  it("a successful welcome save opens the app exactly once", async () => {
    const onSaved = vi.fn();
    const onSaveFailed = vi.fn();
    await saveChoiceThenOpenApp(() => Promise.resolve(), { onSaved, onSaveFailed });
    expect(onSaved).toHaveBeenCalledOnce();
    expect(onSaveFailed).not.toHaveBeenCalled();
  });
});

describe("buildFirstRunSave", () => {
  it("carries a turned-off telemetry switch into the save call or silently re-enables it", () => {
    const { choice, telemetryEnabled } = buildFirstRunSave(
      new Set(["claude-code"]),
      false,
      false,
      "2026-01-01T00:00:00.000Z",
    );
    expect(telemetryEnabled).toBe(false);
    expect(choice).toEqual({
      kept: ["claude-code"],
      search_project_folders: false,
      saved_at: "2026-01-01T00:00:00.000Z",
    });
  });
});
