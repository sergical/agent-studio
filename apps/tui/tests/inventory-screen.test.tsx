import { testRender } from "@opentui/react/test-utils";
import { describe, expect, test } from "bun:test";

import type { DeploymentDto, Inventory } from "../src/cli-types.ts";
import { InventoryScreen } from "../src/screens/InventoryScreen.tsx";

function deployment(name: string): DeploymentDto {
  return {
    id: `deployment:v1/${name}`,
    root: { scope: { scope: "global" }, kind: { kind: "harness", harness: "claude-code" } },
    harness: "claude-code",
    path: `/home/.claude/skills/${name}`,
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
  };
}

function inventory(names: string[]): Inventory {
  return {
    skills: names.map((name) => ({ name, description: null, deployments: [deployment(name)] })),
    projects: [],
    completeness: "complete",
    observations: [],
    timings: [],
  };
}

describe("InventoryScreen", () => {
  test("renders every fixture skill", async () => {
    const names = ["alpha-skill", "beta-skill", "gamma-skill"];
    const setup = await testRender(
      <InventoryScreen inventory={inventory(names)} onOpenSkill={() => {}} />,
      {
        width: 80,
        height: 24,
      },
    );
    await setup.flush();
    const frame = await setup.waitForFrame((f) => f.includes(names[0] ?? ""));
    for (const name of names) {
      expect(frame).toContain(name);
    }
  });

  test("arrow navigation moves the selection and enter opens the skill", async () => {
    const names = ["alpha-skill", "beta-skill", "gamma-skill"];
    const opened: string[] = [];
    const setup = await testRender(
      <InventoryScreen inventory={inventory(names)} onOpenSkill={(name) => opened.push(name)} />,
      { width: 80, height: 24 },
    );
    await setup.renderOnce();
    setup.mockInput.pressArrow("down");
    setup.mockInput.pressArrow("down");
    await setup.renderOnce();
    setup.mockInput.pressEnter();
    await setup.renderOnce();
    expect(opened.at(-1)).toBe("gamma-skill");
  });
});
