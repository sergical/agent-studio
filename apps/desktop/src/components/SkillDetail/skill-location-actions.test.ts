// ============================================================================
// Skill Studio - skill location action routing tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment } from "@skill-studio/lib";
import { materializeRequestForLocationAction } from "./skill-location-actions";

function wholeRootDeployment(): Deployment {
  return {
    id: "dep:v1/global/claude-code/find-bugs",
    destination: "universal",
    owner_kind: "manual",
    mutability: "read-only",
    backing: { kind: "linked-to", deployment_id: "dep:v1/global/universal/find-bugs" },
    agent: "Claude Code",
    scope: "global",
    path: "/home/.claude/skills/find-bugs",
    is_symlink: true,
    shared_via_whole_dir_link: true,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
    codex_implicit_invocation: null,
    disabled_by: null,
    invocation: "both",
    spec_violations: [],
  };
}

describe("materializeRequestForLocationAction", () => {
  it.each([
    ["claude-code", "Claude Code"],
    ["open-code", "OpenCode"],
  ] as const)("routes an explicit %s conversion with display label %s", (harness, harnessLabel) => {
    expect(
      materializeRequestForLocationAction({
        kind: "convert-root",
        target: { deployment_id: "deployment" },
        harness,
        root: "/home/.claude/skills",
      }),
    ).toEqual({
      target: { deployment_id: "deployment" },
      harness,
      harnessLabel,
      root: "/home/.claude/skills",
      intent: "convert-only",
    });
  });

  it("routes only whole-root toggle-off as convert-then-disable", () => {
    const deployment = wholeRootDeployment();
    expect(
      materializeRequestForLocationAction({ kind: "set-enabled", deployment, enabled: false }),
    ).toMatchObject({
      intent: "convert-then-disable",
      target: { deployment_id: deployment.id },
    });
    expect(
      materializeRequestForLocationAction({ kind: "set-enabled", deployment, enabled: true }),
    ).toBeNull();
    expect(
      materializeRequestForLocationAction({
        kind: "set-enabled",
        deployment: { ...deployment, shared_via_whole_dir_link: false },
        enabled: false,
      }),
    ).toBeNull();
  });
});
