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

  it("repeats an impact sentence once when two violations share it", () => {
    const text = describeSpecViolations([
      "compatibility exceeds 500 characters",
      "SKILL.md exceeds recommended 500 lines",
    ]);
    expect(text.match(/Every agent still loads it\./g)).toHaveLength(1);
  });

  it("says one combined impact when name and description are both missing, or the two sentences contradict each other", () => {
    expect(
      describeSpecViolations([
        "missing required frontmatter field: name",
        "missing required frontmatter field: description",
      ]),
    ).toBe(
      "SKILL.md has no name and description in its frontmatter. Codex, OpenCode, and pi skip it. Claude Code still loads it, under the folder name.",
    );
  });

  it("matches a mismatch whose name holds a newline, or the impact sentence is dropped", () => {
    expect(describeSpecViolations(['name "a\nb" does not match its directory name "c"'])).toContain(
      'Claude Code calls it "c".',
    );
  });

  it("returns an empty string when there is nothing to report", () => {
    expect(describeSpecViolations([])).toBe("");
  });
});
