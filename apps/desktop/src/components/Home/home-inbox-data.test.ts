// ============================================================================
// home-inbox-data.test - "Update all"'s batching (review round 1's B4 fix:
// one `update_all_skills` request for every outdated owner instead of one
// `updateSkill` round trip per skill).
// ============================================================================

import { describe, expect, it } from "vitest";
import type { Deployment, UpdateAllItem, UpdateAllOutcome, UpdateOutcome } from "@skill-studio/lib";
import { updateAllOutdatedSkills } from "./home-inbox-data";

const noDeployments: Deployment[] = [];

function ownerSkill(name: string, ownerId: string) {
  return {
    name,
    source_kind: "skills-sh" as const,
    update_owner_ids: [ownerId],
    update_owners: [{ owner_id: ownerId, latest_commit: "next", latest_commit_at: null }],
    deployments: noDeployments,
  };
}

const canonicalDeployment: Deployment = {
  id: "dep:v1/global/1/universal/forked/-/1",
  destination: "universal",
  owner_kind: "fork",
  owner_id: null,
  mutability: "mutable",
  backing: { kind: "canonical" },
  agent: "shared",
  scope: "global",
  path: "/home/.agents/skills/forked",
  is_symlink: false,
  symlink_is_broken: false,
  content_hash: "x",
  disabled: false,
  codex_implicit_invocation: null,
  disabled_by: null,
  invocation: "both",
  spec_violations: [],
  shared_via_whole_dir_link: false,
};

const forkSkill = {
  name: "forked",
  source_kind: "fork" as const,
  update_owner_ids: [],
  update_owners: [],
  deployments: [canonicalDeployment],
};

function outcomeFor(target: string): UpdateOutcome {
  return {
    event_id: `evt-${target}`,
    skill: target,
    deployment_path: "/home/.agents/skills/x",
    tree_hash_before: "aaa",
    tree_hash_after: "bbb",
  };
}

function succeedAll(items: UpdateAllItem["skill"][]): UpdateAllOutcome {
  return {
    items: items.map((skill) => ({ skill, outcome: outcomeFor(skill) })),
    errors: {},
  };
}

function failAll(items: UpdateAllItem["skill"][]): UpdateAllOutcome {
  return {
    items: items.map((skill) => ({ skill, outcome: null })),
    errors: Object.fromEntries(items.map((skill) => [skill, "update failed"])),
  };
}

describe("updateAllOutdatedSkills", () => {
  it("update_all_sends_one_request_for_every_outdated_owner_or_names_the_extra_call", async () => {
    const skills = [
      ownerSkill("alpha", "owner:v1/global/alpha"),
      ownerSkill("beta", "owner:v1/global/beta"),
      ownerSkill("gamma", "owner:v1/global/gamma"),
    ];
    let updateAllCalls = 0;
    let pullForkCalls = 0;
    const seenTargets: string[] = [];

    const tally = await updateAllOutdatedSkills(
      skills,
      async () => {
        pullForkCalls += 1;
        throw new Error("no forks in this batch");
      },
      async (targets) => {
        updateAllCalls += 1;
        const owners = targets.map((target) => target.owner_id ?? "");
        seenTargets.push(...owners);
        return succeedAll(owners);
      },
    );

    expect(updateAllCalls).toBe(1);
    expect(pullForkCalls).toBe(0);
    expect(seenTargets).toEqual([
      "owner:v1/global/alpha",
      "owner:v1/global/beta",
      "owner:v1/global/gamma",
    ]);
    expect(tally).toEqual({ attempted: 3, succeeded: 3, failures: 0 });
  });

  it("pulls a fork upstream separately from the batched owner call", async () => {
    const owner = ownerSkill("alpha", "owner:v1/global/alpha");

    let pullForkCalls = 0;
    let updateAllCalls = 0;

    const tally = await updateAllOutdatedSkills(
      [forkSkill, owner],
      async () => {
        pullForkCalls += 1;
        return {
          from_commit: "aaa",
          to_commit: "bbb",
          merged: [],
          conflicts: [],
          added: [],
          removed: [],
          unchanged: 0,
          message: null,
        };
      },
      async (targets) => {
        updateAllCalls += 1;
        return succeedAll(targets.map((target) => target.owner_id ?? ""));
      },
    );

    expect(pullForkCalls).toBe(1);
    expect(updateAllCalls).toBe(1);
    expect(tally).toEqual({ attempted: 2, succeeded: 2, failures: 0 });
  });

  it("counts every owner update_all_skills reports as failed", async () => {
    const skills = [
      ownerSkill("alpha", "owner:v1/global/alpha"),
      ownerSkill("beta", "owner:v1/global/beta"),
    ];

    const tally = await updateAllOutdatedSkills(
      skills,
      async () => {
        throw new Error("no forks in this batch");
      },
      async (targets) => failAll(targets.map((target) => target.owner_id ?? "")),
    );

    expect(tally).toEqual({ attempted: 2, succeeded: 0, failures: 2 });
  });
});
