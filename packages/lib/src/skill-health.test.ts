// ============================================================================
// Skill Studio - skill-health tests
// ============================================================================

import { describe, expect, it } from "vitest";
import {
  coverageGaps,
  deploymentWithSpecViolations,
  findDuplicateSkills,
  findLinkedRootIssues,
  findParkedButReinstalled,
  findSpecViolations,
  HEALTH_ISSUE_KIND_ORDER,
  isBlockingSpecViolation,
} from "./skill-health";
import type { Deployment, InstalledSkill } from "./skill-types";

/** Minimal `Deployment` fixture, overridable per test. */
function fixtureDeployment(overrides: Partial<Deployment> = {}): Deployment {
  return {
    agent: "shared",
    scope: "global",
    path: "/home/.agents/skills/agent-browser",
    is_symlink: false,
    symlink_is_broken: false,
    content_hash: "abc",
    disabled: false,
    spec_violations: [],
    ...overrides,
  };
}

/** Minimal `InstalledSkill` fixture, overridable per test. */
function fixtureSkill(overrides: Partial<InstalledSkill> = {}): InstalledSkill {
  return {
    name: "agent-browser",
    source: "getsentry/agent-browser",
    source_type: "github",
    installed_at: "2026-01-01T00:00:00Z",
    has_update: false,
    source_kind: "dotagents",
    deployments: [],
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

describe("isBlockingSpecViolation", () => {
  it("treats a missing required field as blocking", () => {
    expect(isBlockingSpecViolation("missing required frontmatter field: name")).toBe(true);
    expect(isBlockingSpecViolation("missing required frontmatter field: description")).toBe(true);
  });

  it("treats an invalid name format as blocking", () => {
    expect(
      isBlockingSpecViolation(
        'name "Bad Name" must be 1-64 lowercase a-z0-9 characters and hyphens, with no leading, trailing, or consecutive hyphens',
      ),
    ).toBe(true);
  });

  it("treats a name/directory mismatch as blocking", () => {
    expect(
      isBlockingSpecViolation(
        'name "other-name" does not match its directory name "agent-browser"',
      ),
    ).toBe(true);
  });

  it("treats length and style notes as non-blocking", () => {
    expect(isBlockingSpecViolation("description exceeds 1024 characters")).toBe(false);
    expect(isBlockingSpecViolation("compatibility exceeds 500 characters")).toBe(false);
    expect(isBlockingSpecViolation("SKILL.md exceeds recommended 500 lines")).toBe(false);
    expect(isBlockingSpecViolation("conflicting invocation keys")).toBe(false);
  });
});

describe("findSpecViolations", () => {
  it("flags a skill with a blocking violation", () => {
    const skill = fixtureSkill({
      spec_violations: ["missing required frontmatter field: description"],
    });
    const issues = findSpecViolations([skill]);
    expect(issues).toHaveLength(1);
    expect(issues[0].kind).toBe("spec-violation");
    expect(issues[0].detail).toBe("missing required frontmatter field: description");
  });

  it("does not flag a skill with only non-blocking violations", () => {
    const skill = fixtureSkill({
      spec_violations: ["description exceeds 1024 characters", "conflicting invocation keys"],
    });
    expect(findSpecViolations([skill])).toEqual([]);
  });

  it("includes only the blocking violations in detail when both kinds are present", () => {
    const skill = fixtureSkill({
      spec_violations: [
        "missing required frontmatter field: name",
        "description exceeds 1024 characters",
      ],
    });
    const issues = findSpecViolations([skill]);
    expect(issues[0].detail).toBe("missing required frontmatter field: name");
  });
});

describe("coverageGaps", () => {
  it("flags a skill deployed to some, but not all, first-class agents at the same scope", () => {
    const skill = fixtureSkill({
      deployments: [fixtureDeployment({ agent: "Claude Code", scope: "global" })],
    });
    const gaps = coverageGaps([skill]);
    expect(gaps).toHaveLength(1);
    expect(gaps[0].scopeLabel).toBe("Global");
    expect(gaps[0].missing).not.toContain("Claude Code");
  });

  it("does not flag a parked skill", () => {
    const skill = fixtureSkill({
      parked: true,
      deployments: [fixtureDeployment({ agent: "Claude Code", scope: "global" })],
    });
    expect(coverageGaps([skill])).toEqual([]);
  });
});

describe("findDuplicateSkills", () => {
  it("names the differing copies against the strict majority", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ agent: "shared", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "Claude Code", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "Cursor", scope: "global", content_hash: "bbb" }),
      ],
    });
    const issues = findDuplicateSkills([skill]);
    expect(issues).toHaveLength(1);
    expect(issues[0].detail).toBe("Global · Cursor differs from Global · Universal folder");
  });

  it("uses a plural verb when more than one copy differs", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ agent: "shared", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "Claude Code", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "OpenCode", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "Cursor", scope: "global", content_hash: "bbb" }),
        fixtureDeployment({ agent: "Codex", scope: "global", content_hash: "ccc" }),
      ],
    });
    const issues = findDuplicateSkills([skill]);
    expect(issues[0].detail).toBe(
      "Global \u00b7 Cursor; Global \u00b7 Codex differ from Global \u00b7 Universal folder",
    );
  });

  it("lists every copy when there is no strict majority", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ agent: "shared", scope: "global", content_hash: "aaa" }),
        fixtureDeployment({ agent: "Cursor", scope: "global", content_hash: "bbb" }),
      ],
    });
    const issues = findDuplicateSkills([skill]);
    expect(issues[0].detail).toBe("2 copies differ: Global · Universal folder; Global · Cursor");
  });
});

describe("findLinkedRootIssues", () => {
  it("dedupes across several skills sharing one whole-dir-linked root and ignores project scope", () => {
    const linkedGlobal = (name: string) =>
      fixtureSkill({
        name,
        deployments: [
          fixtureDeployment({
            agent: "Claude Code",
            scope: "global",
            path: `/home/.claude/skills/${name}`,
            shared_via_whole_dir_link: true,
          }),
          // A project copy under the same agent must never contribute its own
          // issue - only a global root can be the shared whole-dir link.
          fixtureDeployment({
            agent: "Claude Code",
            scope: "project",
            path: `/repo/.claude/skills/${name}`,
            shared_via_whole_dir_link: true,
          }),
        ],
      });
    const skills = [
      linkedGlobal("agent-browser"),
      linkedGlobal("find-bugs"),
      linkedGlobal("motion"),
    ];

    const issues = findLinkedRootIssues(skills);
    expect(issues).toHaveLength(1);
    expect(issues[0].kind).toBe("linked-root");
    // `harness` is the agent id the backend commands key on; the display
    // label rides along separately for the Convert dialog's copy.
    expect(issues[0].harness).toBe("claude-code");
    expect(issues[0].harnessLabel).toBe("Claude Code");
    expect(issues[0].root).toBe("/home/.claude/skills");
  });

  it("does not flag a per-skill symlink into the Universal root", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({
          agent: "Claude Code",
          scope: "global",
          is_symlink: true,
          symlink_target: "/home/.agents/skills/agent-browser",
          shared_via_whole_dir_link: false,
        }),
      ],
    });
    expect(findLinkedRootIssues([skill])).toEqual([]);
  });
});

describe("HEALTH_ISSUE_KIND_ORDER", () => {
  it("includes parked-but-reinstalled", () => {
    expect(HEALTH_ISSUE_KIND_ORDER).toContain("parked-but-reinstalled");
  });

  it("does not include update-available or missing-from-agents", () => {
    expect(HEALTH_ISSUE_KIND_ORDER).not.toContain("update-available");
    expect(HEALTH_ISSUE_KIND_ORDER).not.toContain("missing-from-agents");
  });
});

describe("findParkedButReinstalled", () => {
  it("flags a parked skill whose Universal deployment came back", () => {
    const skill = fixtureSkill({
      parked: true,
      deployments: [fixtureDeployment({ scope: "global" })],
    });
    const issues = findParkedButReinstalled([skill]);
    expect(issues).toHaveLength(1);
    expect(issues[0].kind).toBe("parked-but-reinstalled");
  });

  it("does not flag a parked skill with only its parked-copy deployment", () => {
    const skill = fixtureSkill({
      parked: true,
      deployments: [fixtureDeployment({ scope: "parked" })],
    });
    expect(findParkedButReinstalled([skill])).toEqual([]);
  });

  it("does not flag a skill that isn't parked", () => {
    const skill = fixtureSkill({
      parked: false,
      deployments: [fixtureDeployment({ scope: "global" })],
    });
    expect(findParkedButReinstalled([skill])).toEqual([]);
  });
});

describe("deploymentWithSpecViolations", () => {
  const WARNING = "description exceeds 1024 characters";
  const ERROR = 'name "Find Bugs" is not a valid skill name';
  const plugin = {
    name: "p",
    version: null,
    harness: "Claude Code",
    marketplace: "m",
    id: "p@m",
  };

  it("a_blocking_copy_beats_an_earlier_warning_only_copy_or_the_error_stays_hidden", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ path: "/a", spec_violations: [WARNING] }),
        fixtureDeployment({ path: "/b", spec_violations: [ERROR] }),
      ],
    });
    expect(deploymentWithSpecViolations(skill)?.path).toBe("/b");
  });

  it("a_skill_with_no_violations_returns_undefined_so_callers_keep_their_default", () => {
    const skill = fixtureSkill({ deployments: [fixtureDeployment()] });
    expect(deploymentWithSpecViolations(skill)).toBeUndefined();
  });

  it("an_own_copy_with_a_warning_beats_a_plugin_copy_with_an_error_or_the_page_opens_a_read_only_file", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ path: "/plugin", spec_violations: [ERROR], plugin }),
        fixtureDeployment({ path: "/own", spec_violations: [WARNING] }),
      ],
    });
    expect(deploymentWithSpecViolations(skill)?.path).toBe("/own");
  });

  it("a_clean_own_copy_and_an_errored_plugin_copy_return_undefined_because_the_problem_is_not_the_users_to_fix", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ path: "/plugin", spec_violations: [ERROR], plugin }),
        fixtureDeployment({ path: "/own" }),
      ],
    });
    expect(deploymentWithSpecViolations(skill)).toBeUndefined();
  });

  it("a_plugin_only_skill_returns_its_errored_plugin_copy", () => {
    const skill = fixtureSkill({
      deployments: [fixtureDeployment({ path: "/plugin", spec_violations: [ERROR], plugin })],
    });
    expect(deploymentWithSpecViolations(skill)?.path).toBe("/plugin");
  });

  it("an_editable_copy_beats_a_symlink_copy_when_both_have_the_same_violation", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ path: "/link", is_symlink: true, spec_violations: [ERROR] }),
        fixtureDeployment({ path: "/physical", spec_violations: [ERROR] }),
      ],
    });
    expect(deploymentWithSpecViolations(skill)?.path).toBe("/physical");
  });

  it("a_skill_whose_only_own_copy_is_a_broken_symlink_returns_undefined_instead_of_a_plugin_copy", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ path: "/plugin", spec_violations: [ERROR], plugin }),
        fixtureDeployment({ path: "/broken", symlink_is_broken: true }),
      ],
    });
    expect(deploymentWithSpecViolations(skill)).toBeUndefined();
  });

  it("a_broken_symlink_never_wins_on_its_violations_alone", () => {
    const skill = fixtureSkill({
      deployments: [
        fixtureDeployment({ path: "/broken", symlink_is_broken: true, spec_violations: [ERROR] }),
      ],
    });
    expect(deploymentWithSpecViolations(skill)).toBeUndefined();
  });
});
