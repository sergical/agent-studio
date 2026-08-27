// ============================================================================
// Skill Studio - skill-health tests
// ============================================================================

import { describe, expect, it } from "vitest";
import {
  coverageGaps,
  findDuplicateSkills,
  findParkedButReinstalled,
  findSpecViolations,
  HEALTH_ISSUE_KIND_ORDER,
  isBlockingSpecViolation,
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

describe("isBlockingSpecViolation", () => {
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

describe("findSpecViolations", () => {
  it("flags a skill with a blocking violation", () => {
    const skill = fixtureSkill({
      spec_violations: ["missing required frontmatter field: description"],
    });
    const issues = findSpecViolations([skill]);
    expect(issues).toHaveLength(1);
    expect(issues[0].kind).toBe("spec-violation");
    expect(issues[0].detail).toBe("missing required frontmatter field: description");
  });

  it("does not flag a skill with only non-blocking violations", () => {
    const skill = fixtureSkill({
      spec_violations: ["description exceeds 1024 characters", "conflicting invocation keys"],
    });
    expect(findSpecViolations([skill])).toEqual([]);
  });

  it("includes only the blocking violations in detail when both kinds are present", () => {
    const skill = fixtureSkill({
      spec_violations: [
        "missing required frontmatter field: name",
        "description exceeds 1024 characters",
      ],
    });
    const issues = findSpecViolations([skill]);
    expect(issues[0].detail).toBe("missing required frontmatter field: name");
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

describe("findDuplicateSkills", () => {
  it("names the differing copies against the strict majority", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ agent: "shared", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "Claude Code", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "Cursor", scope: "global", content_hash: "bbb" }),
      ],
    });
    const issues = findDuplicateSkills([skill]);
    expect(issues).toHaveLength(1);
    expect(issues[0].detail).toBe("Global · Cursor differs from Global · shared");
  });

  it("uses a plural verb when more than one copy differs", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ agent: "shared", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "Claude Code", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "OpenCode", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "Cursor", scope: "global", content_hash: "bbb" }),
        fixtureDeployment({ agent: "Codex", scope: "global", content_hash: "ccc" }),
      ],
    });
    const issues = findDuplicateSkills([skill]);
    expect(issues[0].detail).toBe(
      "Global \u00b7 Cursor; Global \u00b7 Codex differ from Global \u00b7 shared",
    );
  });

  it("lists every copy when there is no strict majority", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ agent: "shared", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "Cursor", scope: "global", content_hash: "bbb" }),
      ],
    });
    const issues = findDuplicateSkills([skill]);
    expect(issues[0].detail).toBe("2 copies differ: Global · shared; Global · Cursor");
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
