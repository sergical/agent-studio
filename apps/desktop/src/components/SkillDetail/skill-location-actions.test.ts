// ============================================================================
// Skill Studio - skill location action routing tests
// ============================================================================

import { describe, expect, it, vi } from "vitest";
import type { Deployment } from "@skill-studio/lib";
import { universalDeployment, wholeFolderDeployment } from "../../dev/harness/scanned-deployment";
import {
  forkBeforeInvocationEdit,
  materializeRequestForLocationAction,
} from "./skill-location-actions";
import type { InvocationFile } from "./skill-location-status";

function sharedFile(ownerKind: Deployment["owner_kind"]): InvocationFile {
  const deployment = universalDeployment(
    { universalPath: "/home/.agents/skills/emil-design" },
    { owner_kind: ownerKind, mutability: "mutable", owner_id: null },
  );
  // SAFETY: forkBeforeInvocationEdit reads only `kind` and `deployment`.
  return { kind: "shared", deployment } as InvocationFile;
}

describe("forkBeforeInvocationEdit", () => {
  it("edits_an_ambiguous_shared_folder_in_place_because_the_fork_would_be_refused", async () => {
    const fork = vi.fn(async () => {});
    await forkBeforeInvocationEdit(sharedFile("ambiguous"), fork);
    expect(fork).not.toHaveBeenCalled();
  });

  it.each(["manual", "copy", "fork", "in-repo"] as const)(
    "edits_a_%s_shared_folder_in_place_because_it_has_no_upstream",
    async (ownerKind) => {
      const fork = vi.fn(async () => {});
      await forkBeforeInvocationEdit(sharedFile(ownerKind), fork);
      expect(fork).not.toHaveBeenCalled();
    },
  );

  it.each(["skills-sh", "dotagents", "wildcard-dotagents"] as const)(
    "forks_a_%s_shared_folder_first_so_an_update_does_not_overwrite_the_edit",
    async (ownerKind) => {
      const fork = vi.fn(async () => {});
      await forkBeforeInvocationEdit(sharedFile(ownerKind), fork);
      expect(fork).toHaveBeenCalledTimes(1);
    },
  );
});

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
