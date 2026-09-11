import { describe, expect, test } from "bun:test";

import type { DeploymentDto, Inventory } from "../src/cli-types.ts";
import { buildSkillRows } from "../src/inventory-rows.ts";

function deployment(overrides: Partial<DeploymentDto>): DeploymentDto {
  return {
    id: "deployment:v1/test",
    root: { scope: { scope: "global" }, kind: { kind: "harness", harness: "claude-code" } },
    harness: "claude-code",
    path: "/home/.claude/skills/write-tests",
    destination: "harness",
    backing: "canonical",
    mutability: "mutable",
    link_target: null,
    shared_via_whole_dir_link: false,
    owner_kind: "none",
    owner_id: null,
    content_fingerprint: null,
    disabled_by: null,
    disabled_readers: [],
    spec_violations: [],
    plugin: null,
    ...overrides,
  };
}

function inventory(...deployments: DeploymentDto[]): Inventory {
  return {
    skills: deployments.map((d, index) => ({
      name: `skill-${String(index)}`,
      description: null,
      deployments: [d],
    })),
    projects: [],
    completeness: "complete",
    observations: [],
    timings: [],
  };
}

describe("buildSkillRows", () => {
  test("groups by the primary deployment's harness", () => {
    const rows = buildSkillRows(
      inventory(
        deployment({
          harness: "codex",
          root: { scope: { scope: "global" }, kind: { kind: "harness", harness: "codex" } },
        }),
        deployment({ harness: "claude-code" }),
      ),
    );
    expect(rows.map((r) => r.groupLabel)).toEqual(["claude-code", "codex"]);
  });

  test("labels the universal root when there is no harness", () => {
    const rows = buildSkillRows(
      inventory(
        deployment({
          harness: null,
          root: { scope: { scope: "global" }, kind: { kind: "universal" } },
        }),
      ),
    );
    expect(rows[0]?.groupLabel).toBe("universal");
  });

  test("surfaces disabled and broken-link state in the summary", () => {
    const rows = buildSkillRows(
      inventory(
        deployment({
          disabled_by: "codex_config",
          backing: "linked_to",
          link_target: null,
        }),
      ),
    );
    expect(rows[0]?.stateSummary).toContain("disabled");
    expect(rows[0]?.stateSummary).toContain("broken link");
  });
});
