// ============================================================================
// Skill Studio - skill-plugin-partition tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { ownSkillsView } from "./skill-plugin-partition";
import type { Deployment, InstalledSkill } from "./skill-types";

const WARNING = "description exceeds 1024 characters";
const ERROR = 'name "Find Bugs" is not a valid skill name';

function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  // SAFETY: test fixture; only the fields the code under test reads are filled in.
  return {
    agent: "shared",
    scope: "global",
    path: "/home/.agents/skills/find-bugs",
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
    spec_violations: [],
    ...overrides,
  } as Deployment;
}

function fixtureSkill(deployments: Deployment[], specViolations: string[]): InstalledSkill {
  // SAFETY: test fixture; only the fields the code under test reads are filled in.
  return { name: "find-bugs", deployments, spec_violations: specViolations } as InstalledSkill;
}

describe("ownSkillsView", () => {
  it("a_plugin_copys_violations_leave_the_row_badge_of_a_skill_whose_own_copy_is_clean_or_the_badge_names_a_problem_the_page_cannot_show", () => {
    const skill = fixtureSkill(
      [
        fixtureDeployment({ path: "/own" }),
        fixtureDeployment({
          path: "/plugin",
          spec_violations: [ERROR],
          plugin: { name: "p", version: null, harness: "Claude Code", marketplace: "m", id: "p@m" },
        }),
      ],
      [ERROR],
    );
    expect(ownSkillsView([skill])[0].spec_violations).toEqual([]);
  });

  it("own_copies_with_the_same_violation_list_it_once_or_the_count_doubles", () => {
    const skill = fixtureSkill(
      [
        fixtureDeployment({ path: "/a", spec_violations: [WARNING] }),
        fixtureDeployment({ path: "/b", spec_violations: [WARNING, ERROR] }),
      ],
      [WARNING],
    );
    expect(ownSkillsView([skill])[0].spec_violations).toEqual([WARNING, ERROR]);
  });
});
