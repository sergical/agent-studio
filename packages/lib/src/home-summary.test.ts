// ============================================================================
// Skill Studio - home-summary tests
// ============================================================================

import { describe, expect, it } from "vitest";
import {
  attentionGroups,
  homeInvocationCounts,
  homePromptCost,
  homeSummaryCounts,
  recentlyUsedSkills,
  unusedSkills,
} from "./home-summary";
import type { HealthIssue } from "./skill-health";
import type {
  Deployment,
  InstalledSkill,
  SkillInvocationStats,
  SkillSnapshot,
} from "./skill-types";

function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    agent: "Claude Code",
    scope: "global",
    path: "/home/.claude/skills/find-bugs",
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
    has_spec: false,
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
    update_owner_ids: overrides.update_owner_ids ?? [],
  };
}

function fixtureStats(overrides: Partial<SkillInvocationStats> = {}): SkillInvocationStats {
  return {
    skill: "find-bugs",
    total: 0,
    last_24_hours: 0,
    last_7_days: 0,
    last_14_days: 0,
    last_30_days: 0,
    by_project_30_days: {},
    by_day: {},
    ...overrides,
  };
}

function fixtureSnapshot(overrides: Partial<SkillSnapshot> = {}): SkillSnapshot {
  return {
    skills: [],
    projects: [],
    invocations: [],
    heatmap: { days: {} },
    scanned_at: "2026-01-01T00:00:00Z",
    last_test_by_skill: {},
    update_check: { checked_at: null, gh_status: "ok", message: null, updates_available: 0 },
    ...overrides,
  };
}

describe("homeSummaryCounts", () => {
  it("counts own skills, first-class harnesses present, tracked projects, and 30-day uses", () => {
    const skills = [
      fixtureSkill({ name: "a", deployments: [fixtureDeployment({ agent: "Claude Code" })] }),
      fixtureSkill({ name: "b", deployments: [fixtureDeployment({ agent: "Codex" })] }),
    ];
    const snapshot = fixtureSnapshot({
      skills,
      projects: ["/repo-a", "/repo-b", "/repo-c"],
      invocations: [
        fixtureStats({ skill: "a", last_30_days: 10 }),
        fixtureStats({ skill: "b", last_30_days: 5 }),
      ],
    });
    expect(homeSummaryCounts(snapshot)).toEqual({
      skillCount: 2,
      pluginSkillCount: 0,
      harnessCount: 2,
      projectCount: 3,
      usesIn30Days: 15,
    });
  });

  it("excludes plugin-only skills and non-first-class agents from the harness count", () => {
    const skills = [
      fixtureSkill({
        name: "plugin-only",
        deployments: [
          fixtureDeployment({
            agent: "Claude Code",
            plugin: { name: "acme", harness: "claude-code" },
          }),
        ],
      }),
      fixtureSkill({
        name: "own",
        deployments: [fixtureDeployment({ agent: "Amp" })],
      }),
    ];
    const snapshot = fixtureSnapshot({ skills });
    const counts = homeSummaryCounts(snapshot);
    expect(counts.skillCount).toBe(1);
    expect(counts.harnessCount).toBe(0);
  });

  it("counts plugin-shipped skills separately, zero when there are none", () => {
    const skills = [
      fixtureSkill({ name: "own" }),
      fixtureSkill({
        name: "plugin-only",
        deployments: [
          fixtureDeployment({
            agent: "Claude Code",
            plugin: { name: "acme", harness: "claude-code" },
          }),
        ],
      }),
    ];
    expect(homeSummaryCounts(fixtureSnapshot({ skills: [skills[0]] })).pluginSkillCount).toBe(0);
    expect(homeSummaryCounts(fixtureSnapshot({ skills })).pluginSkillCount).toBe(1);
  });
});

describe("recentlyUsedSkills", () => {
  it("returns the n skills with the most recent invocation, newest first", () => {
    const skills = [
      fixtureSkill({ name: "a" }),
      fixtureSkill({ name: "b" }),
      fixtureSkill({ name: "c" }),
    ];
    const stats = [
      fixtureStats({ skill: "a", last_used: "2026-01-01T00:00:00Z" }),
      fixtureStats({ skill: "b", last_used: "2026-01-03T00:00:00Z" }),
      fixtureStats({ skill: "c", last_used: "2026-01-02T00:00:00Z" }),
    ];
    const result = recentlyUsedSkills(skills, stats, 2);
    expect(result.map((r) => r.skill.name)).toEqual(["b", "c"]);
  });

  it("excludes skills with no recorded invocation", () => {
    const skills = [fixtureSkill({ name: "a" })];
    const stats = [fixtureStats({ skill: "a" })];
    expect(recentlyUsedSkills(skills, stats, 5)).toEqual([]);
  });

  it("excludes stats for skills no longer installed", () => {
    const stats = [fixtureStats({ skill: "gone", last_used: "2026-01-01T00:00:00Z" })];
    expect(recentlyUsedSkills([], stats, 5)).toEqual([]);
  });

  it("carries the busiest 30-day project's basename as projectLabel", () => {
    const skills = [fixtureSkill({ name: "a" })];
    const stats = [
      fixtureStats({
        skill: "a",
        last_used: "2026-01-01T00:00:00Z",
        last_30_days: 7,
        by_project_30_days: { "/home/user/repo-a": 2, "/home/user/repo-b": 5 },
      }),
    ];
    const result = recentlyUsedSkills(skills, stats, 5);
    expect(result[0].projectLabel).toBe("repo-b");
    expect(result[0].usesIn30Days).toBe(7);
  });
});

describe("homeInvocationCounts", () => {
  it("counts own skills by invocation policy", () => {
    const skills = [
      fixtureSkill({ name: "a", invocation: "both" }),
      fixtureSkill({ name: "b", invocation: "model-only" }),
      fixtureSkill({ name: "c", invocation: "user-only" }),
      fixtureSkill({ name: "d", invocation: "both" }),
    ];
    expect(homeInvocationCounts(skills)).toEqual({ both: 2, modelOnly: 1, userOnly: 1 });
  });

  it("excludes plugin-only skills", () => {
    const skills = [
      fixtureSkill({
        name: "plugin-only",
        invocation: "both",
        deployments: [
          fixtureDeployment({
            agent: "Claude Code",
            plugin: { name: "acme", harness: "claude-code" },
          }),
        ],
      }),
    ];
    expect(homeInvocationCounts(skills)).toEqual({ both: 0, modelOnly: 0, userOnly: 0 });
  });
});

describe("homePromptCost", () => {
  it("sums description_tokens for model-invocable skills, split by 30-day use", () => {
    const skills = [
      fixtureSkill({ name: "used", invocation: "both", description_tokens: 10 }),
      fixtureSkill({ name: "idle", invocation: "model-only", description_tokens: 20 }),
      fixtureSkill({ name: "user-only", invocation: "user-only", description_tokens: 100 }),
    ];
    const invocations = [fixtureStats({ skill: "used", last_30_days: 3 })];
    expect(homePromptCost(skills, invocations)).toEqual({
      totalTokens: 30,
      usedTokens: 10,
      usedCount: 1,
      idleTokens: 20,
      idleCount: 1,
    });
  });
});

describe("unusedSkills", () => {
  it("returns own skills with no 30-day use, model-invocable first then alphabetical", () => {
    const skills = [
      fixtureSkill({ name: "zebra", invocation: "user-only" }),
      fixtureSkill({ name: "apple", invocation: "model-only" }),
      fixtureSkill({ name: "mango", invocation: "both" }),
      fixtureSkill({ name: "used-up", invocation: "both" }),
    ];
    const invocations = [fixtureStats({ skill: "used-up", last_30_days: 4 })];
    const result = unusedSkills(skills, invocations);
    expect(result.map((s) => s.name)).toEqual(["apple", "mango", "zebra"]);
  });
});

describe("attentionGroups", () => {
  it("splits issues into broken (error severity) and warnings", () => {
    const skill = fixtureSkill();
    const issues: HealthIssue[] = [
      { kind: "broken-symlink", skill, detail: "target missing" },
      { kind: "lock-only", skill, detail: "no deployment" },
    ];
    const groups = attentionGroups(issues);
    expect(groups.broken.map((i) => i.kind)).toEqual(["broken-symlink"]);
    expect(groups.warnings.map((i) => i.kind)).toEqual(["lock-only"]);
  });
});
