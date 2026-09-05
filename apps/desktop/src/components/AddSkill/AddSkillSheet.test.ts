// ============================================================================
// Skill Studio - AddSkillSheet tests
// Covers the manual-add form's submit gating: `availableMethods` (which
// install methods a parsed source supports) and `addSkillFormValid` (the
// single source of truth the footer's disabled state and `handleSubmit`'s
// early return both read). The bug being guarded against: a git source with
// the dotagents CLI absent had no available method yet the submit button
// stayed enabled, so the form shipped a request it had already determined
// could not be installed.
// ============================================================================

import { describe, expect, it } from "vitest";
import { parseSkillSource } from "@skill-studio/lib";
import type {
  AddMethodDefaults,
  GithubSkillEntry,
  InstallScope,
  ParsedSkillSource,
} from "@skill-studio/lib";
import { addSkillFormValid, availableMethods } from "./AddSkillSheet";

function fixtureDefaults(overrides: Partial<AddMethodDefaults> = {}): AddMethodDefaults {
  return {
    dotagents_installed: true,
    has_skill_lock: false,
    installed_harnesses: [],
    claude_reads_shared_folder: false,
    ...overrides,
  };
}

function fixtureEntry(overrides: Partial<GithubSkillEntry> = {}): GithubSkillEntry {
  return { name: "find-bugs", path: "skills/find-bugs", ...overrides };
}

const ERROR_PARSED: ParsedSkillSource | { error: string } = {
  error: "Enter owner/repo, a GitHub URL, a skills.sh URL, or a local path",
};

describe("availableMethods", () => {
  it("returns only dotagents for a git source when dotagents is installed", () => {
    const parsed = parseSkillSource("git:https://example.com/skills.git");
    expect(availableMethods(parsed, fixtureDefaults())).toEqual(["dotagents"]);
  });

  it("returns no methods for a git source when dotagents is not installed", () => {
    const parsed = parseSkillSource("git:https://example.com/skills.git");
    expect(availableMethods(parsed, fixtureDefaults({ dotagents_installed: false }))).toEqual([]);
  });

  it("defaults to treating dotagents as installed when defaults have not loaded yet", () => {
    const parsed = parseSkillSource("git:https://example.com/skills.git");
    expect(availableMethods(parsed, null)).toEqual(["dotagents"]);
  });

  it("includes dotagents first for a github source when it is installed", () => {
    const parsed = parseSkillSource("getsentry/find-bugs");
    expect(availableMethods(parsed, fixtureDefaults())).toEqual([
      "dotagents",
      "skills-sh",
      "copy",
      "pack",
    ]);
  });

  it("drops dotagents from a github source when it is not installed", () => {
    const parsed = parseSkillSource("getsentry/find-bugs");
    expect(availableMethods(parsed, fixtureDefaults({ dotagents_installed: false }))).toEqual([
      "skills-sh",
      "copy",
      "pack",
    ]);
  });

  it("returns only copy for a local path", () => {
    const parsed = parseSkillSource("~/skills/my-skill");
    expect(availableMethods(parsed, fixtureDefaults({ dotagents_installed: false }))).toEqual([
      "copy",
    ]);
  });

  it("returns no methods for a parse error", () => {
    expect(availableMethods(ERROR_PARSED, fixtureDefaults())).toEqual([]);
  });
});

describe("addSkillFormValid", () => {
  it("is invalid when no install method is available, even with a valid parse and scope", () => {
    const parsed = parseSkillSource("git:https://example.com/skills.git");
    expect(
      addSkillFormValid({
        parsed,
        noMethodsAvailable: true,
        scope: "global",
        projectPath: null,
        githubEntries: null,
      }),
    ).toBe(false);
  });

  it("is valid for a git source when dotagents is installed (method available)", () => {
    const parsed = parseSkillSource("git:https://example.com/skills.git");
    expect(
      addSkillFormValid({
        parsed,
        noMethodsAvailable: false,
        scope: "global",
        projectPath: null,
        githubEntries: null,
      }),
    ).toBe(true);
  });

  it("is invalid when the source does not parse", () => {
    expect(
      addSkillFormValid({
        parsed: ERROR_PARSED,
        noMethodsAvailable: false,
        scope: "global",
        projectPath: null,
        githubEntries: null,
      }),
    ).toBe(false);
  });

  it("is invalid for a project scope without a path", () => {
    const parsed = parseSkillSource("getsentry/find-bugs");
    expect(
      addSkillFormValid({
        parsed,
        noMethodsAvailable: false,
        scope: "project",
        projectPath: null,
        githubEntries: null,
      }),
    ).toBe(false);
  });

  it("is valid for a project scope once a path is chosen", () => {
    const parsed = parseSkillSource("~/skills/my-skill");
    expect(
      addSkillFormValid({
        parsed,
        noMethodsAvailable: false,
        scope: "project",
        projectPath: "/home/me/projects/app",
        githubEntries: null,
      }),
    ).toBe(true);
  });

  it("is invalid for a github source whose picker holds no picked skills", () => {
    const parsed = parseSkillSource("getsentry/find-bugs");
    expect(
      addSkillFormValid({
        parsed,
        noMethodsAvailable: false,
        scope: "global",
        projectPath: null,
        githubEntries: [],
      }),
    ).toBe(false);
  });

  it("is valid for a github source with at least one picked skill folder", () => {
    const parsed = parseSkillSource("getsentry/find-bugs");
    expect(
      addSkillFormValid({
        parsed,
        noMethodsAvailable: false,
        scope: "global",
        projectPath: null,
        githubEntries: [fixtureEntry()],
      }),
    ).toBe(true);
  });

  it("treats noMethodsAvailable as the deciding gate even when every other field is valid", () => {
    const parsed = parseSkillSource("git:https://example.com/skills.git");
    const scope: InstallScope = "global";
    expect(
      addSkillFormValid({
        parsed,
        noMethodsAvailable: true,
        scope,
        projectPath: "/home/me/projects/app",
        githubEntries: [fixtureEntry()],
      }),
    ).toBe(false);
  });
});

describe("submit gating derivation", () => {
  // Mirrors the component's own derivation: parsed -> availableMethods ->
  // noMethodsAvailable -> addSkillFormValid. The bug this guards against is
  // `noMethodsAvailable` reaching `true` while the submit button stayed live.
  function formValidFor(source: string, defaults: AddMethodDefaults | null): boolean {
    const parsed = parseSkillSource(source);
    const methods = availableMethods(parsed, defaults);
    const noMethodsAvailable = !("error" in parsed) && methods.length === 0;
    return addSkillFormValid({
      parsed,
      noMethodsAvailable,
      scope: "global",
      projectPath: null,
      githubEntries: null,
    });
  }

  it("blocks submission for a git URL when dotagents is not installed", () => {
    expect(
      formValidFor(
        "git:https://example.com/skills.git",
        fixtureDefaults({ dotagents_installed: false }),
      ),
    ).toBe(false);
  });

  it("blocks submission for a bare .git URL when dotagents is not installed", () => {
    expect(
      formValidFor(
        "https://example.com/skills.git",
        fixtureDefaults({ dotagents_installed: false }),
      ),
    ).toBe(false);
  });

  it("allows submission for a git URL when dotagents is installed", () => {
    expect(
      formValidFor(
        "git:https://example.com/skills.git",
        fixtureDefaults({ dotagents_installed: true }),
      ),
    ).toBe(true);
  });

  it("allows submission for a local path regardless of dotagents (copy always applies)", () => {
    expect(formValidFor("~/skills/my-skill", fixtureDefaults({ dotagents_installed: false }))).toBe(
      true,
    );
  });

  it("allows submission for a github source when dotagents is not installed (skills-sh/copy still apply)", () => {
    expect(
      formValidFor("getsentry/find-bugs", fixtureDefaults({ dotagents_installed: false })),
    ).toBe(true);
  });

  it("blocks submission for a parse error", () => {
    expect(formValidFor("not a valid source!!", fixtureDefaults())).toBe(false);
  });
});
