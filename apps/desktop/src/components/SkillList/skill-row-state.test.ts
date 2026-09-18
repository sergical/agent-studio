// ============================================================================
// Skill Studio - skill-row-state tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { fixesFor, rowState } from "./skill-row-state";

function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
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

function fixtureSkill(overrides: Partial<InstalledSkill> = {}): InstalledSkill {
  return {
    name: "find-bugs",
    source: "getsentry/find-bugs",
    source_type: "github",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "dotagents",
    deployments: [fixtureDeployment()],
    has_spec: true,
    spec_violations: [],
    skill_md_tokens: 0,
    description_tokens: 0,
    folder_bytes: 0,
    file_count: 0,
    content_hash: "",
    content_hashes: [],
    frontmatter_fields: {},
    folder_truncated: false,
    parked: false,
    invocation: "both",
    update_owners: [],
    update_owner_ids: [],
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
    ...overrides,
  };
}

describe("rowState", () => {
  it("shows the Fix action for a skill whose only issue is invalid YAML frontmatter", () => {
    const skill = fixtureSkill({
      spec_violations: ["invalid YAML frontmatter at line 3, column 1: mapping values not allowed"],
    });
    const state = rowState(skill);
    expect(state?.kind).toBe("violation");
    expect(state?.level).toBe("error");
    expect(state?.action).toBe("Fix");
    expect(fixesFor(state!)).toEqual(["Fix"]);
  });

  it("does not surface a Fix action for a non-blocking spec note", () => {
    const skill = fixtureSkill({ spec_violations: ["description exceeds 1024 characters"] });
    expect(rowState(skill)).toBeNull();
  });
});
