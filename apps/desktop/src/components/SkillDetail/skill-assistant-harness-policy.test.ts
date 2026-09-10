// ============================================================================
// Skill Studio - assistant harness policy tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import {
  isSkillAssistantHarness,
  skillAssistantHarnessPolicy,
} from "./skill-assistant-harness-policy";

function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    id: "dep:v1/global/universal/find-bugs",
    destination: "universal",
    owner_kind: "manual",
    mutability: "mutable",
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
    ...overrides,
  };
}

function fixtureSkill(deployments: Deployment[]): InstalledSkill {
  return {
    name: "find-bugs",
    source: "getsentry/find-bugs",
    source_type: "github",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "dotagents",
    deployments,
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
    update_owner_ids: [],
    update_owners: [],
    description: null,
    fork: null,
    parked_at: null,
    skill_path: null,
    source_url: null,
    trial: null,
    trials: [],
    update_commit: null,
    update_commit_at: null,
    updated_at: null,
  };
}

describe("skillAssistantHarnessPolicy", () => {
  it("marks a disabled shared-root reader as not seeing the skill", () => {
    const policy = skillAssistantHarnessPolicy(
      fixtureSkill([fixtureDeployment({ disabled_readers: ["open-code"] })]),
    );

    expect(policy.defaultHarness).toBe("codex");
    expect(policy.items).toContainEqual({
      value: "open-code",
      label: "OpenCode (doesn't see this skill)",
    });
    expect(policy.items).toContainEqual({ value: "codex", label: "Codex" });
  });

  it("keeps a supported healthy own deployment visible when the shared reader is disabled", () => {
    const policy = skillAssistantHarnessPolicy(
      fixtureSkill([
        fixtureDeployment({ disabled_readers: ["codex", "open-code", "pi"] }),
        fixtureDeployment({
          agent: "OpenCode",
          path: "/home/.config/opencode/skills/find-bugs",
        }),
      ]),
    );

    expect(policy.defaultHarness).toBe("open-code");
    expect(policy.items).toContainEqual({ value: "open-code", label: "OpenCode" });
  });

  it("falls back to Claude Code without offering a Cursor-only deployment as runnable", () => {
    const policy = skillAssistantHarnessPolicy(
      fixtureSkill([
        fixtureDeployment({ agent: "Cursor", path: "/home/.cursor/skills/find-bugs" }),
      ]),
    );

    expect(policy.defaultHarness).toBe("claude-code");
    expect(policy.items.map((item) => item.value)).toEqual([
      "claude-code",
      "codex",
      "open-code",
      "pi",
    ]);
    expect(policy.items[0]?.label).toBe("Claude Code (doesn't see this skill)");
    expect(isSkillAssistantHarness("cursor")).toBe(false);
    expect(isSkillAssistantHarness(policy.defaultHarness)).toBe(true);
  });
});
