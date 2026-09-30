// ============================================================================
// Skill Studio - skill-bulk-run tests
// ============================================================================

import { describe, expect, it, vi } from "vitest";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import { universalDeployment } from "../../dev/harness/scanned-deployment";
import {
  bulkActionToast,
  bulkProgressLabel,
  bulkUpdateProgressLabel,
  planBulkAction,
} from "./skill-bulk-actions";
import { runBatchAction } from "./skill-bulk-run";
import type { BatchApi } from "./skill-bulk-run";

function globalFolder(name: string, fields: Partial<Deployment> = {}): Deployment {
  return universalDeployment(
    { universalPath: `/home/.agents/skills/${name}` },
    { owner_kind: "manual", mutability: "mutable", owner_id: `owner:v1/global/${name}`, ...fields },
  );
}

function fixtureSkill(name: string, fields: Partial<Deployment> = {}): InstalledSkill {
  return {
    name,
    source: "",
    source_type: "local",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "manual",
    deployments: [globalFolder(name, fields)],
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
  };
}

function fakeApi(overrides: Partial<BatchApi> = {}): BatchApi {
  return {
    parkSkills: vi.fn(async (targets) => targets.map(() => ({ error: null }))),
    unparkSkills: vi.fn(async (targets) => targets.map(() => ({ error: null }))),
    setSkillsInvocation: vi.fn(async (targets) => targets.map(() => ({ error: null }))),
    forkBeforeInvocationEdit: vi.fn(async () => {}),
    ...overrides,
  };
}

describe("runBatchAction", () => {
  it("sends one parkSkills call for the whole selection; fails if it still calls the backend once per skill", async () => {
    const skills = ["a", "b", "c"].map((name) => fixtureSkill(name));
    const api = fakeApi();

    const result = await runBatchAction({ kind: "park" }, skills, api);

    expect(api.parkSkills).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.parkSkills).mock.calls[0]?.[0]).toHaveLength(3);
    expect(result.succeeded).toEqual(skills);
    expect(result.failed).toEqual([]);
  });

  it("sends one unparkSkills call for the whole selection", async () => {
    const skills = ["a", "b"].map((name) => fixtureSkill(name));
    const api = fakeApi();

    await runBatchAction({ kind: "unpark" }, skills, api);

    expect(api.unparkSkills).toHaveBeenCalledTimes(1);
    expect(api.parkSkills).not.toHaveBeenCalled();
  });

  it("maps a failed park target to that skill and reports changed, skipped and failed in one toast", async () => {
    const parked = { ...fixtureSkill("done"), parked: true };
    const skills = [fixtureSkill("ok"), fixtureSkill("refused"), parked];
    const plan = planBulkAction(skills, { kind: "park" });
    const api = fakeApi({
      parkSkills: vi.fn(async () => [{ error: null }, { error: "Folder is busy" }]),
    });

    const result = await runBatchAction({ kind: "park" }, plan.applicable, api);
    const toast = bulkActionToast({ kind: "park" }, plan, result);

    expect(result.succeeded.map((skill) => skill.name)).toEqual(["ok"]);
    expect(result.failed).toEqual([{ skill: plan.applicable[1], error: "Folder is busy" }]);
    expect(toast.type).toBe("warning");
    expect(toast.title).toBe("Parked 1 of 3 skills · 1 already parked · 1 failed");
    expect(toast.message).toBe("refused: Folder is busy");
  });

  it("sends one setSkillsInvocation call with every editable file; fails if it writes skill by skill", async () => {
    const skills = ["a", "b", "c"].map((name) => fixtureSkill(name));
    const api = fakeApi();

    const result = await runBatchAction({ kind: "invocation", policy: "user-only" }, skills, api);

    expect(api.setSkillsInvocation).toHaveBeenCalledTimes(1);
    expect(api.setSkillsInvocation).toHaveBeenCalledWith(
      ["a", "b", "c"].map((name) => ({ name, path: `/home/.agents/skills/${name}/SKILL.md` })),
      "user-only",
    );
    expect(result.succeeded).toEqual(skills);
  });

  it("fails only the skill whose invocation target the backend refused", async () => {
    const skills = ["a", "b"].map((name) => fixtureSkill(name));
    const api = fakeApi({
      setSkillsInvocation: vi.fn(async () => [
        { error: "Invocation target is stale" },
        { error: null },
      ]),
    });

    const result = await runBatchAction({ kind: "invocation", policy: "both" }, skills, api);

    expect(result.failed).toEqual([{ skill: skills[0], error: "Invocation target is stale" }]);
    expect(result.succeeded).toEqual([skills[1]]);
  });

  it("keeps a skill whose fork failed out of the batch and reports the fork error", async () => {
    const skills = [fixtureSkill("a"), fixtureSkill("b")];
    const api = fakeApi({
      forkBeforeInvocationEdit: vi.fn(async (file) => {
        if (file.path.endsWith("/a")) throw new Error("Fork is not available");
      }),
    });

    const result = await runBatchAction({ kind: "invocation", policy: "both" }, skills, api);

    expect(api.setSkillsInvocation).toHaveBeenCalledWith(
      [{ name: "b", path: "/home/.agents/skills/b/SKILL.md" }],
      "both",
    );
    expect(result.failed).toEqual([{ skill: skills[0], error: "Fork is not available" }]);
    expect(result.succeeded).toEqual([skills[1]]);
  });
});

describe("bulkProgressLabel", () => {
  it("names the count without a per-item position for a batched action", () => {
    expect(bulkProgressLabel({ kind: "invocation", policy: "both" }, 1, 36)).toBe(
      "Setting invocation on 36 skills…",
    );
    expect(bulkProgressLabel({ kind: "park" }, 1, 1)).toBe("Parking 1 skill…");
    expect(bulkProgressLabel({ kind: "remove" }, 2, 5)).toBe("Removing 2 of 5…");
  });
});

describe("bulkUpdateProgressLabel", () => {
  it("counts finished update targets against the batch total; fails if the label stays a static skill count while the batch runs", () => {
    expect(bulkUpdateProgressLabel(12, 80)).toBe("Updating 12 of 80…");
  });
});
