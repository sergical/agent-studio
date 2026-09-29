// ============================================================================
// Skill Studio - properties rail harness model tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { buildScopeGroups } from "./skill-location-status";
import { railHarnessEntries, readerToggleAction } from "./skill-properties-rail-model";

function universalOnlySkill(): InstalledSkill {
  const deployment: Deployment = {
    id: "dep:v1/global/universal/find-bugs",
    destination: "universal",
    owner_kind: "manual",
    mutability: "read-only",
    backing: { kind: "canonical" },
    agent: "shared",
    scope: "global",
    path: "/home/.agents/skills/find-bugs",
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
    codex_implicit_invocation: null,
    disabled_by: null,
    invocation: "both",
    spec_violations: [],
    shared_via_whole_dir_link: false,
  };
  return {
    name: "find-bugs",
    source: "local",
    source_type: "local",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "manual",
    deployments: [deployment],
    has_spec: true,
    spec_violations: [],
    skill_md_tokens: 0,
    description_tokens: 0,
    folder_bytes: 0,
    file_count: 0,
    content_hash: "abc",
    content_hashes: ["abc"],
    frontmatter_fields: {},
    folder_truncated: false,
    parked: false,
    invocation: "both",
    update_owner_ids: [],
    update_owners: [],
    description: null,
    fork: null,
    parked_at: null,
    skill_path: null,
    source_url: null,
    update_commit: null,
    update_commit_at: null,
    updated_at: null,
  };
}

describe("properties rail Harnesses popover", () => {
  it("a Universal-only skill lists Codex and OpenCode switches and toggling Codex sends set-reader-enabled", () => {
    const skill = universalOnlySkill();
    const entries = railHarnessEntries(skill, buildScopeGroups(skill));

    for (const harness of ["codex", "open-code"] as const) {
      const entry = entries.find((e) => e.harness === harness);
      expect(entry, `the rail does not list ${harness} for a Universal-only skill`).toBeDefined();
      expect(entry!.row?.kind).toBe("reader");
      expect(entry!.row?.hasSwitch, `the rail's ${harness} entry has no switch`).toBe(true);
    }

    const codexRow = entries.find((e) => e.harness === "codex")!.row!;
    expect(
      readerToggleAction(codexRow, false),
      "toggling Codex off in the rail does not send set-reader-enabled for the Universal folder",
    ).toEqual({
      kind: "set-reader-enabled",
      target: codexRow.lifecycleTarget,
      agent: "codex",
      enabled: false,
    });
  });
});
