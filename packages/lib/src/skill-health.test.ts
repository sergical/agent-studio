// ============================================================================
// Skill Studio - skill-health tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { coverageGaps, HEALTH_ISSUE_KIND_ORDER, isBlockingSpecViolation } from "./skill-health";
import type { Deployment, InstalledSkill } from "./skill-types";

/** Minimal `Deployment` fixture, overridable per test. */
function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    id: "fixture-deployment",
    destination: "per-harness",
    owner_kind: "dotagents",
    mutability: "mutable",
    backing: { kind: "canonical" },
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
    description_tokens: 0,
    folder_bytes: 0,
    file_count: 0,
    content_hash: "",
    content_hashes: [],
    frontmatter_fields: {},
    folder_truncated: false,
    parked: false,
    invocation: "both",
    ...overrides,
    update_owner_ids: overrides.update_owner_ids ?? [],
  };
}

describe("isBlockingSpecViolation", () => {
  it("treats malformed YAML as blocking so the detail page can offer repair", () => {
    expect(
      isBlockingSpecViolation(
        "invalid YAML frontmatter at line 3, column 22: mapping values are not allowed in this context",
      ),
    ).toBe(true);
  });

  it("treats a missing required field as blocking", () => {
    expect(isBlockingSpecViolation("missing required frontmatter field: name")).toBe(true);
    expect(isBlockingSpecViolation("missing required frontmatter field: description")).toBe(true);
  });

  it("treats an invalid name format as blocking", () => {
    expect(
      isBlockingSpecViolation(
        'name "Bad Name" must be 1-64 lowercase a-z0-9 characters and hyphens, with no leading, trailing, or consecutive hyphens',
      ),
    ).toBe(true);
  });

  it("treats a name/directory mismatch as blocking", () => {
    expect(
      isBlockingSpecViolation(
        'name "other-name" does not match its directory name "agent-browser"',
      ),
    ).toBe(true);
  });

  it("treats length and style notes as non-blocking", () => {
    expect(isBlockingSpecViolation("description exceeds 1024 characters")).toBe(false);
    expect(isBlockingSpecViolation("compatibility exceeds 500 characters")).toBe(false);
    expect(isBlockingSpecViolation("SKILL.md exceeds recommended 500 lines")).toBe(false);
    expect(isBlockingSpecViolation("conflicting invocation keys")).toBe(false);
  });
});

describe("coverageGaps", () => {
  it("flags a skill deployed to some, but not all, first-class agents at the same scope", () => {
    const skill = fixtureSkill({
      deployments: [fixtureDeployment({ agent: "Claude Code", scope: "global" })],
    });
    const gaps = coverageGaps([skill]);
    expect(gaps).toHaveLength(1);
    expect(gaps[0].scopeLabel).toBe("Global");
    expect(gaps[0].missing).not.toContain("Claude Code");
  });

  it("does not flag a parked skill", () => {
    const skill = fixtureSkill({
      parked: true,
      deployments: [fixtureDeployment({ agent: "Claude Code", scope: "global" })],
    });
    expect(coverageGaps([skill])).toEqual([]);
  });
});

describe("HEALTH_ISSUE_KIND_ORDER", () => {
  it("includes parked-but-reinstalled", () => {
    expect(HEALTH_ISSUE_KIND_ORDER).toContain("parked-but-reinstalled");
  });

  it("does not include update-available or missing-from-agents", () => {
    expect(HEALTH_ISSUE_KIND_ORDER).not.toContain("update-available");
    expect(HEALTH_ISSUE_KIND_ORDER).not.toContain("missing-from-agents");
  });
});
