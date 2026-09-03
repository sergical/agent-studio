// ============================================================================
// Skill Studio - skill-location-status tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import {
  buildInvocationFiles,
  buildScopeGroups,
  folderReaders,
  invocationFooterNote,
  promoteToGlobal,
  rowMenu,
  siblingRows,
  skillRollup,
  titleLink,
} from "./skill-location-status";

function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    agent: "shared",
    scope: "global",
    path: "/home/.agents/skills/find-bugs",
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
    ...overrides,
  };
}

function fixtureSkill(overrides: Partial<InstalledSkill> = {}): InstalledSkill {
  return {
    name: "find-bugs",
    source: "getsentry/find-bugs",
    source_type: "github",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "dotagents",
    deployments: [fixtureDeployment()],
    has_spec: true,
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
    ...overrides,
  };
}

describe("buildScopeGroups", () => {
  it("flags a broken link with the error dot and its relink/remove menu", () => {
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      symlink_is_broken: true,
      symlink_target: "/home/.agents/skills/find-bugs",
      path: "/home/.claude/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [fixtureDeployment(), claude] });
    const [global] = buildScopeGroups(skill);
    const row = global.rows.find((r) => r.harness === "claude-code");
    expect(row?.level).toBe("error");
    expect(row?.conditions[0].what).toBe("Broken link. The target is missing.");
    const menu = rowMenu(row!, global.label);
    expect(menu.entries.map((e) => e.label)).toContain("Relink to the folder");
    expect(menu.danger.map((e) => e.label)).toContain("Remove broken link");
  });

  it("flags an unreadable link distinctly from a broken one", () => {
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      symlink_error: "permission denied",
      path: "/home/.claude/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [fixtureDeployment(), claude] });
    const [global] = buildScopeGroups(skill);
    const row = global.rows.find((r) => r.harness === "claude-code");
    expect(row?.conditions[0].what).toBe("Link cannot be read: permission denied.");
    expect(row?.conditions[0].status).toBe("Link unreadable");
  });

  it("treats a missing required field as blocking (error), won't-load wording", () => {
    const shared = fixtureDeployment({
      spec_violations: ["missing required frontmatter field: description"],
    });
    const skill = fixtureSkill({ deployments: [shared], has_spec: false });
    const [global] = buildScopeGroups(skill);
    expect(global.shared?.level).toBe("error");
    expect(global.shared?.conditions[0].what).toContain("SKILL.md will not load:");
  });

  it("treats a non-blocking violation as a soft warning that still loads", () => {
    const shared = fixtureDeployment({ spec_violations: ["description is over 1024 characters"] });
    const skill = fixtureSkill({ deployments: [shared] });
    const [global] = buildScopeGroups(skill);
    expect(global.shared?.level).toBe("warning");
    expect(global.shared?.conditions[0].what).toContain("The skill still loads.");
  });

  it("flags a copy that drifted from the shared truth", () => {
    const shared = fixtureDeployment({ content_hash: "aaa" });
    const codexCopy = fixtureDeployment({
      agent: "Codex",
      scope: "project",
      project_path: "/repo",
      path: "/repo/.codex/skills/find-bugs",
      content_hash: "zzz",
    });
    const skill = fixtureSkill({ deployments: [shared, codexCopy] });
    const groups = buildScopeGroups(skill);
    const project = groups.find((g) => !g.isGlobal)!;
    const row = project.rows.find((r) => r.harness === "codex");
    expect(row?.level).toBe("warning");
    expect(row?.conditions[0].what).toBe("This copy differs from the shared folder.");
  });

  it("treats a whole-root link as a plain link row with a switch", () => {
    const shared = fixtureDeployment();
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      shared_via_whole_dir_link: true,
      path: "/home/.claude/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [shared, claude] });
    const [global] = buildScopeGroups(skill);
    const row = global.rows.find((r) => r.harness === "claude-code");
    expect(row?.kind).toBe("link");
    expect(row?.level).toBe(null);
    expect(row?.hasSwitch).toBe(true);
    expect(row?.chip).toBe(null);
    expect(row?.conditions).toHaveLength(0);
  });

  it.each([
    ["codex-config", "Off for Codex — switched off in ~/.codex/config.toml."],
    ["opencode-permission", "Off for OpenCode — denied in opencode.json."],
    ["claude-link-removed", "Off for Claude Code — the link under ~/.claude/skills was removed."],
    ["studio-moved", "Off for pi — moved into .skill-studio-disabled."],
  ] as const)("reports the %s off mode with its own sentence", (disabledBy, expectedWhat) => {
    const agentLabel =
      disabledBy === "studio-moved"
        ? "pi"
        : disabledBy === "codex-config"
          ? "Codex"
          : disabledBy === "opencode-permission"
            ? "OpenCode"
            : "Claude Code";
    const shared = fixtureDeployment();
    const row = fixtureDeployment({
      agent: agentLabel,
      is_symlink: agentLabel === "Claude Code",
      disabled: true,
      disabled_by: disabledBy,
      path: `/home/.${agentLabel.toLowerCase()}/skills/find-bugs`,
    });
    const skill = fixtureSkill({ deployments: [shared, row] });
    const [global] = buildScopeGroups(skill);
    const found = global.rows.find((r) => r.harnessLabel === agentLabel);
    expect(found?.level).toBe("off");
    expect(found?.conditions[0].what).toBe(expectedWhat);
  });

  it("marks the whole scope off and parked when the skill is parked", () => {
    const parkedShared = fixtureDeployment({
      scope: "parked",
      path: "/home/.agents/skills-parked/find-bugs",
    });
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      disabled: true,
      disabled_by: "claude-link-removed",
      path: "/home/.claude/skills/find-bugs",
    });
    const skill = fixtureSkill({
      deployments: [parkedShared, claude],
      parked: true,
      parked_at: "2026-01-01T00:00:00Z",
    });
    const [global] = buildScopeGroups(skill);
    expect(global.parkedScope).toBe(true);
    expect(global.shared?.level).toBe("off");
    expect(global.shared?.conditions[0].what).toContain("Off everywhere");
    expect(global.rows.find((r) => r.harness === "claude-code")?.level).toBe("off");
  });

  it("flags parked-but-live with an error dot on the shared folder", () => {
    const parkedShared = fixtureDeployment({
      scope: "parked",
      path: "/home/.agents/skills-parked/find-bugs",
    });
    const liveCopy = fixtureDeployment({
      agent: "Codex",
      scope: "project",
      project_path: "/repo",
      path: "/repo/.codex/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [parkedShared, liveCopy], parked: true });
    const [global] = buildScopeGroups(skill);
    expect(global.shared?.level).toBe("error");
    expect(global.shared?.conditions[0].status).toBe("Parked but live");
  });

  it("synthesizes reader rows for agents that read the shared folder natively", () => {
    const shared = fixtureDeployment();
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      path: "/home/.claude/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [shared, claude] });
    const [global] = buildScopeGroups(skill);
    const pi = global.rows.find((r) => r.harness === "pi");
    expect(pi?.kind).toBe("reader");
    expect(pi?.chip).toBe("always on");
    const codex = global.rows.find((r) => r.harness === "codex");
    expect(codex?.hasSwitch).toBe(true);
    expect(codex?.chip).toBeNull();
  });

  it("keeps broken link errors on their own rows, not the folder", () => {
    const shared = fixtureDeployment();
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      symlink_is_broken: true,
      path: "/home/.claude/skills/find-bugs",
    });
    const codex = fixtureDeployment({
      agent: "Codex",
      is_symlink: true,
      symlink_is_broken: true,
      path: "/home/.codex/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [shared, claude, codex] });
    const [global] = buildScopeGroups(skill);
    expect(global.folderLevel).toBe(null);
    expect(global.rows.find((r) => r.harness === "claude-code")?.level).toBe("error");
    expect(global.rows.find((r) => r.harness === "codex")?.level).toBe("error");
  });

  it("rolls up to an all-off folder when every row is off but nothing errors", () => {
    // The folder rollup only looks at readers - Claude Code and Codex have
    // their own link deployments here, so only OpenCode (the one switchable
    // reader left) needs to be off for the folder to roll up as all-off.
    const shared = fixtureDeployment({ disabled_readers: ["open-code"] });
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      disabled: true,
      disabled_by: "claude-link-removed",
      path: "/home/.claude/skills/find-bugs",
    });
    const codex = fixtureDeployment({
      agent: "Codex",
      is_symlink: true,
      disabled: true,
      disabled_by: "codex-config",
      path: "/home/.codex/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [shared, claude, codex] });
    const [global] = buildScopeGroups(skill);
    expect(global.folderLevel).toBe("off");
    expect(global.folderTip.startsWith("Off everywhere:")).toBe(true);
  });

  it("puts Claude Code beside the folder, not inside it", () => {
    const shared = fixtureDeployment();
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      path: "/home/.claude/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [shared, claude] });
    const [global] = buildScopeGroups(skill);
    expect(folderReaders(global).some((row) => row.harness === "claude-code")).toBe(false);
    expect(folderReaders(global).every((row) => row.kind === "reader")).toBe(true);
    const siblings = siblingRows(global);
    expect(siblings).toHaveLength(1);
    expect(siblings[0]).toMatchObject({ harness: "claude-code", kind: "link" });
  });

  it("a scope with only copies has no reader rows", () => {
    const cursorCopy = fixtureDeployment({
      agent: "Cursor",
      scope: "project",
      project_path: "/repo",
      is_symlink: false,
      path: "/repo/.cursor/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [cursorCopy] });
    const groups = buildScopeGroups(skill);
    const project = groups.find((g) => !g.isGlobal)!;
    expect(folderReaders(project)).toHaveLength(0);
    expect(siblingRows(project)).toHaveLength(1);
  });
});

describe("skillRollup", () => {
  it("reports a lock-only skill with no deployments as a warning", () => {
    const skill = fixtureSkill({ deployments: [] });
    const groups = buildScopeGroups(skill);
    const roll = skillRollup(skill, groups);
    expect(roll.level).toBe("warning");
    expect(roll.tip).toContain("Listed in the lock file");
  });

  it("prefixes every child line with its scope", () => {
    const shared = fixtureDeployment();
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      symlink_is_broken: true,
      path: "/home/.claude/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [shared, claude] });
    const groups = buildScopeGroups(skill);
    const roll = skillRollup(skill, groups);
    expect(roll.tip).toContain("Global · Claude Code:");
  });
});

describe("titleLink", () => {
  it("orders unpark above drift, install-again, enable-everywhere and update", () => {
    const parkedShared = fixtureDeployment({ scope: "parked" });
    const liveCopy = fixtureDeployment({ agent: "Codex", scope: "project", project_path: "/repo" });
    const skill = fixtureSkill({
      deployments: [parkedShared, liveCopy],
      parked: true,
      has_update: true,
    });
    expect(titleLink(skill, true)).toBe("Unpark");
  });

  it("prefers Compare copies over Install again and Update when there's drift", () => {
    const skill = fixtureSkill({ has_update: true });
    expect(titleLink(skill, true)).toBe("Compare copies");
  });

  it("prefers Install again over Enable everywhere and Update for a lock-only skill", () => {
    const skill = fixtureSkill({ deployments: [], parked: true, has_update: true });
    expect(titleLink(skill, false)).toBe("Install again");
  });

  it("prefers Enable everywhere over Update for a parked skill", () => {
    const skill = fixtureSkill({
      deployments: [fixtureDeployment({ scope: "parked" })],
      parked: true,
      has_update: true,
    });
    expect(titleLink(skill, false)).toBe("Enable everywhere");
  });

  it("falls back to Update when nothing else applies", () => {
    const skill = fixtureSkill({ has_update: true });
    expect(titleLink(skill, false)).toBe("Update");
  });

  it("returns null when there is nothing to fix", () => {
    const skill = fixtureSkill();
    expect(titleLink(skill, false)).toBeNull();
  });
});

describe("rowMenu", () => {
  it("leads with the highest condition's first fix even when it is destructive", () => {
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      symlink_error: "permission denied",
      path: "/home/.claude/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [fixtureDeployment(), claude] });
    const [global] = buildScopeGroups(skill);
    const row = global.rows.find((r) => r.harness === "claude-code")!;
    const menu = rowMenu(row, global.label);
    expect(menu.entries[0].label).toBe("Remove link");
  });

  it("puts non-leading danger items after the plain entries, in their own bucket", () => {
    const shared = fixtureDeployment();
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      path: "/home/.claude/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [shared, claude] });
    const [global] = buildScopeGroups(skill);
    const row = global.rows.find((r) => r.harness === "claude-code")!;
    const menu = rowMenu(row, global.label);
    expect(menu.entries.map((e) => e.danger ?? false)).not.toContain(true);
    expect(menu.danger.map((e) => e.label)).toContain("Remove link");
  });

  it("puts the mechanism hint under the enable action for an off row", () => {
    const shared = fixtureDeployment();
    const codex = fixtureDeployment({
      agent: "Codex",
      is_symlink: true,
      disabled: true,
      disabled_by: "codex-config",
      path: "/home/.codex/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [shared, codex] });
    const [global] = buildScopeGroups(skill);
    const row = global.rows.find((r) => r.harness === "codex")!;
    const menu = rowMenu(row, global.label);
    expect(menu.hint).toBe("Turns it back on in Codex's config.toml.");
  });
});

describe("buildInvocationFiles / invocationFooterNote", () => {
  it("builds one footer row per SKILL.md file, skipping per-skill links", () => {
    const shared = fixtureDeployment();
    const claude = fixtureDeployment({
      agent: "Claude Code",
      is_symlink: true,
      path: "/home/.claude/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [shared, claude] });
    const groups = buildScopeGroups(skill);
    const files = buildInvocationFiles(groups, skill);
    expect(files).toHaveLength(1);
    expect(files[0].kind).toBe("shared");
  });

  it("explains the single file's own invocation value", () => {
    const shared = fixtureDeployment({ invocation: "user-only" });
    const skill = fixtureSkill({ deployments: [shared], invocation: "user-only" });
    const groups = buildScopeGroups(skill);
    const files = buildInvocationFiles(groups, skill);
    expect(invocationFooterNote(files, skill.name)).toBe("User only: only /find-bugs starts it.");
  });

  it("points at each-file-sets-its-own with more than one file", () => {
    const shared = fixtureDeployment();
    const codexCopy = fixtureDeployment({
      agent: "Codex",
      scope: "project",
      project_path: "/repo",
      path: "/repo/.codex/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [shared, codexCopy] });
    const groups = buildScopeGroups(skill);
    const files = buildInvocationFiles(groups, skill);
    expect(files).toHaveLength(2);
    expect(invocationFooterNote(files, skill.name)).toBe(
      "Each file sets its own. Symlinks follow the folder they point to.",
    );
  });
});

describe("buildInvocationFiles editability", () => {
  it("keeps the global shared folder editable even when the skill is managed", () => {
    const shared = fixtureDeployment();
    const skill = fixtureSkill({ deployments: [shared], source_kind: "dotagents" });
    const files = buildInvocationFiles(buildScopeGroups(skill), skill);
    expect(files[0]).toMatchObject({ kind: "shared", editable: true });
  });

  it("disables a managed project shared folder", () => {
    const projectShared = fixtureDeployment({
      scope: "project",
      project_path: "/repo",
      path: "/repo/.agents/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [projectShared], source_kind: "skills-sh" });
    const files = buildInvocationFiles(buildScopeGroups(skill), skill);
    expect(files[0]).toMatchObject({ kind: "shared", editable: false });
    expect(files[0].disabledReason).toContain("skills.sh");
  });

  it("disables a managed copy", () => {
    const copy = fixtureDeployment({
      agent: "Cursor",
      scope: "project",
      project_path: "/repo",
      is_symlink: false,
      path: "/repo/.cursor/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [copy], source_kind: "dotagents" });
    const files = buildInvocationFiles(buildScopeGroups(skill), skill);
    expect(files[0]).toMatchObject({ kind: "copy", editable: false });
    expect(files[0].disabledReason).toContain("dotagents");
  });

  it("keeps a manual copy editable in place", () => {
    const copy = fixtureDeployment({
      agent: "Cursor",
      scope: "project",
      project_path: "/repo",
      is_symlink: false,
      path: "/repo/.cursor/skills/find-bugs",
    });
    const skill = fixtureSkill({ deployments: [copy], source_kind: "manual" });
    const files = buildInvocationFiles(buildScopeGroups(skill), skill);
    expect(files[0]).toMatchObject({ kind: "copy", editable: true });
  });

  it("disables a plugin file regardless of the skill's own source_kind", () => {
    const plugin = fixtureDeployment({
      agent: "Codex",
      scope: "plugin",
      is_symlink: false,
      path: "/home/.codex/plugins/cache/foo/skills/find-bugs",
      plugin: { name: "openai-templates", harness: "Codex" },
    });
    const skill = fixtureSkill({ deployments: [plugin], source_kind: "manual" });
    const files = buildInvocationFiles(buildScopeGroups(skill), skill);
    expect(files[0]).toMatchObject({ kind: "plugin", editable: false });
    expect(files[0].disabledReason).toContain("openai-templates");
  });
});

describe("promoteToGlobal", () => {
  const projectShared = (project: string) =>
    fixtureDeployment({
      scope: "project",
      project_path: project,
      path: `${project}/.agents/skills/find-bugs`,
    });
  const projectClaudeLink = (project: string) =>
    fixtureDeployment({
      agent: "Claude Code",
      scope: "project",
      project_path: project,
      is_symlink: true,
      symlink_target: `${project}/.agents/skills/find-bugs`,
      path: `${project}/.claude/skills/find-bugs`,
    });

  it("offers the first project's folder when two projects have it and Global does not", () => {
    const skill = fixtureSkill({
      deployments: [
        projectShared("/repo-a"),
        projectClaudeLink("/repo-a"),
        projectShared("/repo-b"),
      ],
    });
    expect(promoteToGlobal(buildScopeGroups(skill))).toEqual({
      path: "/repo-a/.agents/skills/find-bugs",
      agents: ["claude-code"],
    });
  });

  it("offers nothing when a global copy already exists", () => {
    const skill = fixtureSkill({
      deployments: [fixtureDeployment(), projectShared("/repo-a"), projectShared("/repo-b")],
    });
    expect(promoteToGlobal(buildScopeGroups(skill))).toBe(null);
  });

  it("offers nothing for a single project", () => {
    const skill = fixtureSkill({ deployments: [projectShared("/repo-a")] });
    expect(promoteToGlobal(buildScopeGroups(skill))).toBe(null);
  });
});
