// ============================================================================
// Skill Studio - skill-health tests
// ============================================================================

import { describe, expect, it } from "vitest";
import {
  findParkedButReinstalled,
  findUpdateAvailable,
  HEALTH_ISSUE_KIND_ORDER,
} from "./skill-health";
import type { Deployment, InstalledSkill } from "./skill-types";

/** Minimal `Deployment` fixture, overridable per test. */
function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    agent: "shared",
    scope: "global",
    path: "/home/.agents/skills/agent-browser",
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
    ...overrides,
  };
}

/** Minimal `InstalledSkill` fixture, overridable per test. */
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

describe("findUpdateAvailable", () => {
  it("flags skills with has_update set", () => {
    const skills = [fixtureSkill({ has_update: true }), fixtureSkill({ name: "other" })];
    const issues = findUpdateAvailable(skills);
    expect(issues).toHaveLength(1);
    expect(issues[0].kind).toBe("update-available");
    expect(issues[0].skill.name).toBe("agent-browser");
  });

  it("returns nothing when no skill has an update", () => {
    expect(findUpdateAvailable([fixtureSkill()])).toEqual([]);
  });
});

describe("HEALTH_ISSUE_KIND_ORDER", () => {
  it("includes update-available", () => {
    expect(HEALTH_ISSUE_KIND_ORDER).toContain("update-available");
  });

  it("includes parked-but-reinstalled", () => {
    expect(HEALTH_ISSUE_KIND_ORDER).toContain("parked-but-reinstalled");
  });
});

describe("findParkedButReinstalled", () => {
  it("flags a parked skill whose shared-folder deployment came back", () => {
    const skill = fixtureSkill({
      parked: true,
      deployments: [fixtureDeployment({ scope: "global" })],
    });
    const issues = findParkedButReinstalled([skill]);
    expect(issues).toHaveLength(1);
    expect(issues[0].kind).toBe("parked-but-reinstalled");
  });

  it("does not flag a parked skill with only its parked-copy deployment", () => {
    const skill = fixtureSkill({
      parked: true,
      deployments: [fixtureDeployment({ scope: "parked" })],
    });
    expect(findParkedButReinstalled([skill])).toEqual([]);
  });

  it("does not flag a skill that isn't parked", () => {
    const skill = fixtureSkill({
      parked: false,
      deployments: [fixtureDeployment({ scope: "global" })],
    });
    expect(findParkedButReinstalled([skill])).toEqual([]);
  });
});
