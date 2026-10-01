import { describe, expect, it } from "vitest";
import {
  forkEditedAndUpdate,
  forkableDeployment,
  lifecycleTargetForDeployment,
  lifecycleTargetForHarnessRoot,
  lifecycleTargetForSkill,
  skillCanPark,
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
    path: `${projectPath ?? "/home"}/.agents/skills/x`,
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
