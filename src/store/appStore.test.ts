// ============================================================================
// Skill Studio - appStore tests
// Multi-select actions used by SkillListTable's "Create pack" flow.
// ============================================================================

import { beforeEach, describe, expect, it } from "vitest";
import { useAppStore } from "./appStore";

beforeEach(() => {
  useAppStore.setState({ selectedSkillPaths: new Set(), activeView: { kind: "home" } });
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
    useAppStore.getState().setActiveView({ kind: "skills" });
    expect(useAppStore.getState().selectedSkillPaths).toEqual(new Set());
  });
});

describe("skillListFilter", () => {
  beforeEach(() => {
    useAppStore.setState({
      skillListFilter: { scope: "all", query: "" },
      activeView: { kind: "home" },
    });
  });

  it("typing from Home sets the query and switches to Skills, like the sidebar search box", () => {
    useAppStore.getState().setSkillListFilter({ query: "ab" });
    useAppStore.getState().setActiveView({ kind: "skills" });
    expect(useAppStore.getState().skillListFilter.query).toBe("ab");
    expect(useAppStore.getState().activeView.kind).toBe("skills");
  });

  it("opening and closing a skill leaves the filter unchanged", () => {
    useAppStore.getState().setActiveView({ kind: "skills" });
    useAppStore.getState().setSkillListFilter({ scope: "global" });
    useAppStore.getState().openSkill("find-bugs");
    useAppStore.getState().closeSkill();
    expect(useAppStore.getState().skillListFilter.scope).toBe("global");
  });
});
