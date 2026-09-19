// ============================================================================
// Skill Studio - Add Skill form domain tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { parseSkillSource } from "@skill-studio/lib";
import type { AddMethodDefaults } from "@skill-studio/lib";
import { availableAddSkillMethods, isAddSkillFormValid } from "./add-skill-form";

function methodDefaults(dotagentsInstalled: boolean): AddMethodDefaults {
  return {
    dotagents_installed: dotagentsInstalled,
    has_skill_lock: false,
    installed_harnesses: [],
    claude_reads_shared_folder: false,
  };
}

describe("Add Skill form validation", () => {
  it("rejects a parsed git source when dotagents is unavailable", () => {
    const parsed = parseSkillSource("git:https://example.com/skills.git");
    const methods = availableAddSkillMethods(parsed, methodDefaults(false));

    expect(methods).toEqual([]);
    expect(
      isAddSkillFormValid({
        parsed,
        noMethodsAvailable: methods.length === 0,
        destination: "universal",
        agents: [],
        scope: "global",
        projectPath: null,
        githubEntries: null,
      }),
    ).toBe(false);
  });

  it("never offers pack for a GitHub source while the pack commands are unregistered in lib.rs, catching a regression that reintroduces it", () => {
    const githubSource = parseSkillSource("https://github.com/owner/repo");
    const withDotagents = availableAddSkillMethods(githubSource, methodDefaults(true));
    const withoutDotagents = availableAddSkillMethods(githubSource, methodDefaults(false));

    expect(withDotagents).not.toContain("pack");
    expect(withoutDotagents).not.toContain("pack");
  });

  it("accepts normal sources with a valid install method", () => {
    const gitSource = parseSkillSource("git:https://example.com/skills.git");
    const localSource = parseSkillSource("~/skills/find-bugs");
    const gitMethods = availableAddSkillMethods(gitSource, methodDefaults(true));
    const localMethods = availableAddSkillMethods(localSource, methodDefaults(false));

    expect(gitMethods).toEqual(["dotagents"]);
    expect(localMethods).toEqual(["copy"]);
    expect(
      isAddSkillFormValid({
        parsed: gitSource,
        noMethodsAvailable: gitMethods.length === 0,
        destination: "universal",
        agents: [],
        scope: "global",
        projectPath: null,
        githubEntries: null,
      }),
    ).toBe(true);
  });
});
