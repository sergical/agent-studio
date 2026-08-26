// ============================================================================
// Skill Studio - home-summary tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { homeSummaryCounts, recentlyUsedSkills } from "./home-summary";
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
