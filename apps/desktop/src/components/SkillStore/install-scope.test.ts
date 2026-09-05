// ============================================================================
// Skill Studio - install-scope tests
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, InstalledSkill, SkillWithStatus } from "@skill-studio/lib";
import { resolveInstallScope } from "./install-scope";

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

function fixtureInstalledSkill(overrides: Partial<InstalledSkill> = {}): InstalledSkill {
  return {
    name: "find-bugs",
    source: "getsentry/find-bugs",
    source_type: "github",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "skills-sh",
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

function fixtureSkill(overrides: Partial<SkillWithStatus> = {}): SkillWithStatus {
  return {
    id: "getsentry/find-bugs",
    name: "find-bugs",
    installs: 0,
    is_installed: true,
    installed_info: fixtureInstalledSkill(),
    ...overrides,
  };
}

describe("resolveInstallScope", () => {
  it("targets the project deployment's path for a project-scoped install", () => {
    const skill = fixtureSkill({
      installed_info: fixtureInstalledSkill({
        deployments: [
          fixtureDeployment({
            scope: "project",
            project_path: "/work/my-repo",
            path: "/work/my-repo/.claude/skills/find-bugs",
          }),
        ],
      }),
    });

    expect(resolveInstallScope(skill)).toEqual({ scope: "project", projectPath: "/work/my-repo" });
  });

  it("falls back to global for a global-only install", () => {
    const skill = fixtureSkill({
      installed_info: fixtureInstalledSkill({
        deployments: [fixtureDeployment({ scope: "global" })],
      }),
    });

    expect(resolveInstallScope(skill)).toEqual({ scope: "global", projectPath: null });
  });

  it("falls back to global when the skill has no deployments", () => {
    const skill = fixtureSkill({
      installed_info: fixtureInstalledSkill({ deployments: [] }),
    });

    expect(resolveInstallScope(skill)).toEqual({ scope: "global", projectPath: null });
  });

  it("falls back to global when installed_info is missing", () => {
    const skill = fixtureSkill({ is_installed: true, installed_info: undefined });

    expect(resolveInstallScope(skill)).toEqual({ scope: "global", projectPath: null });
  });

  it("ignores a project deployment that has no project_path", () => {
    const skill = fixtureSkill({
      installed_info: fixtureInstalledSkill({
        deployments: [fixtureDeployment({ scope: "project", project_path: undefined })],
      }),
    });

    expect(resolveInstallScope(skill)).toEqual({ scope: "global", projectPath: null });
  });

  it("ignores non-project scopes (plugin, parked) even with a project_path set", () => {
    const skill = fixtureSkill({
      installed_info: fixtureInstalledSkill({
        deployments: [
          fixtureDeployment({ scope: "plugin", project_path: "/work/repo" }),
          fixtureDeployment({ scope: "parked", project_path: "/work/repo" }),
        ],
      }),
    });

    expect(resolveInstallScope(skill)).toEqual({ scope: "global", projectPath: null });
  });

  it("prefers the project deployment when both global and project deployments exist", () => {
    // A skill deployed globally AND in a project: the drawer's single remove
    // targets the project deployment (per-deployment buttons remain the real
    // fix for the genuinely ambiguous case, but project-wins avoids mutating
    // the global deployment silently).
    const skill = fixtureSkill({
      installed_info: fixtureInstalledSkill({
        deployments: [
          fixtureDeployment({ scope: "global", path: "/home/.agents/skills/find-bugs" }),
          fixtureDeployment({
            scope: "project",
            project_path: "/work/my-repo",
            path: "/work/my-repo/.claude/skills/find-bugs",
          }),
        ],
      }),
    });

    expect(resolveInstallScope(skill)).toEqual({ scope: "project", projectPath: "/work/my-repo" });
  });

  it("returns the first project deployment when several project locations exist", () => {
    const skill = fixtureSkill({
      installed_info: fixtureInstalledSkill({
        deployments: [
          fixtureDeployment({
            scope: "project",
            project_path: "/work/repo-a",
            path: "/work/repo-a/.claude/skills/find-bugs",
          }),
          fixtureDeployment({
            scope: "project",
            project_path: "/work/repo-b",
            path: "/work/repo-b/.claude/skills/find-bugs",
          }),
        ],
      }),
    });

    expect(resolveInstallScope(skill)).toEqual({ scope: "project", projectPath: "/work/repo-a" });
  });

  it("does not mutate the input skill's deployments", () => {
    const deployments = [fixtureDeployment({ scope: "project", project_path: "/work/repo" })];
    const skill = fixtureSkill({
      installed_info: fixtureInstalledSkill({ deployments }),
    });

    resolveInstallScope(skill);

    expect(skill.installed_info?.deployments).toBe(deployments);
    expect(skill.installed_info?.deployments[0].project_path).toBe("/work/repo");
  });
});
