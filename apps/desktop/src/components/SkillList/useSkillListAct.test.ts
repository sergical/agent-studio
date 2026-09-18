// ============================================================================
// Skill Studio - useSkillListAct's reportFixOutcome tests
// ============================================================================

import { describe, expect, it, vi } from "vitest";
import type { InstalledSkill } from "@skill-studio/lib";

import { reportFixOutcome, type FixSkillDeps } from "./useSkillListAct";

function fixtureSkill(overrides: Partial<InstalledSkill> = {}): InstalledSkill {
  return {
    name: "find-bugs",
    source: "getsentry/find-bugs",
    source_type: "github",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "dotagents",
    deployments: [],
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

describe("reportFixOutcome", () => {
  it("calls the fixSkill wrapper for a skill whose only issue is invalid YAML, and toasts success once applied", async () => {
    const skill = fixtureSkill({
      spec_violations: ["invalid YAML frontmatter at line 3, column 1: mapping values not allowed"],
    });
    const fixSkill = vi.fn().mockResolvedValue({
      skill: skill.name,
      applied: [{ kind: "frontmatter_repair", deployment_id: "dep:1", event_id: "evt:1" }],
      unrepaired: [],
      conflicts: [],
    });
    const openConflictPaths = vi.fn();
    const addToast = vi.fn();
    const openDetail = vi.fn();
    const deps: FixSkillDeps = { fixSkill, openConflictPaths };

    await reportFixOutcome(skill, addToast, openDetail, deps);

    expect(fixSkill).toHaveBeenCalledWith("find-bugs");
    expect(addToast).toHaveBeenCalledWith(
      expect.objectContaining({ type: "success", title: "Fixed find-bugs" }),
    );
    expect(openDetail).not.toHaveBeenCalled();
  });

  it("falls through to the detail page's own repair card when the only unrepaired issue is a dangling link", async () => {
    const skill = fixtureSkill();
    const fixSkill = vi.fn().mockResolvedValue({
      skill: skill.name,
      applied: [],
      unrepaired: [
        {
          path: "/home/.claude/skills/find-bugs",
          message: "/home/.claude/skills/find-bugs links to a missing target",
        },
      ],
      conflicts: [],
    });
    const openConflictPaths = vi.fn();
    const addToast = vi.fn();
    const openDetail = vi.fn();
    const deps: FixSkillDeps = { fixSkill, openConflictPaths };

    await reportFixOutcome(skill, addToast, openDetail, deps);

    expect(openDetail).toHaveBeenCalledOnce();
    expect(addToast).not.toHaveBeenCalled();
  });
});
