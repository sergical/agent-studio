import { describe, expect, it } from "vitest";
import { skillUpdateAvailability } from "../../lib/skill-lifecycle-target";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";

function deployment(id: string, ownerId: string, projectPath?: string): Deployment {
  return {
    id,
    destination: "universal",
    owner_kind: "skills-sh",
    owner_id: ownerId,
    mutability: "mutable",
    backing: { kind: "canonical" },
    agent: "Universal",
    scope: projectPath ? "project" : "global",
    path: `${projectPath ?? "/home"}/.agents/skills/x`,
    is_symlink: false,
    symlink_is_broken: false,
    project_path: projectPath,
    content_hash: "x",
    disabled: false,
  };
}

const globalOwner = "owner:v1/global/x";
const firstProjectOwner = "owner:v1/project/%2Fwork%2Fone/x";
const secondProjectOwner = "owner:v1/project/%2Fwork%2Ftwo/x";
const skill = {
  name: "x",
  deployments: [
    deployment("global", globalOwner),
    deployment("project-one", firstProjectOwner, "/work/one"),
    deployment("project-two", secondProjectOwner, "/work/two"),
  ],
  update_owner_ids: [globalOwner, secondProjectOwner],
  update_owners: [
    { owner_id: globalOwner, latest_commit: "global-next" },
    { owner_id: secondProjectOwner, latest_commit: "project-next" },
  ],
} satisfies Pick<InstalledSkill, "name" | "deployments" | "update_owner_ids" | "update_owners">;

describe("InstalledSkillLifecycleActions update selection", () => {
  it("targets only the Global owner when Global is selected", () => {
    expect(
      skillUpdateAvailability(skill, { skillName: "x", scope: "global", projectPath: null }),
    ).toEqual({ available: true, target: { owner_id: globalOwner } });
  });

  it("disables Update for the first Project when only the second Project has an update", () => {
    expect(
      skillUpdateAvailability(skill, {
        skillName: "x",
        scope: "project",
        projectPath: "/work/one",
      }),
    ).toEqual({ available: false, reason: "The selected deployment is up to date." });
  });

  it("targets only the selected Project owner", () => {
    expect(
      skillUpdateAvailability(skill, {
        skillName: "x",
        scope: "project",
        projectPath: "/work/two",
      }),
    ).toEqual({ available: true, target: { owner_id: secondProjectOwner } });
  });

  it("disables Update when one selected scope has several owners", () => {
    const ambiguous = {
      ...skill,
      deployments: [
        deployment("global-one", globalOwner),
        deployment("global-two", firstProjectOwner),
      ],
    };
    const availability = skillUpdateAvailability(ambiguous, {
      skillName: "x",
      scope: "global",
      projectPath: null,
    });
    expect(availability.available).toBe(false);
    if (!availability.available) expect(availability.reason).toContain("specific deployment");
  });
});
