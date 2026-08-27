// ============================================================================
// Skill Studio - skill-coverage tests
// ============================================================================

import { describe, expect, it } from "vitest";
import {
  AGENT_MATRIX_LABELS,
  agentIdFromDeploymentLabel,
  driftingCopies,
  groupDeploymentsForDisplay,
  locationSummary,
  pickCompareDefaults,
  resolveCompareSelection,
} from "./skill-coverage";
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

describe("groupDeploymentsForDisplay", () => {
  it("groups a shared root and its linked harnesses into one group of three", () => {
    const shared = fixtureDeployment({ agent: "shared" });
    const linked = [
      fixtureDeployment({
        agent: "Claude Code",
        is_symlink: true,
        symlink_target: "/home/.agents/skills/find-bugs",
      }),
      fixtureDeployment({
        agent: "Codex",
        is_symlink: true,
        symlink_target: "/home/.agents/skills/find-bugs",
      }),
      fixtureDeployment({
        agent: "Cursor",
        is_symlink: true,
        symlink_target: "/home/.agents/skills/find-bugs",
      }),
    ];
    const result = groupDeploymentsForDisplay([shared, ...linked]);
    expect(result.groups).toHaveLength(1);
    expect(result.groups[0]?.shared).toBe(shared);
    expect(result.groups[0]?.linked).toEqual(linked);
    expect(result.standalone).toEqual([]);
  });

  it("produces only standalone rows when there is no shared root", () => {
    const deployments = [
      fixtureDeployment({ agent: "Claude Code" }),
      fixtureDeployment({ agent: "Codex" }),
    ];
    const result = groupDeploymentsForDisplay(deployments);
    expect(result.groups).toEqual([]);
    expect(result.standalone).toEqual(deployments);
  });

  it("never puts a broken link inside the shared group", () => {
    const shared = fixtureDeployment({ agent: "shared" });
    const broken = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      symlink_is_broken: true,
      symlink_target: "/home/.agents/skills/find-bugs",
    });
    const result = groupDeploymentsForDisplay([shared, broken]);
    expect(result.groups[0]?.linked).toEqual([]);
    expect(result.standalone).toEqual([broken]);
  });

  it("keeps a global shared root and a project shared root as separate groups with no cross-contamination (regression: wrong-root nesting)", () => {
    const globalShared = fixtureDeployment({
      agent: "shared",
      scope: "global",
      path: "/home/.agents/skills/find-bugs",
    });
    const globalLinked = fixtureDeployment({
      agent: "Codex",
      scope: "global",
      is_symlink: true,
      symlink_target: "/home/.agents/skills/find-bugs",
    });
    const projectShared = fixtureDeployment({
      agent: "shared",
      scope: "project",
      project_path: "/home/src/sentry",
      path: "/home/src/sentry/.agents/skills/find-bugs",
    });
    const projectLinked = fixtureDeployment({
      agent: "Claude Code",
      scope: "project",
      project_path: "/home/src/sentry",
      is_symlink: true,
      symlink_target: "/home/src/sentry/.agents/skills/find-bugs",
    });

    const result = groupDeploymentsForDisplay([
      globalShared,
      globalLinked,
      projectShared,
      projectLinked,
    ]);

    expect(result.groups).toHaveLength(2);
    const globalGroup = result.groups.find((g) => g.shared === globalShared);
    const projectGroup = result.groups.find((g) => g.shared === projectShared);
    expect(globalGroup?.linked).toEqual([globalLinked]);
    expect(projectGroup?.linked).toEqual([projectLinked]);
    expect(result.standalone).toEqual([]);
  });

  it("leaves a link standalone when its target matches no shared root", () => {
    const shared = fixtureDeployment({ agent: "shared", path: "/home/.agents/skills/find-bugs" });
    const unrelatedLink = fixtureDeployment({
      agent: "Codex",
      is_symlink: true,
      symlink_target: "/home/other/.agents/skills/find-bugs",
      scope: "project",
      project_path: "/home/other",
    });
    const result = groupDeploymentsForDisplay([shared, unrelatedLink]);
    expect(result.groups[0]?.linked).toEqual([]);
    expect(result.standalone).toEqual([unrelatedLink]);
  });
});

describe("pickCompareDefaults", () => {
  it("picks the strict-majority copy as the left side", () => {
    const majority1 = fixtureDeployment({ agent: "shared", content_hash: "aaa" });
    const majority2 = fixtureDeployment({ agent: "Claude Code", content_hash: "aaa" });
    const minority = fixtureDeployment({ agent: "Cursor", content_hash: "bbb" });
    const { left, right } = pickCompareDefaults([majority1, majority2, minority]);
    expect(left).toBe(majority1);
    expect(right).toBe(minority);
  });

  it("falls back to the first candidate with no strict majority", () => {
    const first = fixtureDeployment({ agent: "shared", content_hash: "aaa" });
    const second = fixtureDeployment({ agent: "Cursor", content_hash: "bbb" });
    const { left, right } = pickCompareDefaults([first, second]);
    expect(left).toBe(first);
    expect(right).toBe(second);
  });

  it("returns undefined for both sides with no candidates", () => {
    expect(pickCompareDefaults([])).toEqual({ left: undefined, right: undefined });
  });
});

describe("resolveCompareSelection", () => {
  it("keeps the current selection when it is still a candidate", () => {
    const kept = fixtureDeployment({ path: "/home/.claude/skills/find-bugs" });
    const other = fixtureDeployment({ agent: "Codex", path: "/home/.codex/skills/find-bugs" });
    expect(resolveCompareSelection(kept.path, [kept, other], other.path)).toBe(kept.path);
  });

  it("falls back to the default pick when the current selection no longer exists", () => {
    const remaining = fixtureDeployment({ agent: "Codex", path: "/home/.codex/skills/find-bugs" });
    expect(
      resolveCompareSelection("/home/.claude/skills/find-bugs", [remaining], remaining.path),
    ).toBe(remaining.path);
  });

  it("treats an unset selection as gone, using the fallback", () => {
    const remaining = fixtureDeployment({ path: "/home/.claude/skills/find-bugs" });
    expect(resolveCompareSelection(undefined, [remaining], remaining.path)).toBe(remaining.path);
  });
});

describe("agentIdFromDeploymentLabel", () => {
  it("maps Cursor to cursor", () => {
    expect(agentIdFromDeploymentLabel("Cursor")).toBe("cursor");
  });

  it("maps Grok Build to grok-build", () => {
    expect(agentIdFromDeploymentLabel("Grok Build")).toBe("grok-build");
  });
});

describe("locationSummary", () => {
  it("puts the shared-root deployment in truth", () => {
    const skill = fixtureSkill({
      deployments: [fixtureDeployment({ agent: "shared", path: "/home/.agents/skills/find-bugs" })],
    });
    expect(locationSummary(skill).truth?.agent).toBe("shared");
  });

  it("groups a symlink into the shared root as a link", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({
          agent: "Codex",
          is_symlink: true,
          symlink_target: "/home/.agents/skills/find-bugs",
        }),
      ],
    });
    const summary = locationSummary(skill);
    expect(summary.links.map((d) => d.agent)).toEqual(["Codex"]);
    expect(summary.truth).toBeNull();
  });

  it("groups a real (non-symlink) own directory as a copy", () => {
    const skill = fixtureSkill({
      deployments: [fixtureDeployment({ agent: "Claude Code", is_symlink: false })],
    });
    expect(locationSummary(skill).copies.map((d) => d.agent)).toEqual(["Claude Code"]);
  });

  it("groups a healthy symlink to somewhere outside the shared root as a copy", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({
          agent: "Claude Code",
          is_symlink: true,
          symlink_target: "/home/src/skills-repo/find-bugs",
        }),
      ],
    });
    expect(locationSummary(skill).copies.map((d) => d.agent)).toEqual(["Claude Code"]);
  });

  it("groups an unresolved symlink as broken", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ agent: "Claude Code", is_symlink: true, symlink_is_broken: true }),
      ],
    });
    expect(locationSummary(skill).broken.map((d) => d.agent)).toEqual(["Claude Code"]);
  });

  it("groups a real (non-symlink) directory reached through a linked root as a link, not a copy", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({
          agent: "Claude Code",
          is_symlink: false,
          resolved_path: "/home/.agents/skills/find-bugs",
        }),
      ],
    });
    const summary = locationSummary(skill);
    expect(summary.links.map((d) => d.agent)).toEqual(["Claude Code"]);
    expect(summary.copies).toEqual([]);
  });
});

describe("driftingCopies", () => {
  it("finds no drift when copies share the truth's content hash", () => {
    const truth = fixtureDeployment({ agent: "shared", content_hash: "abc" });
    const copy = fixtureDeployment({ agent: "Codex", content_hash: "abc" });
    expect(driftingCopies({ truth, links: [], copies: [copy], broken: [] })).toEqual([]);
  });

  it("flags a copy whose content hash differs from the truth's", () => {
    const truth = fixtureDeployment({ agent: "shared", content_hash: "abc" });
    const copy = fixtureDeployment({ agent: "Codex", content_hash: "xyz" });
    expect(driftingCopies({ truth, links: [], copies: [copy], broken: [] })).toEqual([copy]);
  });
});

describe("AGENT_MATRIX_LABELS", () => {
  it("includes Cursor and Grok Build alongside the original four", () => {
    expect(AGENT_MATRIX_LABELS).toEqual([
      "Claude Code",
      "Codex",
      "OpenCode",
      "pi",
      "Cursor",
      "Grok Build",
    ]);
  });
});
