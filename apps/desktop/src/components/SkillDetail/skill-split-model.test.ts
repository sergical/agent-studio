// ============================================================================
// Skill Studio - skill-split-model tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { AgentId, SplitCopy } from "@skill-studio/lib";
import { perSkillLinkDeployment, universalDeployment } from "../../dev/harness/scanned-deployment";
import { skill } from "../../dev/harness/skill-fixture";
import { buildScopeGroups, rowMenu } from "./skill-location-status";
import { SPLIT_HARNESSES, splitFolderRows, splitReaders } from "./skill-split-model";

const UNIVERSAL = "/home/.agents/skills/find-bugs";

describe("split dialog model", () => {
  it("split_dialog_lists_one_folder_per_checked_harness_in_harness_order_or_the_user_cannot_see_what_is_written", () => {
    const targets: SplitCopy[] = [
      { harness: "open-code", path: "/home/.opencode-custom/skills/find-bugs" },
      { harness: "claude-code", path: "/home/.claude/skills/find-bugs" },
      { harness: "codex", path: "/custom-codex/skills/find-bugs" },
      { harness: "pi", path: "/home/.pi/agent/skills/find-bugs" },
    ];
    const checked = new Set<AgentId>(["codex", "claude-code", "open-code"]);

    expect(splitFolderRows(targets, checked)).toEqual([
      { harness: "claude-code", label: "Claude Code", path: "/home/.claude/skills/find-bugs" },
      { harness: "codex", label: "Codex", path: "/custom-codex/skills/find-bugs" },
      { harness: "open-code", label: "OpenCode", path: "/home/.opencode-custom/skills/find-bugs" },
    ]);
  });

  it("universal_row_menu_offers_split_with_the_linked_harness_checked_or_the_dialog_would_drop_it_by_default", () => {
    const installed = skill({
      name: "find-bugs",
      deployments: [
        universalDeployment({ universalPath: UNIVERSAL }),
        perSkillLinkDeployment({
          agent: "Claude Code",
          path: "/home/.claude/skills/find-bugs",
          universalPath: UNIVERSAL,
        }),
      ],
    });
    const [global] = buildScopeGroups(installed);

    const menu = rowMenu(global.shared!, global.label, null, splitReaders(global));
    const split = menu.entries.find((entry) => entry.label === "Split into agent folders…");

    expect(split?.action).toMatchObject({ kind: "split", projectPath: null });
    const readers = split?.action.kind === "split" ? split.action.readers : [];
    expect(readers).toContain("claude-code");
    expect(readers.every((harness) => SPLIT_HARNESSES.includes(harness))).toBe(true);
  });

  it("split_is_not_offered_on_a_per_skill_link_row_or_the_user_could_split_from_a_link", () => {
    const installed = skill({
      name: "find-bugs",
      deployments: [
        universalDeployment({ universalPath: UNIVERSAL }),
        perSkillLinkDeployment({
          agent: "Claude Code",
          path: "/home/.claude/skills/find-bugs",
          universalPath: UNIVERSAL,
        }),
      ],
    });
    const [global] = buildScopeGroups(installed);
    const link = global.rows.find((row) => row.kind === "link")!;

    const labels = rowMenu(link, global.label).entries.map((entry) => entry.label);

    expect(labels).not.toContain("Split into agent folders…");
  });
});
