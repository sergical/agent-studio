// ============================================================================
// Skill Studio - skill-row-state tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, HealthIssue, InstalledSkill } from "@skill-studio/lib";
import { fixesFor, issueRowState, rowState } from "./skill-row-state";

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

  it("does not surface a Fix action for a non-blocking spec note", () => {
    const skill = fixtureSkill({ spec_violations: ["description exceeds 1024 characters"] });
    expect(rowState(skill)).toBeNull();
  });

  it("a_linked_root_warning_on_a_skill_with_an_update_shows_the_warning_glyph_or_names_the_update_that_outranked_it", () => {
    const skill = fixtureSkill({ update_owner_ids: ["x"] });
    const issue: HealthIssue = {
      kind: "linked-root",
      skill,
      detail: "Claude Code reads the Universal folder through a root link",
      harness: "claude-code",
      harnessLabel: "Claude Code",
      root: "/home/.agents/skills",
    };
    // `rowState` still ranks the update above the warning - it's the Skills list's own ladder,
    // not the rule the Home group uses; this documents why the group can't reuse it as-is.
    expect(rowState(skill)?.kind).toBe("update");
    expect(issueRowState(issue).kind).toBe("issue");
    expect(issueRowState(issue).level).toBe("warning");
  });

  it("a_lock_only_warning_with_no_row_state_still_shows_the_warning_glyph_or_names_the_missing_glyph", () => {
    const skill = fixtureSkill();
    const issue: HealthIssue = {
      kind: "lock-only",
      skill,
      detail: "Only recorded in the lock file",
    };
    expect(rowState(skill)).toBeNull();
    expect(issueRowState(issue).level).toBe("warning");
  });

  it("a_broken_symlink_issue_shows_the_error_glyph_or_names_the_wrong_level", () => {
    const skill = fixtureSkill();
    const issue: HealthIssue = {
      kind: "broken-symlink",
      skill,
      detail: "Symlink target no longer exists",
    };
    expect(issueRowState(issue).level).toBe("error");
  });
});
