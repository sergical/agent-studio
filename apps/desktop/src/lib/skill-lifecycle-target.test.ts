import { describe, expect, it } from "vitest";
import {
  lifecycleTargetForDeployment,
  lifecycleTargetForHarnessRoot,
  lifecycleTargetForSkill,
  lifecycleTargetForTrial,
  skillLifecycleScopeSelection,
  skillMutableLifecycleScopes,
  skillRemovalAvailability,
  skillRemovalDescription,
  skillRemovalPreview,
  skillUpdateOwnerTargets,
  updateSkillOwners,
} from "./skill-lifecycle-target";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";

function deployment(id: string, ownerId?: string, projectPath?: string): Deployment {
  return {
    id,
    destination: "universal",
    owner_kind: "skills-sh",
    owner_id: ownerId,
    mutability: "mutable",
    backing: { kind: "canonical" },
    agent: "Universal",
    scope: projectPath ? "project" : "global",
    path: `${projectPath ?? "/home"}/.agents/skills/x`,
    is_symlink: false,
    symlink_is_broken: false,
    project_path: projectPath,
    content_hash: "x",
    disabled: false,
  };
}

describe("lifecycleTargetForSkill", () => {
  it("targets the selected deployment instead of its aggregate name", () => {
    expect(lifecycleTargetForDeployment(deployment("selected", "owner:x"))).toEqual({
      deployment_id: "selected",
    });
  });

  it("does not mix same-name global and project owners", () => {
    const skill = {
      name: "x",
      source_kind: "skills-sh",
      deployments: [
        deployment("global", "owner:v1/global/x"),
        deployment("project", "owner:v1/project/p/x", "/p"),
      ],
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;
    expect(lifecycleTargetForSkill(skill, "project", "/p")).toEqual({
      owner_id: "owner:v1/project/p/x",
    });
  });

  it("rejects two owners in one scope", () => {
    const skill = {
      name: "x",
      source_kind: "skills-sh",
      deployments: [deployment("a", "owner:a"), deployment("b", "owner:b")],
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;
    expect(() => lifecycleTargetForSkill(skill, "global")).toThrow("multiple lifecycle owners");
  });

  it("targets the exact ownerless Universal Copy instead of its linked Claude deployment", () => {
    const canonical = {
      ...deployment("canonical"),
      owner_kind: "copy" as const,
    };
    const linked = {
      ...deployment("linked"),
      owner_kind: "copy" as const,
      agent: "Claude Code",
      backing: { kind: "linked-to", deployment_id: canonical.id } as const,
    };
    const skill = {
      name: "x",
      source_kind: "manual",
      deployments: [canonical, linked],
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

    expect(lifecycleTargetForSkill(skill, "global")).toEqual({ deployment_id: "canonical" });
  });

  it("disables aggregate removal for multiple independent Copy deployments", () => {
    const copy = (id: string, agent: string) => ({
      ...deployment(id),
      owner_kind: "copy" as const,
      destination: "per-harness" as const,
      agent,
      backing: { kind: "independent" } as const,
    });
    const skill = {
      name: "x",
      source_kind: "manual",
      deployments: [copy("claude", "Claude Code"), copy("codex", "Codex")],
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

    const availability = skillRemovalAvailability(skill, {
      skillName: "x",
      scope: "global",
      projectPath: null,
    });
    expect(availability.available).toBe(false);
    if (!availability.available) expect(availability.reason).toContain("Locations");
  });

  it("targets the deployment selected by a linked-root repair", () => {
    const linked = {
      ...deployment("claude-link", "owner:x"),
      agent: "Claude Code",
      path: "/home/.claude/skills/x",
      shared_via_whole_dir_link: true,
    };
    const skill = {
      name: "x",
      source_kind: "skills-sh",
      deployments: [deployment("universal", "owner:x"), linked],
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;
    expect(lifecycleTargetForHarnessRoot(skill, "claude-code", "/home/.claude/skills")).toEqual({
      deployment_id: "claude-link",
    });
  });

  it("targets the canonical deployment for the aggregate trial chip", () => {
    const linked = {
      ...deployment("linked", "owner:x"),
      backing: { kind: "linked-to", deployment_id: "canonical" } as const,
    };
    const skill = {
      name: "x",
      source_kind: "skills-sh",
      deployments: [linked, deployment("canonical", "owner:x")],
      trial: {
        deployment_id: "",
        expires_at: "2026-09-06T00:00:00Z",
        method: "skills-sh",
        scope: "global",
      },
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind" | "trial">;
    expect(lifecycleTargetForTrial(skill, skill.trial)).toEqual({ deployment_id: "canonical" });
  });

  it("targets simultaneous Global and Project trials by exact deployment id", () => {
    const skill = {
      name: "x",
      source_kind: "skills-sh",
      deployments: [
        deployment("global-copy", "owner:global"),
        deployment("project-copy", "owner:project", "/work/project"),
      ],
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;
    const globalTrial = {
      deployment_id: "global-copy",
      expires_at: "2026-09-06T00:00:00Z",
      method: "skills-sh" as const,
      scope: "global" as const,
    };
    const projectTrial = {
      deployment_id: "project-copy",
      expires_at: "2026-09-06T01:00:00Z",
      method: "skills-sh" as const,
      scope: "project" as const,
      project_path: "/work/project",
    };

    expect(lifecycleTargetForTrial(skill, globalTrial)).toEqual({ deployment_id: "global-copy" });
    expect(lifecycleTargetForTrial(skill, projectTrial)).toEqual({ deployment_id: "project-copy" });
  });

  it("selects the only mutable installed scope", () => {
    const skill = {
      name: "x",
      source_kind: "skills-sh",
      deployments: [deployment("project", "owner:project", "/work/project")],
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

    expect(skillLifecycleScopeSelection(skill)).toEqual({
      skillName: "x",
      scope: "project",
      projectPath: "/work/project",
    });
  });

  it("lists only mutable installed projects and replaces stale skill state", () => {
    const readOnly = {
      ...deployment("read-only", "owner:old", "/work/old"),
      mutability: "read-only" as const,
    };
    const skill = {
      name: "next",
      source_kind: "skills-sh",
      deployments: [
        readOnly,
        deployment("global", "owner:global"),
        deployment("project-b", "owner:b", "/work/b"),
        deployment("project-a", "owner:a", "/work/a"),
      ],
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

    expect(skillMutableLifecycleScopes(skill)).toEqual([
      { skillName: "next", scope: "global", projectPath: null },
      { skillName: "next", scope: "project", projectPath: "/work/a" },
      { skillName: "next", scope: "project", projectPath: "/work/b" },
    ]);
    expect(
      skillLifecycleScopeSelection(skill, {
        skillName: "previous",
        scope: "project",
        projectPath: "/work/old",
      }),
    ).toEqual({ skillName: "next", scope: "global", projectPath: null });
  });

  it("previews the selected owner group and only links backed by that group", () => {
    const canonical = deployment("canonical", "owner:selected");
    const linked = {
      ...deployment("linked", "owner:selected"),
      backing: { kind: "linked-to", deployment_id: "canonical" } as const,
      agent: "Claude Code",
    };
    const independent = {
      ...deployment("independent", "owner:other"),
      destination: "per-harness" as const,
      backing: { kind: "independent" } as const,
      mutability: "read-only" as const,
    };
    const skill = {
      name: "x",
      source_kind: "skills-sh",
      deployments: [canonical, linked, independent],
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

    const preview = skillRemovalPreview(skill, {
      skillName: "x",
      scope: "global",
      projectPath: null,
    });
    expect(preview.target).toEqual({ owner_id: "owner:selected" });
    expect(preview.managedDeployments.map(({ id }) => id)).toEqual(["canonical"]);
    expect(preview.linkedDeployments.map(({ id }) => id)).toEqual(["linked"]);
    expect(skillRemovalDescription(preview)).toBe(
      "This removes 1 managed deployment and 1 verified dependent link. Independent copies outside this group remain. This cannot be undone.",
    );
  });

  it("previews only the app-managed Claude link for a dotagents owner", () => {
    const canonical = {
      ...deployment("canonical", "owner:selected"),
      owner_kind: "dotagents" as const,
    };
    const claudeLink = {
      ...canonical,
      id: "claude-link",
      agent: "Claude Code",
      backing: { kind: "linked-to", deployment_id: canonical.id } as const,
    };
    const codexLink = {
      ...canonical,
      id: "codex-link",
      agent: "Codex",
      backing: { kind: "linked-to", deployment_id: canonical.id } as const,
    };
    const skill = {
      name: "x",
      source_kind: "dotagents",
      deployments: [canonical, claudeLink, codexLink],
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

    const preview = skillRemovalPreview(skill, {
      skillName: "x",
      scope: "global",
      projectPath: null,
    });

    expect(preview.linkedDeployments.map(({ id }) => id)).toEqual(["claude-link"]);
  });
});

describe("skill update owner targets", () => {
  it("keeps a project-only update on its exact owner", () => {
    expect(skillUpdateOwnerTargets({ update_owner_ids: ["owner:v1/project/%2Fp/x"] })).toEqual([
      { owner_id: "owner:v1/project/%2Fp/x" },
    ]);
  });

  it("updates mixed Global and Project owners and reports a partial failure", async () => {
    const seen: string[] = [];
    const summary = await updateSkillOwners(
      {
        update_owner_ids: ["owner:v1/global/x", "owner:v1/project/%2Fp/x"],
      },
      async (target) => {
        const ownerId = target.owner_id ?? "";
        seen.push(ownerId);
        return ownerId.includes("project")
          ? { success: false, error: "project update failed" }
          : { success: true };
      },
    );

    expect(seen).toEqual(["owner:v1/global/x", "owner:v1/project/%2Fp/x"]);
    expect(summary).toEqual({
      attempted: 2,
      succeeded: 1,
      failures: [{ ownerId: "owner:v1/project/%2Fp/x", message: "project update failed" }],
    });
  });
});
