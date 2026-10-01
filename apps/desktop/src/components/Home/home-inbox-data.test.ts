// ============================================================================
// home-inbox-data.test - "Update all"'s batching (review round 1's B4 fix:
// one `update_all_skills` request for every outdated owner instead of one
// `updateSkill` round trip per skill).
// ============================================================================

import { describe, expect, it } from "vitest";
import { ownSkillsView } from "@skill-studio/lib";
import type {
  Deployment,
  ForkRecord,
  HealthIssue,
  InstalledSkill,
  UpdateAllItem,
  UpdateAllOutcome,
  UpdateOutcome,
} from "@skill-studio/lib";
import {
  homeRowState,
  issueDeploymentPath,
  updateAllFailureMessage,
  updateAllOutdatedSkills,
} from "./home-inbox-data";

function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    id: "dep:v1/global/universal/find-bugs",
    destination: "universal",
    owner_kind: "manual",
    mutability: "read-only",
    backing: { kind: "canonical" },
    agent: "shared",
    scope: "global",
    path: "/home/.agents/skills/find-bugs",
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
    codex_implicit_invocation: null,
    disabled_by: null,
    invocation: "both",
    spec_violations: [],
    shared_via_whole_dir_link: false,
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
    update_owners: [],
    update_owner_ids: [],
    description: null,
    fork: null,
    parked_at: null,
    skill_path: null,
    source_url: null,
    update_commit: null,
    update_commit_at: null,
    updated_at: null,
    ...overrides,
  };
}

describe("homeRowState", () => {
  it("a_warnings_group_row_for_a_skill_with_an_update_shows_the_warning_glyph_or_names_the_update_that_outranked_it", () => {
    const skill = fixtureSkill({ update_owner_ids: ["x"] });
    const issue: HealthIssue = {
      kind: "linked-root",
      skill,
      detail: "Claude Code reads the Universal folder through a root link",
      harness: "claude-code",
      harnessLabel: "Claude Code",
      root: "/home/.agents/skills",
    };
    const state = homeRowState("warn", skill, issue);
    expect(state?.kind).toBe("issue");
    expect(state?.level).toBe("warning");
  });

  it("an_updates_group_row_for_a_skill_with_an_update_and_a_blocking_violation_shows_the_update_glyph_or_names_the_violation_that_would_outrank_it", () => {
    // `rowState`'s ladder puts a blocking spec violation ahead of "update available"; the
    // Updates group still shows "update" for this skill, since `updateRowState` only reads
    // `update_owner_ids` - unlike `rowState`, it never runs the violation rung.
    const skill = fixtureSkill({
      update_owner_ids: ["x"],
      spec_violations: ["invalid YAML frontmatter at line 3, column 1: mapping values not allowed"],
    });
    const state = homeRowState("upd", skill, null);
    expect(state?.kind).toBe("update");
  });

  it("an_unused_group_row_for_a_skill_with_a_spec_violation_shows_the_ladder_glyph_or_names_the_missing_state", () => {
    const skill = fixtureSkill({
      spec_violations: ["invalid YAML frontmatter at line 3, column 1: mapping values not allowed"],
    });
    const state = homeRowState("unused", skill, null);
    expect(state?.kind).toBe("violation");
  });
});

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
    expect(tally).toEqual({
      attempted: 3,
      succeeded: 3,
      failures: 0,
      skillsAttempted: 3,
      skillsSucceeded: 3,
      firstError: null,
    });
  });

  it("update_all_pulls_a_fork_upstream_separately_from_the_batched_owner_call_or_names_the_extra_call", async () => {
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
    expect(tally).toEqual({
      attempted: 2,
      succeeded: 2,
      failures: 0,
      skillsAttempted: 2,
      skillsSucceeded: 2,
      firstError: null,
    });
  });

  it("update_all_counts_every_owner_update_all_skills_reports_as_failed_or_names_the_uncounted_owner", async () => {
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

    expect(tally).toEqual({
      attempted: 2,
      succeeded: 0,
      failures: 2,
      skillsAttempted: 2,
      skillsSucceeded: 0,
      firstError: "update failed",
    });
  });

  it("update_all_summary_counts_each_failed_owner_or_names_the_overcounted_success", async () => {
    // "alpha" is installed twice (two owners, e.g. a skills-sh copy and a
    // dotagents copy) and both updates fail. `errors` is keyed by skill
    // name, so it collapses the two failures to one key - counting
    // failures from `Object.keys(errors).length` reports 1 failure and
    // credits the other owner as succeeded even though neither did.
    const skills = [
      ownerSkill("alpha", "owner:v1/global/alpha-skills-sh"),
      ownerSkill("alpha", "owner:v1/global/alpha-dotagents"),
    ];

    const tally = await updateAllOutdatedSkills(
      skills,
      async () => {
        throw new Error("no forks in this batch");
      },
      // The real backend resolves each target's skill name server-side;
      // both owners here are "alpha", so `failAll`'s `errors` (keyed by
      // name) collapses to one key while `items` keeps both entries -
      // same as production.
      async (targets) => failAll(targets.map(() => "alpha")),
    );

    expect(tally).toEqual({
      attempted: 2,
      succeeded: 0,
      failures: 2,
      skillsAttempted: 1,
      skillsSucceeded: 0,
      firstError: "update failed",
    });
  });

  it("update_all_reports_the_first_item_error_and_counts_each_failed_item_or_names_the_lost_reason", async () => {
    const skills = [
      ownerSkill("alpha", "owner:v1/global/alpha"),
      ownerSkill("beta", "owner:v1/global/beta"),
      ownerSkill("gamma", "owner:v1/global/gamma"),
    ];

    const tally = await updateAllOutdatedSkills(
      skills,
      async () => {
        throw new Error("no forks in this batch");
      },
      async () => ({
        items: [
          { skill: "alpha", outcome: outcomeFor("alpha") },
          { skill: "beta", outcome: null },
          { skill: "gamma", outcome: null },
        ],
        errors: { beta: "beta is wildcard-dotagents (read-only)", gamma: "gamma failed" },
      }),
    );

    expect(tally).toEqual({
      attempted: 3,
      succeeded: 1,
      failures: 2,
      skillsAttempted: 3,
      skillsSucceeded: 1,
      firstError: "beta is wildcard-dotagents (read-only)",
    });
  });

  it("update_all_counts_every_target_failed_and_keeps_the_error_text_when_the_call_rejects_or_drops_the_ipc_error", async () => {
    const skills = [
      ownerSkill("alpha", "owner:v1/global/alpha"),
      ownerSkill("beta", "owner:v1/global/beta"),
    ];

    const tally = await updateAllOutdatedSkills(
      skills,
      async () => {
        throw new Error("no forks in this batch");
      },
      async () => {
        throw new Error("backend unreachable");
      },
    );

    expect(tally).toEqual({
      attempted: 2,
      succeeded: 0,
      failures: 2,
      skillsAttempted: 2,
      skillsSucceeded: 0,
      firstError: "backend unreachable",
    });
  });

  it("update_all_progress_counts_forks_then_owner_targets_toward_one_total_or_names_the_skipped_step", async () => {
    const seen: [number, number][] = [];

    await updateAllOutdatedSkills(
      [forkSkill, ownerSkill("alpha", "owner:v1/global/alpha")],
      async () => ({
        from_commit: "aaa",
        to_commit: "bbb",
        merged: [],
        conflicts: [],
        added: [],
        removed: [],
        unchanged: 0,
        message: null,
      }),
      async (targets, onOwnerDone) => {
        onOwnerDone(1);
        return succeedAll(targets.map((target) => target.owner_id ?? ""));
      },
      (done, total) => seen.push([done, total]),
    );

    expect(seen).toEqual([
      [0, 2],
      [1, 2],
      [2, 2],
    ]);
  });
});

describe("updateAllFailureMessage", () => {
  it("update_all_toast_names_the_first_error_and_truncates_a_long_one_or_hides_why_it_failed", () => {
    const tally = {
      attempted: 5,
      succeeded: 2,
      failures: 3,
      skillsAttempted: 5,
      skillsSucceeded: 2,
      firstError: "read-only",
    };
    expect(updateAllFailureMessage(tally)).toBe("3 failed: read-only");
    expect(updateAllFailureMessage({ ...tally, failures: 0, firstError: null })).toBeUndefined();
    const long = updateAllFailureMessage({ ...tally, firstError: "x".repeat(500) }) ?? "";
    expect(long.length).toBeLessThan(170);
    expect(long.endsWith("…")).toBe(true);
  });

  it("update_all_toast_counts_skills_not_copies_so_it_agrees_with_its_title", () => {
    const base = { attempted: 2, firstError: "read-only" };
    // One skill, both copies failed: title "Updated 0 of 1 skill", so one failed.
    expect(
      updateAllFailureMessage({
        ...base,
        succeeded: 0,
        failures: 2,
        skillsAttempted: 1,
        skillsSucceeded: 0,
      }),
    ).toBe("1 failed: read-only");
    // Three copies failed across two skills, one skill fully updated: one skill failed, not three.
    expect(
      updateAllFailureMessage({
        ...base,
        attempted: 4,
        succeeded: 1,
        failures: 3,
        skillsAttempted: 2,
        skillsSucceeded: 1,
      }),
    ).toBe("1 failed: read-only");
  });
});

describe("issueDeploymentPath", () => {
  const ERROR = 'name "Find Bugs" is not a valid skill name';

  it("a_spec_violation_issue_opens_the_warned_copy_instead_of_the_clean_first_copy_or_the_warning_stays_hidden", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ path: "/a/clean" }),
        fixtureDeployment({ path: "/b/warned", spec_violations: [ERROR] }),
      ],
    });
    const issue: HealthIssue = { kind: "spec-violation", skill, detail: ERROR };
    expect(issueDeploymentPath(issue)).toBe("/b/warned");
  });

  it("a_non_spec_issue_names_no_copy_so_the_page_opens_its_default", () => {
    const skill = fixtureSkill({
      deployments: [fixtureDeployment({ path: "/b/warned", spec_violations: [ERROR] })],
    });
    const issue: HealthIssue = { kind: "broken-symlink", skill, detail: "broken" };
    expect(issueDeploymentPath(issue)).toBeUndefined();
  });

  it("a_skill_also_shipped_by_a_plugin_opens_the_warned_own_copy_not_the_plugin_copy_in_the_own_view", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ path: "/own/warned", spec_violations: [ERROR] }),
        fixtureDeployment({
          path: "/plugin/errored",
          spec_violations: [ERROR],
          plugin: { name: "p", version: null, harness: "Claude Code", marketplace: "m", id: "p@m" },
        }),
      ],
      spec_violations: [ERROR],
    });
    const [own] = ownSkillsView([skill]);
    const issue: HealthIssue = { kind: "spec-violation", skill: own, detail: ERROR };
    expect(issueDeploymentPath(issue)).toBe("/own/warned");
  });
});

describe("updateAllOutdatedSkills with edited skills", () => {
  const edited = {
    ...ownerSkill("edited", "owner:v1/global/edited"),
    deployments: [
      { ...canonicalDeployment, id: "dep:v1/edited", owner_kind: "skills-sh" as const },
    ],
  };
  const plain = ownerSkill("plain", "owner:v1/global/plain");

  it("update_all_forks_then_pulls_an_edited_skill_and_updates_the_rest_or_overwrites_the_edit", async () => {
    const calls: string[] = [];
    const tally = await updateAllOutdatedSkills(
      [edited, plain],
      async (target) => {
        calls.push(`pull ${target.deployment_id}`);
        return {
          from_commit: "a",
          to_commit: "b",
          merged: [],
          added: [],
          removed: [],
          conflicts: [],
          unchanged: 0,
          message: null,
        };
      },
      async (targets) => {
        const owners = targets.map((target) => target.owner_id ?? "");
        calls.push(`update ${owners.join(",")}`);
        return succeedAll(owners);
      },
      undefined,
      {
        names: new Set(["edited"]),
        fork: async (target) => {
          calls.push(`fork ${target.deployment_id}`);
          const record: ForkRecord = {
            deployment_id: "dep:v1/forked",
            forked_at: "2026-10-01T00:00:00Z",
            origin_tool: "skills-sh",
            origin_source: "owner/repo",
            repo: "owner/repo",
            path: "skills/edited",
            declared_ref: null,
            base_commit: "a",
          };
          return record;
        },
      },
    );

    expect(calls).toEqual([
      "fork dep:v1/edited",
      "pull dep:v1/forked",
      "update owner:v1/global/plain",
    ]);
    expect(tally.skillsAttempted).toBe(2);
    expect(tally.skillsSucceeded).toBe(2);
  });

  it("update_all_counts_an_edited_skill_whose_fork_fails_as_failed_and_never_updates_it_or_loses_the_edit", async () => {
    const tally = await updateAllOutdatedSkills(
      [edited],
      async () => {
        throw new Error("pull must not run after a failed fork");
      },
      async () => {
        throw new Error("the edited skill must not be overwritten");
      },
      undefined,
      {
        names: new Set(["edited"]),
        fork: async () => {
          throw new Error("fork refused");
        },
      },
    );

    expect(tally.failures).toBe(1);
    expect(tally.skillsSucceeded).toBe(0);
    expect(tally.firstError).toBe("fork refused");
  });
});
