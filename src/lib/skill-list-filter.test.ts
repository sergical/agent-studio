// ============================================================================
// Skill Studio - skill-list-filter tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { applySkillListFilter, defaultSkillListFilter } from "./skill-list-filter";
import type { Deployment, InstalledSkill } from "./skill-types";

function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    agent: "Claude Code",
    scope: "global",
    path: "/home/.claude/skills/find-bugs",
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
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

describe("applySkillListFilter", () => {
  it("defaults to every non-parked skill with an empty query", () => {
    const skills = [fixtureSkill(), fixtureSkill({ name: "parked-one", parked: true })];
    expect(applySkillListFilter(skills, defaultSkillListFilter())).toEqual([skills[0]]);
  });

  it("parked scope shows only parked skills", () => {
    const skills = [fixtureSkill(), fixtureSkill({ name: "parked-one", parked: true })];
    const result = applySkillListFilter(skills, { scope: "parked", query: "" });
    expect(result.map((s) => s.name)).toEqual(["parked-one"]);
  });

  it("global scope includes global and plugin deployments, not project ones", () => {
    const globalSkill = fixtureSkill({ name: "global-skill" });
    const projectSkill = fixtureSkill({
      name: "project-skill",
      deployments: [fixtureDeployment({ scope: "project", project_path: "/repo" })],
    });
    const result = applySkillListFilter([globalSkill, projectSkill], {
      scope: "global",
      query: "",
    });
    expect(result.map((s) => s.name)).toEqual(["global-skill"]);
  });

  it("project scope matches only deployments with that project_path", () => {
    const skillA = fixtureSkill({
      name: "a",
      deployments: [fixtureDeployment({ scope: "project", project_path: "/repo-a" })],
    });
    const skillB = fixtureSkill({
      name: "b",
      deployments: [fixtureDeployment({ scope: "project", project_path: "/repo-b" })],
    });
    const result = applySkillListFilter([skillA, skillB], {
      scope: { project: "/repo-a" },
      query: "",
    });
    expect(result.map((s) => s.name)).toEqual(["a"]);
  });

  it("filters by harness display label", () => {
    const skills = [
      fixtureSkill({ name: "claude-only" }),
      fixtureSkill({ name: "codex-only", deployments: [fixtureDeployment({ agent: "Codex" })] }),
    ];
    const result = applySkillListFilter(skills, {
      scope: "all",
      harness: "Codex",
      query: "",
    });
    expect(result.map((s) => s.name)).toEqual(["codex-only"]);
  });

  it("filters by source kind", () => {
    const skills = [
      fixtureSkill({ name: "a", source_kind: "dotagents" }),
      fixtureSkill({ name: "b", source_kind: "manual" }),
    ];
    const result = applySkillListFilter(skills, { scope: "all", source: "manual", query: "" });
    expect(result.map((s) => s.name)).toEqual(["b"]);
  });

  it("filters by issue kind", () => {
    const skills = [
      fixtureSkill({
        name: "broken",
        deployments: [fixtureDeployment({ symlink_is_broken: true })],
      }),
      fixtureSkill({ name: "healthy" }),
    ];
    const result = applySkillListFilter(skills, {
      scope: "all",
      issue: "broken-symlink",
      query: "",
    });
    expect(result.map((s) => s.name)).toEqual(["broken"]);
  });

  it("plugin source filters by deployment, not the aggregate source_kind", () => {
    const skill = fixtureSkill({
      name: "mixed",
      source_kind: "dotagents",
      deployments: [
        fixtureDeployment(),
        fixtureDeployment({ scope: "plugin", plugin: { harness: "Claude Code", name: "acme" } }),
      ],
    });
    const pluginResult = applySkillListFilter([skill], {
      scope: "all",
      source: "plugin",
      query: "",
    });
    expect(pluginResult.map((s) => s.name)).toEqual(["mixed"]);

    const dotagentsResult = applySkillListFilter([skill], {
      scope: "all",
      source: "dotagents",
      query: "",
    });
    expect(dotagentsResult.map((s) => s.name)).toEqual(["mixed"]);
  });

  it('issue "any" keeps skills with at least one issue, of any kind', () => {
    const skills = [
      fixtureSkill({
        name: "broken",
        deployments: [fixtureDeployment({ symlink_is_broken: true })],
      }),
      fixtureSkill({ name: "healthy" }),
    ];
    const result = applySkillListFilter(skills, { scope: "all", issue: "any", query: "" });
    expect(result.map((s) => s.name)).toEqual(["broken"]);
  });

  it("query matches name, description, and source, case-insensitively", () => {
    const skills = [
      fixtureSkill({ name: "find-bugs", description: "Locates regressions" }),
      fixtureSkill({ name: "other", description: "Unrelated", source: "acme/other" }),
    ];
    expect(applySkillListFilter(skills, { scope: "all", query: "REGRESSIONS" })).toEqual([
      skills[0],
    ]);
    expect(applySkillListFilter(skills, { scope: "all", query: "acme" })).toEqual([skills[1]]);
  });
});
