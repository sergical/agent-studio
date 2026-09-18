import { describe, expect, it } from "vitest";
import { continueIsBlocked, showScreenForChoiceRead } from "./useFirstRun";

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
