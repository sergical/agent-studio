// ============================================================================
// Skill Studio - skill location action routing tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { wholeFolderDeployment } from "../../dev/harness/scanned-deployment";
import { materializeRequestForLocationAction } from "./skill-location-actions";

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
    });
  });

  it("never_routes_the_enabled_switch_to_the_conversion_dialog_for_a_whole_folder_link_or_names_the_direction", () => {
    const deployment = wholeFolderDeployment({
      agent: "Claude Code",
      path: "/home/.claude/skills/find-bugs",
      universalPath: "/home/.agents/skills/find-bugs",
    });
    for (const enabled of [false, true]) {
      expect(
        materializeRequestForLocationAction({ kind: "set-enabled", deployment, enabled }),
        `switching ${enabled ? "on" : "off"} opened the conversion dialog`,
      ).toBeNull();
    }
  });
});
