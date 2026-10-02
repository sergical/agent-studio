import { describe, expect, it } from "vitest";
import {
  forkEditedAndUpdate,
  forkableDeployment,
  lifecycleTargetForDeployment,
  lifecycleTargetForHarnessRoot,
  lifecycleTargetForSkill,
  skillCanPark,
  skillDeploymentRemovalAvailability,
  skillParkVerb,
  skillLifecycleScopeSelection,
  skillMutableLifecycleScopes,
  skillRemovalAvailability,
  skillRemovalBlockedReason,
  skillRemovalChoices,
  skillRemovalEmptiesSkill,
  skillRemovalDescription,
  skillRemovalPreview,
  skillsWithLocalEdits,
  skillUpdateOwnerTargets,
  skillUpdateToast,
  updateSkillOwners,
} from "./skill-lifecycle-target";
import type { Deployment, ForkRecord, InstalledSkill, PullResult } from "@skill-studio/lib";

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
    path: `${projectPath ?? "/home/u"}/.agents/skills/x`,
    is_symlink: false,
    symlink_is_broken: false,
    project_path: projectPath,
    content_hash: "x",
    disabled: false,
    codex_implicit_invocation: null,
    disabled_by: null,
    invocation: "both",
    spec_violations: [],
    shared_via_whole_dir_link: false,
  };
}

function globalRemovalTarget(skill: Pick<InstalledSkill, "name" | "deployments" | "source_kind">) {
  return (
    skillRemovalChoices(skill).find((choice) => choice.selection.scope === "global")?.preview
      .target ?? null
  );
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
    expect(() => lifecycleTargetForSkill(skill, "global")).toThrow("more than one source");
  });

  describe("motion as the real scan reports it", () => {
    // Fixtures copy the fields `skill-studio scan --json` gives for the user's
    // `motion`: every folder is read-only because no ledger owns it.
    const home = "/Users/me";
    const repo = "/Users/me/src/repo-architect";
    function scanned(overrides: Partial<Deployment> & Pick<Deployment, "id" | "path">): Deployment {
      return {
        ...deployment(overrides.id),
        owner_kind: "manual",
        owner_id: null,
        mutability: "read-only",
        ...overrides,
      };
    }
    const projectFolder = scanned({
      id: "project-universal",
      path: `${repo}/.agents/skills/motion`,
      scope: "project",
      project_path: repo,
      owner_kind: "in-repo",
    });
    const projectLink = scanned({
      id: "project-claude",
      path: `${repo}/.claude/skills/motion`,
      agent: "Claude Code",
      scope: "project",
      project_path: repo,
      owner_kind: "in-repo",
      backing: { kind: "linked-to", deployment_id: "project-universal" },
      is_symlink: true,
      symlink_target: `${repo}/.agents/skills/motion`,
    });
    const globalFolder = scanned({ id: "global-universal", path: `${home}/.agents/skills/motion` });
    const globalWholeDirLink = scanned({
      id: "global-claude",
      path: `${home}/.claude/skills/motion`,
      agent: "Claude Code",
      backing: { kind: "linked-to", deployment_id: "global-universal" },
      shared_via_whole_dir_link: true,
      resolved_path: `${home}/.agents/skills/motion`,
    });

    it("explains that a repository copy is deleted in the repository, not by Skill Studio", () => {
      const skill = {
        name: "motion",
        source_kind: "manual",
        deployments: [projectFolder, projectLink],
      } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

      expect(() => lifecycleTargetForSkill(skill, "project", repo)).toThrow("part of a repository");
      expect(skillMutableLifecycleScopes(skill)).toEqual([]);
    });

    it("explains that a folder no ledger owns is deleted by hand, and says how", () => {
      const skill = {
        name: "motion",
        source_kind: "manual",
        deployments: [globalFolder, globalWholeDirLink],
      } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

      expect(() => lifecycleTargetForSkill(skill, "global")).toThrow(
        "did not install motion, so it will not delete it. Use Reveal in Finder",
      );
    });

    it("removes a copy Skill Studio owns with its links, and names the separate Claude folder that stays", () => {
      const universal = {
        ...globalFolder,
        owner_kind: "copy" as const,
        mutability: "mutable" as const,
      };
      const claudeFolder = {
        ...globalWholeDirLink,
        id: "global-claude-real",
        owner_kind: "copy" as const,
        mutability: "mutable" as const,
        backing: { kind: "independent" } as const,
        destination: "per-harness" as const,
        shared_via_whole_dir_link: false,
        resolved_path: null,
      };
      const skill = {
        name: "motion",
        source_kind: "manual",
        deployments: [universal, claudeFolder],
      } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

      const preview = skillRemovalPreview(skill, {
        skillName: "motion",
        scope: "global",
        projectPath: null,
      });
      expect(preview.target).toEqual({ deployment_id: "global-universal" });
      expect(preview.linkedDeployments).toEqual([]);
      expect(skillRemovalDescription(preview)).toBe(
        `This removes 1 folder and 0 links to it. The separate copy at ~/.claude/skills/motion stays. This cannot be undone.`,
      );
    });

    it("lists the whole-directory link as part of the folder, never as a link the backend deletes", () => {
      const universal = {
        ...globalFolder,
        owner_kind: "copy" as const,
        mutability: "mutable" as const,
      };
      const skill = {
        name: "motion",
        source_kind: "manual",
        deployments: [universal, { ...globalWholeDirLink, mutability: "mutable" as const }],
      } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

      const preview = skillRemovalPreview(skill, {
        skillName: "motion",
        scope: "global",
        projectPath: null,
      });
      expect(preview.linkedDeployments).toEqual([]);
      expect(preview.staying).toEqual([]);
    });

    it("never offers to remove a per-agent folder, which the backend refuses", () => {
      const perAgent = {
        ...globalFolder,
        id: "claude-folder",
        destination: "per-harness" as const,
        backing: { kind: "independent" } as const,
        owner_kind: "copy" as const,
        mutability: "mutable" as const,
      };
      const skill = {
        name: "motion",
        source_kind: "manual",
        deployments: [perAgent],
      } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;

      expect(() => lifecycleTargetForSkill(skill, "global")).toThrow("no single Universal folder");
      expect(skillDeploymentRemovalAvailability(skill, perAgent)).toMatchObject({
        available: false,
      });
    });
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
    if (!availability.available) expect(availability.reason).toContain("Reveal in Finder");
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
    const canonical = {
      ...deployment("canonical", "owner:selected"),
      owner_kind: "dotagents" as const,
    };
    const linked = {
      ...deployment("linked", "owner:selected"),
      backing: { kind: "linked-to", deployment_id: "canonical" } as const,
      agent: "Claude Code",
      is_symlink: true,
      symlink_target: canonical.path,
    };
    const unrelatedLink = {
      ...deployment("unrelated", "owner:selected"),
      backing: { kind: "linked-to", deployment_id: "canonical" } as const,
      agent: "Codex",
      is_symlink: true,
      symlink_target: "/elsewhere/x",
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
      deployments: [canonical, linked, unrelatedLink, independent],
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
      "This removes 1 folder and 1 link to it. The separate copies at ~/.agents/skills/x, ~/.agents/skills/x stay. This cannot be undone.",
    );
  });

  it("previews every per-skill link the backend deletes for a dotagents owner, not only Claude's", () => {
    const canonical = {
      ...deployment("canonical", "owner:selected"),
      owner_kind: "dotagents" as const,
    };
    const claudeLink = {
      ...canonical,
      id: "claude-link",
      agent: "Claude Code",
      backing: { kind: "linked-to", deployment_id: canonical.id } as const,
      is_symlink: true,
      symlink_target: canonical.path,
    };
    const codexLink = {
      ...canonical,
      id: "codex-link",
      agent: "Codex",
      backing: { kind: "linked-to", deployment_id: canonical.id } as const,
      is_symlink: true,
      symlink_target: canonical.path,
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

    expect(preview.linkedDeployments.map(({ id }) => id)).toEqual(["claude-link", "codex-link"]);
  });
});

describe("skill update owner targets", () => {
  it("skips an owner whose deployments are all read-only, because the backend refuses to update it; fails if Home offers a wildcard-dotagents owner", () => {
    // SAFETY: skillUpdateOwnerTargets reads only `owner_id` and `mutability`.
    const deployment = (owner_id: string, mutability: Deployment["mutability"]) =>
      ({ owner_id, mutability }) as Deployment;
    const update = (owner_id: string) => ({
      owner_id,
      latest_commit: "next",
      latest_commit_at: null,
    });

    expect(
      skillUpdateOwnerTargets({
        update_owner_ids: ["owner:v1/global/wild", "owner:v1/global/sh"],
        update_owners: [update("owner:v1/global/wild"), update("owner:v1/global/sh")],
        deployments: [
          deployment("owner:v1/global/wild", "read-only"),
          deployment("owner:v1/global/sh", "read-only"),
          deployment("owner:v1/global/sh", "mutable"),
        ],
      }),
    ).toEqual([{ owner_id: "owner:v1/global/sh" }]);
  });

  it("keeps a project-only update on its exact owner", () => {
    expect(
      skillUpdateOwnerTargets({
        update_owner_ids: ["owner:v1/project/%2Fp/x"],
        update_owners: [
          { owner_id: "owner:v1/project/%2Fp/x", latest_commit: "next", latest_commit_at: null },
        ],
      }),
    ).toEqual([{ owner_id: "owner:v1/project/%2Fp/x" }]);
  });

  it("updates mixed Global and Project owners and reports a partial failure", async () => {
    const seen: string[] = [];
    const summary = await updateSkillOwners(
      {
        update_owner_ids: ["owner:v1/global/x", "owner:v1/project/%2Fp/x"],
        update_owners: [
          { owner_id: "owner:v1/global/x", latest_commit: "next", latest_commit_at: null },
          { owner_id: "owner:v1/project/%2Fp/x", latest_commit: "next", latest_commit_at: null },
        ],
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

describe("skillUpdateToast", () => {
  const failure = { ownerId: "owner:v1/project/%2Fp/x", message: "project update failed" };

  it("names the skill alone when every copy updated, or names the count that leaked in", () => {
    expect(skillUpdateToast("find-bugs", { attempted: 2, succeeded: 2, failures: [] })).toEqual({
      type: "success",
      title: "Updated find-bugs",
    });
  });

  it("reports how many copies updated when only some did, or hides the failed ones", () => {
    expect(
      skillUpdateToast("find-bugs", { attempted: 2, succeeded: 1, failures: [failure] }),
    ).toEqual({
      type: "warning",
      title: "Updated 1 of 2 copies of find-bugs",
      message: "project update failed",
    });
  });

  it("reports an error with the failure text when no copy updated, or reads as a success", () => {
    expect(
      skillUpdateToast("find-bugs", { attempted: 1, succeeded: 0, failures: [failure] }),
    ).toEqual({
      type: "error",
      title: "Could not update find-bugs",
      message: "project update failed",
    });
  });
});

describe("skillRemovalChoices global target", () => {
  function skill(
    deployments: Deployment[],
    sourceKind: InstalledSkill["source_kind"] = "skills-sh",
  ) {
    return {
      name: "x",
      source_kind: sourceKind,
      deployments,
    } satisfies Pick<InstalledSkill, "name" | "deployments" | "source_kind">;
  }

  it("returns an exact global owner target", () => {
    expect(globalRemovalTarget(skill([deployment("global", "owner:v1/global/x")]))).toEqual({
      owner_id: "owner:v1/global/x",
    });
  });

  it("returns an exact app-managed Copy deployment even when the display source is manual", () => {
    const copy = {
      ...deployment("global-copy"),
      owner_kind: "copy" as const,
    };

    expect(globalRemovalTarget(skill([copy], "manual"))).toEqual({
      deployment_id: "global-copy",
    });
  });

  it("rejects project-only, plugin, parked, manual, ambiguous, and lock-only records", () => {
    const projectOnly = deployment("project", "owner:v1/project/%2Frepo/x", "/repo");
    const plugin = {
      ...deployment("plugin"),
      scope: "plugin" as const,
      owner_kind: "plugin" as const,
      mutability: "read-only" as const,
    };
    const parked = {
      ...deployment("parked", "owner:v1/global/x"),
      scope: "parked" as const,
    };
    const manual = {
      ...deployment("manual"),
      owner_kind: "manual" as const,
      mutability: "read-only" as const,
    };
    const ambiguous = {
      ...deployment("ambiguous"),
      owner_kind: "ambiguous" as const,
      mutability: "read-only" as const,
    };

    expect(globalRemovalTarget(skill([projectOnly]))).toBeNull();
    expect(globalRemovalTarget(skill([plugin], "plugin"))).toBeNull();
    expect(globalRemovalTarget(skill([parked]))).toBeNull();
    expect(globalRemovalTarget(skill([manual], "manual"))).toBeNull();
    expect(globalRemovalTarget(skill([ambiguous], "dotagents"))).toBeNull();
    expect(globalRemovalTarget(skill([]))).toBeNull();
  });

  it("rejects a global scope with multiple mutable owners", () => {
    expect(
      globalRemovalTarget(skill([deployment("one", "owner:one"), deployment("two", "owner:two")])),
    ).toBeNull();
  });
});

describe("skill page header removal and park choices", () => {
  const view = (deployments: Deployment[]) =>
    ({ name: "x", source_kind: "skills-sh", deployments }) satisfies Pick<
      InstalledSkill,
      "name" | "deployments" | "source_kind"
    >;
  const global = deployment("global", "owner:v1/global/x");
  const project = deployment("project", "owner:v1/project/remix/x", "/code/remix");
  const inRepo = {
    ...deployment("in-repo", undefined, "/code/remix"),
    owner_kind: "in-repo" as const,
    mutability: "read-only" as const,
  };

  it("labels Remove by scope so a project uninstall never reads as a global one", () => {
    const labels = (deployments: Deployment[]) =>
      skillRemovalChoices(view(deployments)).map((choice) => choice.label);

    expect(labels([global])).toEqual(["Remove"]);
    expect(labels([project])).toEqual(["Remove from remix"]);
    expect(labels([global, project])).toEqual(["Remove global install", "Remove from remix"]);
  });

  it("tells two projects with the same folder name apart, so Remove never targets the wrong one", () => {
    const clientA = deployment("a", "owner:v1/project/a/x", "/work/client-a/app");
    const clientB = deployment("b", "owner:v1/project/b/x", "/work/client-b/app");
    const choices = skillRemovalChoices(view([clientA, clientB]));

    expect(choices.map((choice) => choice.label)).toEqual([
      "Remove from client-a/app",
      "Remove from client-b/app",
    ]);
    expect(choices.map((choice) => choice.key)).toEqual([
      "project:/work/client-a/app",
      "project:/work/client-b/app",
    ]);
    expect(choices[0].confirmMessage).toContain("Project: /work/client-a/app");
    expect(choices.map((choice) => choice.preview.target)).toEqual([
      { owner_id: "owner:v1/project/a/x" },
      { owner_id: "owner:v1/project/b/x" },
    ]);
  });

  it("names the repository when an in-repo skill has nothing the app may delete", () => {
    expect(skillRemovalChoices(view([inRepo]))).toEqual([]);
    expect(skillRemovalBlockedReason(view([inRepo]))).toBe(
      "Part of the remix repository; delete it there",
    );
    expect(skillRemovalBlockedReason(view([global]))).toBeNull();
  });

  it("keeps the page open when a project removal leaves the global install behind", () => {
    const [globalChoice, projectChoice] = skillRemovalChoices(view([global, project]));

    expect(skillRemovalEmptiesSkill(view([global, project]), projectChoice.selection)).toBe(false);
    expect(skillRemovalEmptiesSkill(view([project]), projectChoice.selection)).toBe(true);
    expect(skillRemovalEmptiesSkill(view([global]), globalChoice.selection)).toBe(true);
  });

  it("offers Park only where the core's park can move a folder, so the button never errors", () => {
    const parked = { ...deployment("parked"), scope: "parked" as const };

    expect(skillCanPark(view([global]))).toBe(true);
    expect(skillCanPark(view([parked]))).toBe(true);
    expect(skillCanPark(view([project]))).toBe(false);
    expect(skillCanPark(view([inRepo]))).toBe(false);
  });

  it("the row menu offers no Park entry for a project-only skill and Park or Unpark for a Global one", () => {
    const menuView = (deployments: Deployment[], parked = false) => ({
      ...view(deployments),
      parked,
    });
    const parked = { ...deployment("parked"), scope: "parked" as const };

    expect(
      skillParkVerb(menuView([project])),
      "the row menu offers Park for a project-only skill, which ops::park refuses",
    ).toBeNull();
    expect(skillParkVerb(menuView([global]))).toBe("Park");
    expect(skillParkVerb(menuView([parked], true))).toBe("Unpark");
  });
});

describe("skillsWithLocalEdits", () => {
  const skill = (name: string, sourceKind: InstalledSkill["source_kind"] = "skills-sh") => ({
    name,
    source_kind: sourceKind,
    update_owner_ids: [`owner:v1/global/${name}`],
    update_owners: [
      { owner_id: `owner:v1/global/${name}`, latest_commit: "next", latest_commit_at: null },
    ],
  });

  it("only_a_checked_edited_skill_is_returned_or_a_clean_update_gets_a_dialog", async () => {
    const skills = [skill("clean"), skill("edited"), skill("unchecked")];
    const verdicts = [
      { edited: false, checked: true },
      { edited: true, checked: true },
      { edited: true, checked: false },
    ];
    const result = await skillsWithLocalEdits(skills, async () => verdicts);
    expect(result.map((s) => s.name)).toEqual(["edited"]);
  });

  it("a_fork_is_never_checked_or_it_would_warn_about_edits_that_are_the_point_of_a_fork", async () => {
    let asked = 0;
    const result = await skillsWithLocalEdits([skill("forked", "fork")], async () => {
      asked += 1;
      return [{ edited: true, checked: true }];
    });
    expect(asked).toBe(0);
    expect(result).toEqual([]);
  });

  it("a_failed_check_reads_as_no_edits_or_a_backend_error_blocks_every_update", async () => {
    const result = await skillsWithLocalEdits([skill("edited")], async () => {
      throw new Error("ipc down");
    });
    expect(result).toEqual([]);
  });
});

describe("forkEditedAndUpdate", () => {
  const globalOwner = "owner:v1/global/x";
  const projectOwner = "owner:v1/project/x";
  const skill = {
    name: "x",
    deployments: [
      deployment("dep:global", globalOwner),
      deployment("dep:project", projectOwner, "/p"),
    ],
    update_owner_ids: [globalOwner, projectOwner],
    update_owners: [
      { owner_id: globalOwner, latest_commit: "n", latest_commit_at: null },
      { owner_id: projectOwner, latest_commit: "n", latest_commit_at: null },
    ],
  };
  const pull: PullResult = {
    from_commit: "a",
    to_commit: "b",
    merged: [],
    conflicts: [],
    added: [],
    removed: [],
    unchanged: 0,
    message: null,
  };
  const record: ForkRecord = {
    deployment_id: "dep:forked",
    forked_at: "2026-10-01T00:00:00Z",
    origin_tool: "skills-sh",
    origin_source: "o/r",
    repo: "o/r",
    path: "x",
    declared_ref: null,
    base_commit: "a",
  };

  it("only_the_global_universal_skills_sh_folder_is_forkable_or_fork_is_offered_where_it_must_fail", () => {
    expect(forkableDeployment(skill)?.id).toBe("dep:global");
    expect(
      forkableDeployment({ deployments: [deployment("dep:project", projectOwner, "/p")] }),
    ).toBe(undefined);
  });

  it("updates_the_other_owner_normally_after_the_fork_or_the_project_copy_is_silently_dropped", async () => {
    const calls: string[] = [];
    const { others } = await forkEditedAndUpdate(skill, {
      fork: async (target) => {
        calls.push(`fork ${target.deployment_id}`);
        return record;
      },
      pullFork: async (target) => {
        calls.push(`pull ${target.deployment_id}`);
        return pull;
      },
      updateOwner: async (target) => {
        calls.push(`update ${target.owner_id}`);
        return { success: true };
      },
    });
    expect(calls).toEqual(["fork dep:global", "pull dep:forked", `update ${projectOwner}`]);
    expect(others.succeeded).toBe(1);
  });

  it("a_failed_pull_after_a_good_fork_says_the_edits_are_kept_or_it_reads_like_a_lost_fork", async () => {
    await expect(
      forkEditedAndUpdate(skill, {
        fork: async () => record,
        pullFork: async () => {
          throw new Error("network down");
        },
        updateOwner: async () => ({ success: true }),
      }),
    ).rejects.toThrow(/fork was made and your edits are kept.*network down/);
  });

  it("a_scoped_update_leaves_the_other_owners_alone_or_a_global_fork_updates_the_project", async () => {
    const updated: string[] = [];
    await forkEditedAndUpdate(
      skill,
      {
        fork: async () => record,
        pullFork: async () => pull,
        updateOwner: async (target) => {
          updated.push(target.owner_id ?? "");
          return { success: true };
        },
      },
      { updateOthers: false },
    );
    expect(updated).toEqual([]);
  });
});

describe("removing a skills.sh skill that has other real folders", () => {
  const universal = deployment("universal", "owner:v1/global/x");
  const scope = { skillName: "x", scope: "global", projectPath: null } as const;
  const projectScope = { skillName: "x", scope: "project", projectPath: "/home/u/repo" } as const;
  const realFolder = (
    agent: string,
    path: string,
    fields: Partial<Deployment> = {},
  ): Deployment => ({
    ...deployment(`${agent}-folder`),
    owner_id: null,
    owner_kind: "manual",
    mutability: "read-only",
    destination: "per-harness",
    backing: { kind: "independent" },
    agent,
    path,
    ...fields,
  });
  const link = (agent: string, path: string, target = universal.path): Deployment => ({
    ...deployment(`${agent}-link`, "owner:v1/global/x"),
    destination: "per-harness",
    backing: { kind: "linked-to", deployment_id: "universal" },
    agent,
    is_symlink: true,
    symlink_target: target,
    path,
  });
  const skillOf = (
    deployments: Deployment[],
    sourceKind: InstalledSkill["source_kind"] = "skills-sh",
  ) =>
    ({ name: "x", source_kind: sourceKind, deployments }) satisfies Pick<
      InstalledSkill,
      "name" | "deployments" | "source_kind"
    >;
  const projectUniversal = deployment(
    "project-universal",
    "owner:v1/project/repo/x",
    "/home/u/repo",
  );
  const projectFolder = (agent: string, path: string): Deployment =>
    realFolder(agent, path, { scope: "project", project_path: "/home/u/repo" });

  it("refuses Remove and names the folder with a home-relative path when a separate real folder sits at another agent, or Undo cannot restore it", () => {
    const availability = skillRemovalAvailability(
      skillOf([universal, realFolder("Cursor", "/home/u/.cursor/skills/x")]),
      scope,
    );
    expect(availability).toEqual({
      available: false,
      reason: expect.stringContaining("~/.cursor/skills/x"),
    });
  });

  it("allows Remove when the other agents only hold links or a whole-folder link to the Universal folder, or Remove is refused for a layout that loses nothing", () => {
    const wholeFolder = {
      ...realFolder("Claude Code", "/home/u/.claude/skills/x"),
      destination: "universal" as const,
      backing: { kind: "linked-to", deployment_id: "universal" } as const,
      shared_via_whole_dir_link: true,
      resolved_path: universal.path,
    };
    const availability = skillRemovalAvailability(
      skillOf([universal, link("Codex", "/home/u/.codex/skills/x"), wholeFolder]),
      scope,
    );
    expect(availability.available).toBe(true);
  });

  it("refuses Remove when a whole-folder link leads to a real folder outside the Universal folder, or a dotfiles copy is deleted", () => {
    const dotfiles = {
      ...realFolder("Claude Code", "/home/u/.claude/skills/x"),
      destination: "universal" as const,
      backing: { kind: "linked-to", deployment_id: "universal" } as const,
      shared_via_whole_dir_link: true,
      resolved_path: "/home/u/dotfiles/claude/skills/x",
    };
    expect(skillRemovalAvailability(skillOf([universal, dotfiles]), scope)).toEqual({
      available: false,
      reason: expect.stringContaining("~/.claude/skills/x"),
    });
  });

  it("allows a project Remove with a real folder at .codex/skills, which the CLI leaves alone, or the dialog refuses for nothing", () => {
    const availability = skillRemovalAvailability(
      skillOf([projectUniversal, projectFolder("Codex", "/home/u/repo/.codex/skills/x")]),
      projectScope,
    );
    expect(availability.available).toBe(true);
  });

  it("refuses a project Remove with a real folder at .claude/skills, or Undo cannot restore it", () => {
    const availability = skillRemovalAvailability(
      skillOf([projectUniversal, projectFolder("Claude Code", "/home/u/repo/.claude/skills/x")]),
      projectScope,
    );
    expect(availability).toEqual({
      available: false,
      reason: expect.stringContaining("~/repo/.claude/skills/x"),
    });
  });

  it("allows Remove with a real folder at OpenCode's singular skill folder, which the CLI never touches, or the dialog refuses for nothing", () => {
    const availability = skillRemovalAvailability(
      skillOf([universal, realFolder("OpenCode", "/home/u/.config/opencode/skill/x")]),
      scope,
    );
    expect(availability.available).toBe(true);
  });

  it("lists a link that points outside the Universal folder as its own removed link with the kept target named, or the dialog promises it survives or counts it as a link to the folder", () => {
    const stray = link("Cursor", "/home/u/.cursor/skills/x", "/home/u/dotfiles/x");
    const preview = skillRemovalPreview(skillOf([universal, stray]), scope);
    expect(preview.otherLinks.map(({ id }) => id)).toEqual(["Cursor-link"]);
    expect(preview.linkedDeployments).toEqual([]);
    expect(preview.staying).toEqual([]);
    expect(skillRemovalDescription(preview)).toBe(
      "This removes 1 folder and 0 links to it. It also deletes the link at ~/.cursor/skills/x. The folder it points to, ~/dotfiles/x, stays. Separate copies elsewhere stay. This cannot be undone.",
    );
  });

  it("allows Remove and lists the link when a symlink sits inside a whole-folder link to a dotfiles folder, or rm -rf of a link is refused as lost data", () => {
    const innerLink = {
      ...realFolder("Claude Code", "/home/u/.claude/skills/x"),
      destination: "universal" as const,
      backing: { kind: "linked-to", deployment_id: "universal" } as const,
      shared_via_whole_dir_link: true,
      is_symlink: true,
      symlink_target: "/home/u/elsewhere/x",
      resolved_path: "/home/u/elsewhere/x",
    };
    const availability = skillRemovalAvailability(skillOf([universal, innerLink]), scope);
    expect(availability.available).toBe(true);
    if (availability.available) {
      expect(availability.preview.otherLinks.map(({ path }) => path)).toEqual([
        "/home/u/.claude/skills/x",
      ]);
    }
  });

  it("keeps a separate folder in the stays list for a Copy owner, whose removal leaves it alone, or the dialog hides it", () => {
    const copy = { ...universal, owner_kind: "copy" as const, owner_id: null };
    const preview = skillRemovalPreview(
      skillOf([copy, realFolder("Cursor", "/home/u/.cursor/skills/x")], "manual"),
      scope,
    );
    expect(preview.staying.map(({ id }) => id)).toEqual(["Cursor-folder"]);
  });

  it("offers no Remove target for a per-agent Copy alone, or the backend refuses it", () => {
    const perAgentCopy = realFolder("Claude Code", "/home/u/.claude/skills/x", {
      owner_kind: "copy",
      mutability: "mutable",
    });
    expect(skillRemovalAvailability(skillOf([perAgentCopy], "manual"), scope).available).toBe(
      false,
    );
  });

  it("targets the universal Copy when a per-agent Copy sits beside it, or Remove reads as ambiguous", () => {
    const universalCopy = {
      ...universal,
      id: "universal-copy",
      owner_kind: "copy" as const,
      owner_id: null,
    };
    const perAgentCopy = realFolder("Claude Code", "/home/u/.claude/skills/x", {
      owner_kind: "copy",
      mutability: "mutable",
    });
    expect(globalRemovalTarget(skillOf([universalCopy, perAgentCopy], "manual"))).toEqual({
      deployment_id: "universal-copy",
    });
  });
});
