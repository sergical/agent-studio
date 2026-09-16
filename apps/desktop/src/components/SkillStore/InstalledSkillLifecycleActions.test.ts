import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { InstalledSkillLifecycleActions } from "./InstalledSkillLifecycleActions";
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
    {
      owner_id: globalOwner,
      latest_commit: "global-next",
      comparison: { kind: "different" },
      actionable: true,
    },
    { owner_id: firstProjectOwner, comparison: { kind: "equal" }, actionable: false },
    {
      owner_id: secondProjectOwner,
      latest_commit: "project-next",
      comparison: { kind: "different" },
      actionable: true,
    },
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

it("renders the disabled update and its Unknown reason in the store drawer", () => {
  const installed: InstalledSkill = {
    ...skill,
    deployments: [deployment("global", globalOwner)],
    source: "acme/x",
    source_type: "github",
    source_kind: "skills-sh",
    installed_at: "",
    has_update: false,
    update_owner_ids: [],
    update_owners: [
      {
        owner_id: globalOwner,
        actionable: false,
        comparison: { kind: "unknown-with-reason", reason: "GitHub lookup failed" },
      },
    ],
    has_spec: false,
    spec_violations: [],
    skill_md_tokens: 0,
    description_tokens: 0,
    folder_bytes: 0,
    file_count: 1,
    content_hash: "x",
    content_hashes: ["x"],
    frontmatter_fields: {},
    folder_truncated: false,
    parked: false,
    invocation: "both",
  };
  const markup = renderToStaticMarkup(
    createElement(InstalledSkillLifecycleActions, {
      skill: {
        id: "acme/x/x",
        name: "x",
        top_source: "acme/x",
        installs: 1,
        is_installed: true,
        installed_info: installed,
      },
      onInstallComplete: () => {},
      onRemoveComplete: () => {},
    }),
  );
  expect(markup).toContain("Update Skill");
  expect(markup).toContain("GitHub lookup failed");
  expect(markup).toMatch(/<button[^>]*disabled[^>]*>[\s\S]*?Update Skill/);
});
