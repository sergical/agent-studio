// ============================================================================
// Skill Studio - skill-source-parse tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { parseSkillSource } from "./skill-source-parse";

describe("parseSkillSource", () => {
  it("parses a bare owner/repo", () => {
    expect(parseSkillSource("getsentry/find-bugs")).toEqual({
      kind: "github",
      repo: "getsentry/find-bugs",
    });
  });

  it("parses owner/repo/<path> without guessing a skill name", () => {
    expect(parseSkillSource("getsentry/find-bugs/skills/find-bugs")).toEqual({
      kind: "github",
      repo: "getsentry/find-bugs",
      path: "skills/find-bugs",
    });
  });

  it("parses a bare github.com repo URL", () => {
    expect(parseSkillSource("https://github.com/getsentry/find-bugs")).toEqual({
      kind: "github",
      repo: "getsentry/find-bugs",
    });
  });

  it("parses a github.com /tree/<ref>/<path> URL without guessing a skill name", () => {
    expect(
      parseSkillSource("https://github.com/getsentry/find-bugs/tree/v2/skills/find-bugs"),
    ).toEqual({
      kind: "github",
      repo: "getsentry/find-bugs",
      path: "skills/find-bugs",
      ref: "v2",
    });
  });

  it("leaves a /tree/<ref>/skills folder URL as a folder, not one skill", () => {
    expect(parseSkillSource("https://github.com/kentcdodds/kcd-skills/tree/main/skills")).toEqual({
      kind: "github",
      repo: "kentcdodds/kcd-skills",
      path: "skills",
      ref: "main",
    });
  });

  it("parses a github.com /blob/<ref>/<path>/SKILL.md URL", () => {
    expect(
      parseSkillSource(
        "https://github.com/getsentry/find-bugs/blob/main/skills/find-bugs/SKILL.md",
      ),
    ).toEqual({
      kind: "github",
      repo: "getsentry/find-bugs",
      path: "skills/find-bugs",
      ref: "main",
      skillName: "find-bugs",
    });
  });

  it("rejects a /blob/<ref>/<path> URL that doesn't point at SKILL.md", () => {
    expect(
      parseSkillSource(
        "https://github.com/getsentry/find-bugs/blob/main/skills/find-bugs/README.md",
      ),
    ).toEqual({
      error: "Enter owner/repo, a GitHub URL, a skills.sh URL, or a local path",
    });
  });

  it("parses a skills.sh URL", () => {
    expect(parseSkillSource("https://skills.sh/getsentry/find-bugs/find-bugs")).toEqual({
      kind: "github",
      repo: "getsentry/find-bugs",
      path: "find-bugs",
      skillName: "find-bugs",
    });
  });

  it("parses a git: source", () => {
    expect(parseSkillSource("git:https://example.com/skills.git")).toEqual({
      kind: "git",
      url: "https://example.com/skills.git",
    });
  });

  it("parses a bare .git URL", () => {
    expect(parseSkillSource("https://example.com/skills.git")).toEqual({
      kind: "git",
      url: "https://example.com/skills.git",
    });
  });

  it("parses an absolute local path", () => {
    expect(parseSkillSource("/Users/me/my-skill")).toEqual({
      kind: "local",
      localPath: "/Users/me/my-skill",
    });
  });

  it("parses a ~/ local path", () => {
    expect(parseSkillSource("~/skills/my-skill")).toEqual({
      kind: "local",
      localPath: "~/skills/my-skill",
    });
  });

  it("trims surrounding whitespace before parsing", () => {
    expect(parseSkillSource("  getsentry/find-bugs  ")).toEqual({
      kind: "github",
      repo: "getsentry/find-bugs",
    });
  });

  it("rejects an empty source", () => {
    expect(parseSkillSource("   ")).toEqual({
      error: "Enter owner/repo, a GitHub URL, a skills.sh URL, or a local path",
    });
  });

  it("rejects garbage input", () => {
    expect(parseSkillSource("not a valid source!!")).toEqual({
      error: "Enter owner/repo, a GitHub URL, a skills.sh URL, or a local path",
    });
  });
});
