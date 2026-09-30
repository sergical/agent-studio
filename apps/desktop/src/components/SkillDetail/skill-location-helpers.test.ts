// ============================================================================
// Skill Studio - skill location helper tests
// ============================================================================

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { SwitchControl } from "../ui/SwitchControl";
import {
  canOfferHarnessSwitch,
  canOfferHarnessSwitchForRow,
  harnessSwitchOffTitle,
  NO_OFF_SWITCH_TITLE,
  REGISTRY_COPY_NO_SWITCH_TITLE,
  sharedFolderSwitchPolicy,
} from "./skill-location-helpers";
import { buildScopeGroups, rowMenu } from "./skill-location-status";
import type { AgentLocationRow } from "./skill-location-status";
import {
  perSkillLinkDeployment,
  realCopyDeployment,
  universalDeployment,
  wholeFolderDeployment,
  withScannerIdentity,
} from "../../dev/harness/scanned-deployment";

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

describe("canOfferHarnessSwitch", () => {
  it("harness_rail_toggle_is_disabled_for_a_copy_that_cannot_park_or_names_the_row", () => {
    const projectCopy = sharedDeployment({
      agent: "Claude Code",
      scope: "project",
      project_path: "/repo",
      path: "/repo/.claude/skills/find-bugs",
      is_symlink: false,
      disabled: false,
      disabled_by: null,
    });

    // A project-scope copy has no native per-skill disable and was never
    // moved aside, so the Harnesses rail must not offer a switch for it -
    // `park`/`unpark` only ever target the Global Universal deployment.
    expect(canOfferHarnessSwitch(projectCopy)).toBe(false);
  });

  it("claude_code_switch_is_live_for_every_global_layout_or_names_the_layout", () => {
    // Claude Code's off switch writes `skillOverrides` in ~/.claude/settings.json,
    // so it works however the entry reaches ~/.claude/skills.
    const universalPath = "/home/.agents/skills/find-bugs";
    const path = "/home/.claude/skills/find-bugs";
    const layouts = {
      "per-skill link": perSkillLinkDeployment({ agent: "Claude Code", path, universalPath }),
      "whole-folder link": wholeFolderDeployment({ agent: "Claude Code", path, universalPath }),
      "real copy": realCopyDeployment({ agent: "Claude Code", path }),
    };
    for (const [layout, deployment] of Object.entries(layouts)) {
      expect(canOfferHarnessSwitch(deployment), `${layout}: the switch is not offered`).toBe(true);
    }
  });

  it("studio_moved_row_keeps_its_switch_or_names_the_missing_native_disable", () => {
    const movedCopy = sharedDeployment({
      agent: "pi",
      scope: "project",
      project_path: "/repo",
      path: "/repo/.pi/skills/find-bugs",
      disabled: true,
      disabled_by: "studio-moved",
    });

    expect(canOfferHarnessSwitch(movedCopy)).toBe(true);
  });

  it("studio_moved_copy_row_has_no_switch_or_names_the_row", () => {
    const registryCopy = sharedDeployment({
      agent: "pi",
      scope: "project",
      project_path: "/repo",
      path: "/repo/.pi/skills/find-bugs",
      owner_kind: "copy",
      disabled: true,
      disabled_by: "studio-moved",
    });

    // `restore_moved_deployment` refuses a Copy-owned row outright
    // (`refuse_registry_copy_restore`), so the switch must not pretend it
    // has a way back either.
    expect(canOfferHarnessSwitch(registryCopy)).toBe(false);
    const row: AgentLocationRow = {
      kind: "copy",
      harness: "pi",
      harnessLabel: "pi",
      path: registryCopy.path,
      caption: "",
      conditions: [],
      level: null,
      deployment: registryCopy,
      lifecycleTarget: { deployment_id: registryCopy.id },
      hasSwitch: false,
      switchOn: false,
      invocation: null,
    };
    expect(harnessSwitchOffTitle(row)).toBe(REGISTRY_COPY_NO_SWITCH_TITLE);

    // `canToggleHarness` alone would read a disabled claude-code/codex/open-code
    // row as switchable regardless of ownership, so the Copy-owned guard must
    // run ahead of that branch too, not just the studio-moved fallback.
    const registryCopyOnClaudeCode = sharedDeployment({
      agent: "Claude Code",
      scope: "project",
      project_path: "/repo",
      path: "/repo/.claude/skills/find-bugs",
      owner_kind: "copy",
      disabled: true,
      disabled_by: "studio-moved",
    });
    expect(canOfferHarnessSwitch(registryCopyOnClaudeCode)).toBe(false);
  });
});

// `SkillPropertiesRail`'s Harnesses popover renders `SwitchControl` inline
// (no standalone row component to import), so this exercises the rail's
// exact disabled/title wiring - `canOfferHarnessSwitchForRow` +
// `NO_OFF_SWITCH_TITLE` - against the same `SwitchControl` it renders.
describe("harness rail switch (canOfferHarnessSwitchForRow + NO_OFF_SWITCH_TITLE)", () => {
  it("harness_rail_switch_is_disabled_for_a_copy_that_cannot_park_or_names_the_row", () => {
    const deployment = sharedDeployment({
      agent: "Claude Code",
      scope: "project",
      project_path: "/repo",
      path: "/repo/.claude/skills/find-bugs",
    });
    const row: AgentLocationRow = {
      kind: "copy",
      harness: "claude-code",
      harnessLabel: "Claude Code",
      path: deployment.path,
      caption: "",
      conditions: [],
      level: null,
      deployment,
      lifecycleTarget: { deployment_id: deployment.id },
      hasSwitch: false,
      switchOn: false,
      invocation: null,
    };
    const offerSwitch = canOfferHarnessSwitchForRow(row);
    expect(offerSwitch).toBe(false);

    const markup = renderToStaticMarkup(
      createElement(SwitchControl, {
        checked: row.switchOn,
        disabled: !offerSwitch,
        onCheckedChange: () => undefined,
        ariaLabel: "Enabled for Claude Code",
        title: offerSwitch ? undefined : NO_OFF_SWITCH_TITLE,
      }),
    );
    expect(markup).toContain(`title="${NO_OFF_SWITCH_TITLE}"`);
    expect(markup).toContain("disabled=");
  });
});

describe("per-harness reader switches", () => {
  it("pi, Cursor and Grok Build reader rows show a disabled switch whose title says to Park", () => {
    const [global] = buildScopeGroups(skillWithDeployments([sharedDeployment()]));
    for (const harness of ["pi", "cursor", "grok-build"] as const) {
      const row = global.rows.find((r): r is AgentLocationRow => r.harness === harness);
      expect(row, `the Global group lost the ${harness} reader row`).toBeDefined();
      expect(
        canOfferHarnessSwitchForRow(row!),
        `${harness} has no per-skill switch, but its row offers one`,
      ).toBe(false);
      expect(
        harnessSwitchOffTitle(row!),
        `the ${harness} switch title does not point at Park as the off path`,
      ).toContain("Park the skill to turn it off for every harness");
    }
  });

  it("a project reader row hides the Codex and OpenCode switches and the Global reader row keeps them", () => {
    const projectShared = sharedDeployment({
      id: "dep:v1/project/universal/find-bugs",
      scope: "project",
      project_path: "/repo",
      path: "/repo/.agents/skills/find-bugs",
    });
    const groups = buildScopeGroups(skillWithDeployments([sharedDeployment(), projectShared]));
    const global = groups.find((g) => g.isGlobal)!;
    const project = groups.find((g) => !g.isGlobal)!;
    for (const harness of ["codex", "open-code"] as const) {
      const globalRow = global.rows.find((r) => r.harness === harness);
      const projectRow = project.rows.find((r) => r.harness === harness);
      expect(globalRow?.kind).toBe("reader");
      expect(projectRow?.kind).toBe("reader");
      expect(globalRow?.hasSwitch, `the Global ${harness} reader row lost its switch`).toBe(true);
      expect(
        projectRow?.hasSwitch,
        `the project ${harness} reader row offers a switch that writes the Global config`,
      ).toBe(false);
    }
  });
});
