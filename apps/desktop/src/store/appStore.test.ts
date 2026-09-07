// ============================================================================
// Skill Studio - appStore tests
// Multi-select actions used by SkillListTable's "Create pack" flow.
// ============================================================================

import { beforeEach, describe, expect, it } from "vitest";
import { useAppStore } from "./appStore";

beforeEach(() => {
  useAppStore.setState({
    selectedSkillPaths: new Set(),
    selectionMode: false,
    activeView: { kind: "home" },
    skillListFilter: { scope: "all", query: "" },
  });
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

describe("selection mode", () => {
  it("enterSelectionMode turns the table's checkboxes on", () => {
    useAppStore.getState().enterSelectionMode();
    expect(useAppStore.getState().selectionMode).toBe(true);
  });

  it("toggling two paths while in selection mode selects both", () => {
    useAppStore.getState().enterSelectionMode();
    useAppStore.getState().toggleSkillSelection("/a");
    useAppStore.getState().toggleSkillSelection("/b");
    expect(useAppStore.getState().selectedSkillPaths).toEqual(new Set(["/a", "/b"]));
  });

  it("exitSelectionMode clears both the mode and the selection", () => {
    useAppStore.getState().enterSelectionMode();
    useAppStore.getState().toggleSkillSelection("/a");
    useAppStore.getState().exitSelectionMode();
    expect(useAppStore.getState().selectionMode).toBe(false);
    expect(useAppStore.getState().selectedSkillPaths).toEqual(new Set());
  });

  it("leaving the list view ends selection mode", () => {
    useAppStore.getState().enterSelectionMode();
    useAppStore.getState().toggleSkillSelection("/a");
    useAppStore.getState().setActiveView({ kind: "home" });
    expect(useAppStore.getState().selectionMode).toBe(false);
    expect(useAppStore.getState().selectedSkillPaths).toEqual(new Set());
  });

  it("opening a skill ends selection mode", () => {
    useAppStore.getState().enterSelectionMode();
    useAppStore.getState().toggleSkillSelection("/a");
    useAppStore.getState().openSkill("find-bugs");
    expect(useAppStore.getState().selectionMode).toBe(false);
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

  it("keeps sidebar and filter-bar query updates on the same store value", () => {
    const updateQuery = (query: string) => useAppStore.getState().setSkillListFilter({ query });

    updateQuery("from sidebar");
    expect(useAppStore.getState().skillListFilter.query).toBe("from sidebar");

    updateQuery("from filter bar");
    expect(useAppStore.getState().skillListFilter.query).toBe("from filter bar");
  });

  it("opening and closing a skill leaves the filter unchanged", () => {
    useAppStore.getState().setActiveView({ kind: "skills" });
    useAppStore.getState().setSkillListFilter({ scope: "global", query: "regression" });
    useAppStore.getState().openSkill("find-bugs");
    useAppStore.getState().closeSkill();
    expect(useAppStore.getState().skillListFilter.scope).toBe("global");
    expect(useAppStore.getState().skillListFilter.query).toBe("regression");
  });

  it("changing scope clears selection mode and paths together", () => {
    useAppStore.setState({
      skillListFilter: { scope: "global", query: "find" },
      selectionMode: true,
      selectedSkillPaths: new Set(["/global/find-bugs"]),
    });

    useAppStore.getState().setSkillListFilter({ scope: { project: "/repo" } });

    const state = useAppStore.getState();
    expect(state.skillListFilter).toEqual({ scope: { project: "/repo" }, query: "find" });
    expect(state.selectionMode).toBe(false);
    expect(state.selectedSkillPaths).toEqual(new Set());
  });

  it("keeps selection for query and other unrelated filter changes", () => {
    useAppStore.setState({
      skillListFilter: { scope: "global", query: "" },
      selectionMode: true,
      selectedSkillPaths: new Set(["/global/find-bugs"]),
    });

    useAppStore.getState().setSkillListFilter({ query: "find", source: "manual" });

    const state = useAppStore.getState();
    expect(state.skillListFilter).toEqual({ scope: "global", query: "find", source: "manual" });
    expect(state.selectionMode).toBe(true);
    expect(state.selectedSkillPaths).toEqual(new Set(["/global/find-bugs"]));
  });

  it("resetting a scoped filter also clears its stale selection", () => {
    useAppStore.setState({
      skillListFilter: { scope: "parked", query: "" },
      selectionMode: true,
      selectedSkillPaths: new Set(["/parked/find-bugs"]),
    });

    useAppStore.getState().resetSkillListFilter();

    expect(useAppStore.getState().skillListFilter).toEqual({ scope: "all", query: "" });
    expect(useAppStore.getState().selectionMode).toBe(false);
    expect(useAppStore.getState().selectedSkillPaths).toEqual(new Set());
  });
});

describe("compare intent", () => {
  it("openSkill with a compare intent stores it on the skill view", () => {
    useAppStore.getState().openSkill("find-bugs", undefined, "compare");
    const view = useAppStore.getState().activeView;
    expect(view.kind).toBe("skill");
    expect(view.kind === "skill" && view.intent).toBe("compare");
  });

  it("clearSkillIntent removes the intent without changing the rest of the view", () => {
    useAppStore.getState().openSkill("find-bugs", "/home/.agents/skills/find-bugs", "compare");
    useAppStore.getState().clearSkillIntent();
    const view = useAppStore.getState().activeView;
    expect(view.kind).toBe("skill");
    expect(view.kind === "skill" && view.intent).toBeUndefined();
    expect(view.kind === "skill" && view.deploymentPath).toBe("/home/.agents/skills/find-bugs");
  });

  it("clearSkillIntent is a no-op off the skill view", () => {
    useAppStore.getState().setActiveView({ kind: "home" });
    useAppStore.getState().clearSkillIntent();
    expect(useAppStore.getState().activeView).toEqual({ kind: "home" });
  });
});
