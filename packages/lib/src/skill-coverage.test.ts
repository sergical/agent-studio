// ============================================================================
// Skill Studio - skill-coverage tests
// ============================================================================

import { describe, expect, it } from "vitest";
import {
  AGENT_MATRIX_LABELS,
  agentIdFromDeploymentLabel,
  deploymentRelationText,
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

describe("groupDeploymentsForDisplay", () => {
  it("groups a Universal root with the harnesses that read it through a whole-root link", () => {
    const shared = fixtureDeployment({ agent: "shared" });
    const readers = [
      fixtureDeployment({
        agent: "Claude Code",
        path: "/home/.claude/skills/find-bugs",
        resolved_path: "/home/.agents/skills/find-bugs",
        shared_via_whole_dir_link: true,
      }),
      fixtureDeployment({
        agent: "Codex",
        path: "/home/.codex/skills/find-bugs",
        resolved_path: "/home/.agents/skills/find-bugs",
        shared_via_whole_dir_link: true,
      }),
    ];
    const result = groupDeploymentsForDisplay([shared, ...readers]);
    expect(result.groups).toHaveLength(1);
    expect(result.groups[0]?.shared).toBe(shared);
    expect(result.groups[0]?.linked).toEqual(readers);
    expect(result.standalone).toEqual([]);
  });

  it("keeps a per-skill symlink out of the group - it is a location of its own", () => {
    const shared = fixtureDeployment({ agent: "shared" });
    const symlink = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      symlink_target: "/home/.agents/skills/find-bugs",
    });
    const result = groupDeploymentsForDisplay([shared, symlink]);
    expect(result.groups[0]?.linked).toEqual([]);
    expect(result.standalone).toEqual([symlink]);
  });

  it("produces only standalone rows when there is no Universal root", () => {
    const deployments = [
      fixtureDeployment({ agent: "Claude Code" }),
      fixtureDeployment({ agent: "Codex" }),
    ];
    const result = groupDeploymentsForDisplay(deployments);
    expect(result.groups).toEqual([]);
    expect(result.standalone).toEqual(deployments);
  });

  it("never puts a broken link inside the Universal group", () => {
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

  it("keeps global and project Universal roots separate", () => {
    const globalShared = fixtureDeployment({
      agent: "shared",
      scope: "global",
      path: "/home/.agents/skills/find-bugs",
    });
    const globalReader = fixtureDeployment({
      agent: "Codex",
      scope: "global",
      path: "/home/.codex/skills/find-bugs",
      resolved_path: "/home/.agents/skills/find-bugs",
      shared_via_whole_dir_link: true,
    });
    const projectShared = fixtureDeployment({
      agent: "shared",
      scope: "project",
      project_path: "/home/src/sentry",
      path: "/home/src/sentry/.agents/skills/find-bugs",
    });
    const projectReader = fixtureDeployment({
      agent: "Claude Code",
      scope: "project",
      project_path: "/home/src/sentry",
      path: "/home/src/sentry/.claude/skills/find-bugs",
      resolved_path: "/home/src/sentry/.agents/skills/find-bugs",
      shared_via_whole_dir_link: true,
    });

    const result = groupDeploymentsForDisplay([
      globalShared,
      globalReader,
      projectShared,
      projectReader,
    ]);

    expect(result.groups).toHaveLength(2);
    const globalGroup = result.groups.find((g) => g.shared === globalShared);
    const projectGroup = result.groups.find((g) => g.shared === projectShared);
    expect(globalGroup?.linked).toEqual([globalReader]);
    expect(projectGroup?.linked).toEqual([projectReader]);
    expect(result.standalone).toEqual([]);
  });

  it("leaves a reader standalone when its root link matches no Universal root", () => {
    const shared = fixtureDeployment({ agent: "shared", path: "/home/.agents/skills/find-bugs" });
    const unrelatedReader = fixtureDeployment({
      agent: "Codex",
      scope: "project",
      project_path: "/home/other",
      path: "/home/other/.codex/skills/find-bugs",
      resolved_path: "/home/other/.agents/skills/find-bugs",
      shared_via_whole_dir_link: true,
    });
    const result = groupDeploymentsForDisplay([shared, unrelatedReader]);
    expect(result.groups[0]?.linked).toEqual([]);
    expect(result.standalone).toEqual([unrelatedReader]);
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
  it("puts the Universal deployment in truth", () => {
    const skill = fixtureSkill({
      deployments: [fixtureDeployment({ agent: "shared", path: "/home/.agents/skills/find-bugs" })],
    });
    expect(locationSummary(skill).truth?.agent).toBe("shared");
  });

  it("groups a symlink into the Universal root as a link", () => {
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

  it("groups a healthy symlink outside the Universal root as a copy", () => {
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

describe("deploymentRelationText", () => {
  it("says the Universal root lives here", () => {
    expect(
      deploymentRelationText(
        fixtureDeployment({ agent: "shared", path: "/Users/me/.agents/skills/find-bugs" }),
      ),
    ).toBe("lives here");
  });

  it("calls a per-skill link into the Universal root a symlink", () => {
    expect(
      deploymentRelationText(
        fixtureDeployment({
          is_symlink: true,
          symlink_target: "/Users/me/.agents/skills/find-bugs",
        }),
      ),
    ).toBe("symlink");
  });

  it("names the linked root a whole-dir link reads through", () => {
    expect(
      deploymentRelationText(
        fixtureDeployment({
          path: "/Users/me/.claude/skills/find-bugs",
          shared_via_whole_dir_link: true,
        }),
      ),
    ).toBe("reads this folder via ~/.claude/skills");
  });

  it("calls a real directory a copy", () => {
    expect(deploymentRelationText(fixtureDeployment())).toBe("copy");
  });

  it("calls an unresolved link broken", () => {
    expect(
      deploymentRelationText(fixtureDeployment({ is_symlink: true, symlink_is_broken: true })),
    ).toBe("broken link");
  });
});
