// ============================================================================
// Skill Studio - skill-row-state tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { fixesFor, rowGroup, rowState } from "./skill-row-state";

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

  it("keeps a notes-only skill Healthy, or a harmless length note would crowd Needs attention", () => {
    const note = "description exceeds 1024 characters";
    const skill = fixtureSkill({
      spec_violations: [note],
      deployments: [fixtureDeployment({ spec_violations: [note] })],
    });
    expect(rowState(skill)).toBeNull();
    expect(rowGroup(skill)).toBe("healthy");
  });

  it("puts a name-mismatch skill in Needs attention as a warning, so agents' different names get checked", () => {
    const mismatch = 'name "other" does not match its directory name "find-bugs"';
    const skill = fixtureSkill({
      spec_violations: [mismatch],
      deployments: [fixtureDeployment({ spec_violations: [mismatch] })],
    });
    expect(rowState(skill)?.level).toBe("warning");
    expect(rowGroup(skill)).toBe("attention");
  });
});
