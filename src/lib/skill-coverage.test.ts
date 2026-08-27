// ============================================================================
// Skill Studio - skill-coverage tests
// ============================================================================

import { describe, expect, it } from "vitest";
import {
  AGENT_MATRIX_LABELS,
  agentIdFromDeploymentLabel,
  driftingCopies,
  locationSummary,
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
