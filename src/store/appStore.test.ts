// ============================================================================
// Skill Studio - appStore tests
// Multi-select actions used by SkillListTable's "Create pack" flow.
// ============================================================================

import { beforeEach, describe, expect, it } from "vitest";
import { useAppStore } from "./appStore";

beforeEach(() => {
  useAppStore.setState({ selectedSkillPaths: new Set(), activeView: { kind: "dashboard" } });
});

describe("skill selection", () => {
  it("toggleSkillSelection adds an unselected path and removes a selected one", () => {
    useAppStore.getState().toggleSkillSelection("/home/u/.agents/skills/find-bugs");
    expect(useAppStore.getState().selectedSkillPaths).toEqual(
      new Set(["/home/u/.agents/skills/find-bugs"]),
    );

    useAppStore.getState().toggleSkillSelection("/home/u/.agents/skills/find-bugs");
    expect(useAppStore.getState().selectedSkillPaths).toEqual(new Set());
  });

  it("selectSkills replaces the selection with the given paths, for shift-click range-select", () => {
    useAppStore.getState().toggleSkillSelection("/a");
    useAppStore.getState().selectSkills(["/b", "/c", "/d"]);
    expect(useAppStore.getState().selectedSkillPaths).toEqual(new Set(["/b", "/c", "/d"]));
  });

  it("clearSkillSelection empties the selection", () => {
    useAppStore.getState().selectSkills(["/a", "/b"]);
    useAppStore.getState().clearSkillSelection();
    expect(useAppStore.getState().selectedSkillPaths).toEqual(new Set());
  });

  it("setActiveView clears the selection so it doesn't leak across views", () => {
    useAppStore.getState().selectSkills(["/a", "/b"]);
    useAppStore.getState().setActiveView({ kind: "global" });
    expect(useAppStore.getState().selectedSkillPaths).toEqual(new Set());
  });
});
