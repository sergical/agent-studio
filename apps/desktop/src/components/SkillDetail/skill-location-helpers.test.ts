// ============================================================================
// Skill Studio - skill location helper tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { sharedFolderSwitchPolicy } from "./skill-location-helpers";
import { buildScopeGroups, rowMenu } from "./skill-location-status";

function sharedDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    id: "dep:v1/global/universal/find-bugs",
    destination: "universal",
    owner_kind: "manual",
    mutability: "read-only",
    backing: { kind: "canonical" },
    agent: "shared",
    scope: "global",
    path: "/home/.agents/skills/find-bugs",
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
    codex_implicit_invocation: null,
    disabled_by: null,
    invocation: "both",
    spec_violations: [],
    shared_via_whole_dir_link: false,
    ...overrides,
  };
}

function skillWithDeployments(deployments: Deployment[]): InstalledSkill {
  return {
    name: "find-bugs",
    source: "local",
    source_type: "local",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "manual",
    deployments,
    has_spec: true,
    spec_violations: [],
    skill_md_tokens: 0,
    description_tokens: 0,
    folder_bytes: 0,
    file_count: 0,
    content_hash: "abc",
    content_hashes: ["abc"],
    frontmatter_fields: {},
    folder_truncated: false,
    parked: false,
    invocation: "both",
    update_owner_ids: [],
    update_owners: [],
    description: null,
    fork: null,
    parked_at: null,
    skill_path: null,
    source_url: null,
    trial: null,
    trials: [],
    update_commit: null,
    update_commit_at: null,
    updated_at: null,
  };
}

describe("sharedFolderSwitchPolicy", () => {
  it("keeps same-name Global and Project Universal folders isolated", () => {
    const skill = skillWithDeployments([
      sharedDeployment(),
      sharedDeployment({
        id: "dep:v1/project/universal/find-bugs",
        scope: "project",
        project_path: "/repo",
        path: "/repo/.agents/skills/find-bugs",
        disabled: true,
      }),
    ]);
    const groups = buildScopeGroups(skill);
    const global = groups.find((group) => group.isGlobal)!;
    const project = groups.find((group) => !group.isGlobal)!;
    const globalPolicy = sharedFolderSwitchPolicy(global);
    const projectPolicy = sharedFolderSwitchPolicy(project);

    expect(globalPolicy).toMatchObject({ checked: true, disabled: false });
    expect(globalPolicy.actionForCheckedChange(false)).toEqual({ kind: "park" });
    expect(globalPolicy.actionForCheckedChange(true)).toEqual({ kind: "unpark" });

    expect(projectPolicy).toMatchObject({ checked: false, disabled: true });
    expect(projectPolicy.actionForCheckedChange(false)).toBeNull();
    expect(projectPolicy.actionForCheckedChange(true)).toBeNull();
    expect(
      rowMenu(project.shared!, project.label, project.projectPath ?? null).entries.map(
        (entry) => entry.action.kind,
      ),
    ).not.toContain("park");
  });
});
