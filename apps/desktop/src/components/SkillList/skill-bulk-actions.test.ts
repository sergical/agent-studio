// ============================================================================
// Skill Studio - skill-bulk-actions tests
// ============================================================================

import { describe, expect, it, vi } from "vitest";
import type { Deployment, InstalledSkill, PluginInfo } from "@skill-studio/lib";
import { realCopyDeployment, universalDeployment } from "../../dev/harness/scanned-deployment";
import {
  bulkActionToast,
  bulkDisabledReason,
  bulkRemovalTargets,
  bulkUpdateTargets,
  bulkUpdateResult,
  planBulkAction,
  runBulkSequentially,
} from "./skill-bulk-actions";

function globalFolder(name: string, fields: Partial<Deployment> = {}): Deployment {
  return universalDeployment(
    { universalPath: `/home/.agents/skills/${name}` },
    { owner_kind: "manual", mutability: "mutable", owner_id: `owner:v1/global/${name}`, ...fields },
  );
}

function fixtureSkill(name: string, overrides: Partial<InstalledSkill> = {}): InstalledSkill {
  return {
    name,
    source: "",
    source_type: "local",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "manual",
    deployments: [globalFolder(name)],
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

function pluginSkill(name: string): InstalledSkill {
  return fixtureSkill(name, {
    source_kind: "plugin",
    deployments: [
      realCopyDeployment(
        { agent: "Claude Code", path: `/home/.claude/plugins/cache/p/${name}` },
        {
          owner_kind: "plugin",
          mutability: "read-only",
          owner_id: null,
          // SAFETY: the plugin-skill checks read only `plugin.name`; the other PluginInfo fields are irrelevant here.
          plugin: { name: "p" } as PluginInfo,
        },
      ),
    ],
  });
}

function updatable(name: string): InstalledSkill {
  return fixtureSkill(name, {
    source_kind: "skills-sh",
    update_owner_ids: [`owner:v1/global/${name}`],
    update_owners: [
      { owner_id: `owner:v1/global/${name}`, latest_commit: "abc", latest_commit_at: null },
    ],
    deployments: [globalFolder(name, { owner_kind: "skills-sh" })],
  });
}

const names = (skills: InstalledSkill[]) => skills.map((skill) => skill.name);

describe("planBulkAction", () => {
  const unparked = fixtureSkill("unparked");
  const parked = fixtureSkill("parked", { parked: true });
  const plugin = pluginSkill("from-plugin");

  it("splits a mixed selection into parkable and already-parked skills, and skips a plugin skill; fails if park ignores the parked flag or the missing folder", () => {
    const plan = planBulkAction([unparked, parked, plugin], { kind: "park" });
    expect(names(plan.applicable)).toEqual(["unparked"]);
    expect(plan.skipped.map((s) => [s.skill.name, s.reason])).toEqual([
      ["parked", "already parked"],
      ["from-plugin", "no Global Universal folder"],
    ]);
  });

  it("offers unpark only for parked skills; fails if unpark applies to a skill that is not parked", () => {
    const plan = planBulkAction([unparked, parked, plugin], { kind: "unpark" });
    expect(names(plan.applicable)).toEqual(["parked"]);
    expect(plan.skipped.map((s) => s.reason)).toEqual(["not parked", "no Global Universal folder"]);
  });

  it("skips a plugin skill for invocation because it has no editable file; fails if read-only files count as editable", () => {
    const plan = planBulkAction([unparked, plugin], { kind: "invocation", policy: "user-only" });
    expect(names(plan.applicable)).toEqual(["unparked"]);
    expect(names(plan.skipped.map((s) => s.skill))).toEqual(["from-plugin"]);
  });

  it("updates only skills whose owner reports a newer commit; fails if a current skill is selected for update", () => {
    const plan = planBulkAction([updatable("stale"), unparked, plugin], { kind: "update" });
    expect(names(plan.applicable)).toEqual(["stale"]);
    expect(names(plan.skipped.map((s) => s.skill))).toEqual(["unparked", "from-plugin"]);
  });

  it("removes skills with a mutable copy and skips a plugin skill; fails if a read-only plugin copy is offered for removal", () => {
    const plan = planBulkAction([unparked, plugin], { kind: "remove" });
    expect(names(plan.applicable)).toEqual(["unparked"]);
    expect(plan.skipped).toEqual([{ skill: plugin, reason: "no removable copy" }]);
  });

  it("skips a skill whose global scope has two lifecycle owners, because removal would be ambiguous; fails if the removal availability check is bypassed", () => {
    const ambiguous = fixtureSkill("ambiguous", {
      deployments: [
        globalFolder("ambiguous"),
        realCopyDeployment(
          { agent: "Claude Code", path: "/home/.claude/skills/ambiguous" },
          { owner_kind: "copy", mutability: "mutable", owner_id: "owner:v1/global/other" },
        ),
      ],
    });
    const plan = planBulkAction([ambiguous], { kind: "remove" });
    expect(plan.applicable).toEqual([]);
    expect(plan.skipped.map((s) => s.reason)).toEqual(["needs a specific copy"]);
  });

  it("gives a disabled tooltip that names the reasons when nothing applies; fails if an empty plan has no reason or a runnable one has", () => {
    const none = planBulkAction([parked, parked], { kind: "park" });
    expect(bulkDisabledReason({ kind: "park" }, none)).toBe("Nothing to park: 2 already parked");
    const some = planBulkAction([unparked], { kind: "park" });
    expect(bulkDisabledReason({ kind: "park" }, some)).toBeNull();
  });
});

describe("bulkActionToast", () => {
  const a = fixtureSkill("a");
  const b = fixtureSkill("b");
  const c = fixtureSkill("c", { parked: true });

  it("names the count when every skill changed; fails if the title drops the count", () => {
    const plan = { applicable: [a, b], skipped: [] };
    expect(bulkActionToast({ kind: "park" }, plan, { succeeded: [a, b], failed: [] })).toEqual({
      type: "success",
      title: "Parked 2 skills",
    });
  });

  it("reports changed of total with the skip reasons on a partial skip; fails if skipped skills are hidden", () => {
    const plan = { applicable: [a, b], skipped: [{ skill: c, reason: "already parked" }] };
    expect(bulkActionToast({ kind: "park" }, plan, { succeeded: [a, b], failed: [] })).toEqual({
      type: "success",
      title: "Parked 2 of 3 skills · 1 already parked",
    });
  });

  it("lists the failed skill with its error and downgrades the toast; fails if a failure is reported as success", () => {
    const plan = { applicable: [a, b], skipped: [] };
    const toast = bulkActionToast({ kind: "remove" }, plan, {
      succeeded: [a],
      failed: [{ skill: b, error: "lease held" }],
    });
    expect(toast).toEqual({
      type: "warning",
      title: "Removed 1 of 2 skills · 1 failed",
      message: "b: lease held",
    });
  });

  it("uses the error type when nothing changed; fails if a total failure still looks like a warning", () => {
    const plan = { applicable: [a], skipped: [] };
    const toast = bulkActionToast({ kind: "update" }, plan, {
      succeeded: [],
      failed: [{ skill: a, error: "boom" }],
    });
    expect(toast.type).toBe("error");
  });
});

describe("runBulkSequentially", () => {
  it("keeps going after one rejected call and reports it; fails if a rejection stops the rest or is lost", async () => {
    const skills = [fixtureSkill("a"), fixtureSkill("b"), fixtureSkill("c")];
    const run = vi.fn(async (skill: InstalledSkill) => {
      if (skill.name === "b") throw new Error("lease held");
    });
    const result = await runBulkSequentially(skills, run);
    expect(run).toHaveBeenCalledTimes(3);
    expect(names(result.succeeded)).toEqual(["a", "c"]);
    expect(result.failed.map((f) => [f.skill.name, f.error])).toEqual([["b", "lease held"]]);
  });

  it("never overlaps two calls; fails if the runner starts a skill before the previous one settled", async () => {
    let active = 0;
    let peak = 0;
    const run = async () => {
      active += 1;
      peak = Math.max(peak, active);
      await Promise.resolve();
      active -= 1;
    };
    await runBulkSequentially([fixtureSkill("a"), fixtureSkill("b")], run);
    expect(peak).toBe(1);
  });
});

describe("bulkUpdateResult", () => {
  it("marks a skill with an error entry as failed and the rest as updated; fails if batch errors are dropped", () => {
    const a = fixtureSkill("a");
    const b = fixtureSkill("b");
    const result = bulkUpdateResult([a, b], {
      items: [
        { skill: "a", outcome: {} },
        { skill: "b", outcome: null },
      ],
      errors: { b: "network" },
    });
    expect(names(result.succeeded)).toEqual(["a"]);
    expect(result.failed).toEqual([{ skill: b, error: "network" }]);
  });
});

function projectFolder(name: string, projectPath: string, fields: Partial<Deployment> = {}) {
  return globalFolder(name, {
    scope: "project",
    project_path: projectPath,
    path: `${projectPath}/.agents/skills/${name}`,
    owner_id: `owner:v1/project/${projectPath}/${name}`,
    ...fields,
  });
}

describe("bulk targets across locations", () => {
  const twoLocations = (name: string, kind: "manual" | "skills-sh" = "manual") =>
    fixtureSkill(name, {
      source_kind: kind === "manual" ? "manual" : "skills-sh",
      deployments: [
        globalFolder(name, { owner_kind: kind }),
        projectFolder(name, "/work/p", { owner_kind: kind }),
      ],
    });

  it("removes a skill installed globally and in a project from both locations; fails if bulk Remove targets only the first location and the row stays", () => {
    const targets = bulkRemovalTargets(twoLocations("both"));
    expect(targets.map((target) => target.owner_id)).toEqual([
      "owner:v1/global/both",
      "owner:v1/project//work/p/both",
    ]);
  });

  it("updates every location that reports a newer commit; fails if bulk Update targets only the first location", () => {
    const skill = twoLocations("both", "skills-sh");
    skill.update_owner_ids = skill.deployments.flatMap((d) => d.owner_id ?? []);
    skill.update_owners = skill.update_owner_ids.map((owner_id) => ({
      owner_id,
      latest_commit: "abc",
      latest_commit_at: null,
    }));
    expect(bulkUpdateTargets(skill).map((target) => target.owner_id)).toEqual(
      skill.update_owner_ids,
    );
  });

  it("reports a skill as failed when any of its location updates fails; fails if a later item hides an earlier failure", () => {
    const skill = twoLocations("both", "skills-sh");
    const result = bulkUpdateResult([skill], {
      items: [
        { skill: "both", outcome: null },
        { skill: "both", outcome: {} },
      ],
      errors: {},
    });
    expect(names(result.failed.map((failure) => failure.skill))).toEqual(["both"]);
    expect(result.succeeded).toEqual([]);
  });
});
