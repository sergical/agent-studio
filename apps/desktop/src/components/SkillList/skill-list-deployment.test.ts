// ============================================================================
// skill-list-deployment.test - the copy a list row opens on per scope filter.
// ============================================================================

import { describe, expect, it } from "vitest";
import { ownSkillsView } from "@skill-studio/lib";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { deploymentForScope } from "./skill-list-deployment";

const ERROR = 'name "Find Bugs" is not a valid skill name';

function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  // SAFETY: test fixture; only the fields the code under test reads are filled in.
  return {
    agent: "shared",
    scope: "global",
    path: "/home/.agents/skills/find-bugs",
    is_symlink: false,
    symlink_is_broken: false,
    spec_violations: [],
    ...overrides,
  } as Deployment;
}

function fixtureSkill(deployments: Deployment[]): InstalledSkill {
  // SAFETY: test fixture; only the fields the code under test reads are filled in.
  return {
    name: "find-bugs",
    deployments,
    spec_violations: deployments.flatMap((d) => d.spec_violations),
  } as InstalledSkill;
}

describe("deploymentForScope", () => {
  it("the_all_view_opens_the_warned_copy_instead_of_the_clean_first_copy_or_the_row_badge_and_page_disagree", () => {
    const skill = fixtureSkill([
      fixtureDeployment({ path: "/a/clean" }),
      fixtureDeployment({ path: "/b/warned", spec_violations: [ERROR] }),
    ]);
    expect(deploymentForScope(skill, "all")).toBe("/b/warned");
  });

  it("the_all_view_opens_the_warned_own_copy_of_a_skill_a_plugin_also_ships_or_it_returns_nothing_and_the_page_picks_the_plugin_copy", () => {
    const skill = fixtureSkill([
      fixtureDeployment({ path: "/own/clean" }),
      fixtureDeployment({ path: "/own/warned", spec_violations: [ERROR] }),
      fixtureDeployment({
        path: "/plugin/errored",
        spec_violations: [ERROR],
        plugin: { name: "p", version: null, harness: "Claude Code", marketplace: "m", id: "p@m" },
      }),
    ]);
    const [own] = ownSkillsView([skill]);
    expect(deploymentForScope(own, "all")).toBe("/own/warned");
  });

  it("a_project_scope_opens_that_projects_copy_even_when_another_scope_has_the_warning", () => {
    const skill = fixtureSkill([
      fixtureDeployment({ path: "/p/clean", scope: "project", project_path: "/p" }),
      fixtureDeployment({ path: "/g/warned", spec_violations: [ERROR] }),
    ]);
    expect(deploymentForScope(skill, { project: "/p" })).toBe("/p/clean");
  });

  it("the_global_scope_opens_the_global_copy_not_a_warned_project_copy", () => {
    const skill = fixtureSkill([
      fixtureDeployment({
        path: "/p/warned",
        scope: "project",
        project_path: "/p",
        spec_violations: [ERROR],
      }),
      fixtureDeployment({ path: "/g/clean" }),
    ]);
    expect(deploymentForScope(skill, "global")).toBe("/g/clean");
  });
});
