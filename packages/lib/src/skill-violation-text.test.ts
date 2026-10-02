import { describe, expect, it } from "vitest";
import { describeSpecViolations } from "./skill-violation-text";

describe("describeSpecViolations", () => {
  it("collapses every missing frontmatter field into one clause", () => {
    expect(
      describeSpecViolations([
        "missing required frontmatter field: name",
        "missing required frontmatter field: description",
      ]),
    ).toContain("SKILL.md has no name and description in its frontmatter. ");
  });

  it("lists three or more missing fields with a serial comma", () => {
    expect(
      describeSpecViolations([
        "missing required frontmatter field: name",
        "missing required frontmatter field: description",
        "missing required frontmatter field: license",
      ]),
    ).toContain("SKILL.md has no name, description, and license in its frontmatter. ");
  });

  it("keeps other violations as their own sentences, after the missing fields", () => {
    expect(
      describeSpecViolations([
        "description exceeds 1024 characters",
        "missing required frontmatter field: name",
        "conflicting invocation keys",
      ]),
    ).toBe(
      "SKILL.md has no name in its frontmatter. Description exceeds 1024 characters. Conflicting invocation keys. Every agent still loads it. pi shows a warning. OpenCode skips it without a warning. Claude Code, Codex, and pi use the folder name.",
    );
  });

  it("names both names in a mismatch so the reader sees which agent shows which", () => {
    expect(describeSpecViolations(['name "a" does not match its directory name "b"'])).toContain(
      'Claude Code calls it "b". Codex, OpenCode, and pi call it "a".',
    );
  });

  it("states who skips a skill with no name and no description, once each", () => {
    const text = describeSpecViolations([
      "missing required frontmatter field: name",
      "missing required frontmatter field: description",
    ]);
    expect(text).toContain("Codex, OpenCode, and pi skip it. Claude Code still loads it.");
    expect(text).toContain("OpenCode skips it without a warning.");
  });

  it("repeats an impact sentence once when two violations share it", () => {
    const text = describeSpecViolations([
      "compatibility exceeds 500 characters",
      "SKILL.md exceeds recommended 500 lines",
    ]);
    expect(text.match(/Every agent still loads it\./g)).toHaveLength(1);
  });

  it("returns an empty string when there is nothing to report", () => {
    expect(describeSpecViolations([])).toBe("");
  });
});
