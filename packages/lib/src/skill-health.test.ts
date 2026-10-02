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
  findSpecWarnings,
  specViolationSeverity,
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

const NAME_FORMAT =
  'name "Bad Name" must be 1-64 lowercase a-z0-9 characters and hyphens, with no leading, trailing, or consecutive hyphens';
const NAME_MISMATCH = 'name "other-name" does not match its directory name "agent-browser"';

describe("specViolationSeverity", () => {
  it.each([
    "missing required frontmatter field: name",
    "missing required frontmatter field: description",
    "invalid YAML frontmatter at line 3, column 5: bad indent",
  ])("rates %s an error, because at least one agent skips the skill", (violation) => {
    expect(specViolationSeverity(violation)).toBe("error");
  });

  it.each([NAME_MISMATCH, "conflicting invocation keys"])(
    "rates %s a warning, because agents disagree but all load it",
    (violation) => {
      expect(specViolationSeverity(violation)).toBe("warning");
    },
  );

  it.each([
    NAME_FORMAT,
    "description exceeds 1024 characters",
    "compatibility exceeds 500 characters",
    "SKILL.md exceeds recommended 500 lines",
  ])("rates %s a note, because every agent loads it unchanged", (violation) => {
    expect(specViolationSeverity(violation)).toBe("note");
  });

  it("rates an unrecognised message a warning, so a new Rust message is never ignored", () => {
    expect(specViolationSeverity("something new from the validator")).toBe("warning");
  });

  it("blocks only on errors, so a name mismatch does not turn a skill red", () => {
    expect(isBlockingSpecViolation("missing required frontmatter field: name")).toBe(true);
    expect(isBlockingSpecViolation(NAME_MISMATCH)).toBe(false);
    expect(isBlockingSpecViolation(NAME_FORMAT)).toBe(false);
  });
});

describe("findSpecViolations", () => {
  it("flags a skill with an error violation and describes the agent impact", () => {
    const skill = fixtureSkill({
      spec_violations: ["missing required frontmatter field: description"],
    });
    const issues = findSpecViolations([skill]);
    expect(issues).toHaveLength(1);
    expect(issues[0].kind).toBe("spec-violation");
    expect(issues[0].detail).toContain("Codex, OpenCode, and pi skip it.");
  });

  it("does not flag a skill with only warnings or notes, or a failure is hidden as red", () => {
    const skill = fixtureSkill({
      spec_violations: ["description exceeds 1024 characters", NAME_MISMATCH],
    });
    expect(findSpecViolations([skill])).toEqual([]);
  });

  it("leaves warnings and notes out of the detail when an error is present", () => {
    const skill = fixtureSkill({
      spec_violations: [
        "missing required frontmatter field: name",
        "description exceeds 1024 characters",
      ],
    });
    expect(findSpecViolations([skill])[0].detail).not.toContain("1024");
  });
});

describe("findSpecWarnings", () => {
  it("returns a name-mismatch skill as spec-warning and keeps it out of findSpecViolations", () => {
    const skill = fixtureSkill({ spec_violations: [NAME_MISMATCH] });
    const issues = findSpecWarnings([skill]);
    expect(issues).toHaveLength(1);
    expect(issues[0].kind).toBe("spec-warning");
    expect(issues[0].detail).toContain('Claude Code calls it "agent-browser"');
    expect(findSpecViolations([skill])).toEqual([]);
  });

  it("ignores a notes-only skill, or a harmless length note would raise a warning", () => {
    const skill = fixtureSkill({ spec_violations: [NAME_FORMAT] });
    expect(findSpecWarnings([skill])).toEqual([]);
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
  const ERROR = "missing required frontmatter field: description";
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
