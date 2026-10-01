// ============================================================================
// Skill Studio - nav-history tests
// Back/forward stacks behind the mouse buttons and ⌘[ / ⌘].
// ============================================================================

import { describe, expect, it } from "vitest";
import type { ActiveView } from "../store/appStore";
import {
  EMPTY_NAV_HISTORY,
  NAV_HISTORY_LIMIT,
  recordNavigation,
  stepBack,
  stepForward,
} from "./nav-history";

const home: ActiveView = { kind: "home" };
const skills: ActiveView = { kind: "skills" };
const settings: ActiveView = { kind: "settings" };
const skillPage = (name: string): Extract<ActiveView, { kind: "skill" }> => ({
  kind: "skill",
  name,
  from: home,
});
const always = () => true;

describe("recordNavigation", () => {
  it("pushes the location being left, so Back has somewhere to go", () => {
    const history = recordNavigation(EMPTY_NAV_HISTORY, home, skills);
    expect(history.back).toEqual([home]);
  });

  it("records nothing when the target is the current place, so Back is not a no-op step", () => {
    const history = recordNavigation(EMPTY_NAV_HISTORY, skillPage("a"), {
      ...skillPage("a"),
      intent: "compare",
    });
    expect(history).toBe(EMPTY_NAV_HISTORY);
  });

  it("stores a skill entry without its one-shot intent, so stepping back never reopens the dialog", () => {
    const withIntent: ActiveView = { ...skillPage("a"), intent: "compare" };
    const history = recordNavigation(EMPTY_NAV_HISTORY, withIntent, home);
    expect(history.back[0]).toMatchObject({ kind: "skill", name: "a", intent: undefined });
  });

  it("clears the forward stack on a new navigation after going back", () => {
    let history = recordNavigation(EMPTY_NAV_HISTORY, home, skills);
    history = stepBack(history, skills, always)!.history;
    expect(history.forward).toEqual([skills]);

    history = recordNavigation(history, home, settings);
    expect(history.forward).toEqual([]);
  });

  it("keeps only the newest entries past the cap, so memory stays bounded", () => {
    let history = EMPTY_NAV_HISTORY;
    for (let i = 0; i < NAV_HISTORY_LIMIT + 10; i++) {
      history = recordNavigation(history, skillPage(`s${i}`), skillPage(`s${i + 1}`));
    }
    expect(history.back).toHaveLength(NAV_HISTORY_LIMIT);
    expect(history.back[0]).toMatchObject({ name: "s10" });
  });
});

describe("stepBack and stepForward", () => {
  it("Back lands on the previous place and Forward returns to where Back left", () => {
    const history = recordNavigation(EMPTY_NAV_HISTORY, home, skills);
    const back = stepBack(history, skills, always)!;
    expect(back.view).toEqual(home);

    const forward = stepForward(back.history, home, always)!;
    expect(forward.view).toEqual(skills);
    expect(forward.history.back).toEqual([home]);
    expect(forward.history.forward).toEqual([]);
  });

  it("returns null with nothing to go to, so the caller leaves the view alone", () => {
    expect(stepBack(EMPTY_NAV_HISTORY, home, always)).toBeNull();
    expect(stepForward(EMPTY_NAV_HISTORY, home, always)).toBeNull();
  });

  it("skips a skill that no longer exists and lands on the next valid entry", () => {
    let history = recordNavigation(EMPTY_NAV_HISTORY, settings, skillPage("gone"));
    history = recordNavigation(history, skillPage("gone"), skills);
    const exists = (view: ActiveView) => view.kind !== "skill" || view.name !== "gone";

    const back = stepBack(history, skills, exists)!;
    expect(back.view).toEqual(settings);
    expect(back.history.back).toEqual([]);
  });

  it("returns null when every earlier entry was removed, not a stale page", () => {
    const history = recordNavigation(EMPTY_NAV_HISTORY, skillPage("gone"), home);
    expect(stepBack(history, home, (view) => view.kind !== "skill")).toBeNull();
  });
});
