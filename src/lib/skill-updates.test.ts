// ============================================================================
// Skill Studio - skill-updates tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { skillsWithUpdates } from "./skill-updates";
import type { InstalledSkill, SkillSnapshot } from "./skill-types";

function fixtureSkill(overrides: Partial<InstalledSkill> = {}): InstalledSkill {
  return {
    name: "agent-browser",
    source: "getsentry/agent-browser",
    source_type: "github",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "dotagents",
    deployments: [],
    has_spec: false,
    spec_violations: [],
    skill_md_tokens: 0,
    folder_bytes: 0,
    file_count: 0,
    content_hash: "",
    content_hashes: [],
    frontmatter_fields: {},
    folder_truncated: false,
    parked: false,
    invocation: "both",
    ...overrides,
  };
}

function fixtureSnapshot(skills: InstalledSkill[]): SkillSnapshot {
  return {
    skills,
    projects: [],
    invocations: [],
    heatmap: { days: {} },
    scanned_at: "2026-01-01T00:00:00Z",
    last_test_by_skill: {},
    update_check: { checked_at: null, gh_status: "ok", message: null, updates_available: 0 },
  };
}

describe("skillsWithUpdates", () => {
  it("returns only skills with has_update set", () => {
    const skills = [fixtureSkill({ has_update: true }), fixtureSkill({ name: "other" })];
    expect(skillsWithUpdates(fixtureSnapshot(skills)).map((s) => s.name)).toEqual([
      "agent-browser",
    ]);
  });

  it("returns nothing when no skill has an update", () => {
    expect(skillsWithUpdates(fixtureSnapshot([fixtureSkill()]))).toEqual([]);
  });
});
