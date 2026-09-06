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
  };
}

describe("materializeRequestForLocationAction", () => {
  it("routes an explicit conversion as convert-only", () => {
    expect(
      materializeRequestForLocationAction({
        kind: "convert-root",
        target: { deployment_id: "deployment" },
        harness: "claude-code",
        root: "/home/.claude/skills",
      }),
    ).toMatchObject({ intent: "convert-only", root: "/home/.claude/skills" });
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
