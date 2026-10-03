// ============================================================================
// Skill Studio - skill location helper tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { sharedFolderSwitchPolicy } from "./skill-location-helpers";
import { buildScopeGroups, rowMenu } from "./skill-location-status";
import { universalDeployment, withScannerIdentity } from "../../dev/harness/scanned-deployment";

/** A Global Universal row with `overrides`; id, destination, and backing follow the scanner. */
function sharedDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return withScannerIdentity({
    ...universalDeployment(
      { universalPath: "/home/.agents/skills/find-bugs" },
      { owner_kind: "manual", mutability: "read-only", content_hash: "abc" },
    ),
    ...overrides,
  });
}

function skillWithDeployments(deployments: Deployment[]): InstalledSkill {
  return {
    name: "find-bugs",
    source: "local",
    source_type: "local",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "manual",
    deployments,
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

describe("sharedFolderSwitchPolicy", () => {
  it("keeps same-name Global and Project Universal folders isolated", () => {
    const skill = skillWithDeployments([
      sharedDeployment(),
      sharedDeployment({
        scope: "project",
        project_path: "/repo",
        path: "/repo/.agents/skills/find-bugs",
        disabled: true,
      }),
    ]);
    const groups = buildScopeGroups(skill);
    const global = groups.find((group) => group.isGlobal)!;
    const project = groups.find((group) => !group.isGlobal)!;
    const globalPolicy = sharedFolderSwitchPolicy(global);
    const projectPolicy = sharedFolderSwitchPolicy(project);

    expect(globalPolicy).toMatchObject({ checked: true, disabled: false });
    expect(globalPolicy.actionForCheckedChange(false)).toEqual({ kind: "park" });
    expect(globalPolicy.actionForCheckedChange(true)).toEqual({ kind: "unpark" });

    expect(projectPolicy).toMatchObject({ checked: false, disabled: true });
    expect(projectPolicy.actionForCheckedChange(false)).toBeNull();
    expect(projectPolicy.actionForCheckedChange(true)).toBeNull();
    expect(
      rowMenu(project.shared!, project.label, project.projectPath ?? null).entries.map(
        (entry) => entry.action.kind,
      ),
    ).not.toContain("park");
  });
});

describe("agent rows", () => {
  it("offers no per-agent switch on any row of a Global Universal skill, or names the row that does", () => {
    const groups = buildScopeGroups(skillWithDeployments([sharedDeployment()]));
    const global = groups.find((g) => g.isGlobal)!;
    const withSwitch = global.rows
      .filter((row) => row.kind !== "shared" && row.hasSwitch)
      .map((row) => row.harness);
    expect(withSwitch, "these agent rows still offer an on/off switch").toEqual([]);
  });

  it("captions a Codex reader row 'Hidden by Codex setting' and offers 'Open config.toml', or names what the row shows instead", () => {
    const groups = buildScopeGroups(
      skillWithDeployments([
        sharedDeployment({
          disabled_readers: ["codex"],
          disabling_config_files: [{ agent: "codex", path: "/custom/codex-home/config.toml" }],
        }),
      ]),
    );
    const codex = groups.find((g) => g.isGlobal)!.rows.find((r) => r.harness === "codex")!;
    expect(codex.caption).toBe("Hidden by Codex setting");
    expect(rowMenu(codex, "Global").entries[0]).toMatchObject({
      label: "Open config.toml",
      action: { kind: "open-editor", path: "/custom/codex-home/config.toml", label: "config.toml" },
    });
  });

  it("opens_the_global_settings_file_from_a_project_row_hidden_by_a_setting_or_names_the_path_it_opens", () => {
    const projectClaude = withScannerIdentity({
      ...sharedDeployment({
        agent: "Claude Code",
        scope: "project",
        project_path: "/proj",
        path: "/proj/.claude/skills/find-bugs",
        is_symlink: false,
        disabled: true,
        disabled_by: "claude-skill-overrides",
        disabling_config_files: [{ agent: "claude-code", path: "/home/.claude/settings.json" }],
      }),
    });
    const groups = buildScopeGroups(skillWithDeployments([projectClaude]));
    const row = groups.find((g) => !g.isGlobal)!.rows.find((r) => r.harness === "claude-code")!;
    expect(rowMenu(row, "Project").entries[0]).toMatchObject({
      label: "Open settings.json",
      action: { kind: "open-editor", path: "/home/.claude/settings.json" },
    });
  });
});
