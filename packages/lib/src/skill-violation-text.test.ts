import { describe, expect, it } from "vitest";
import { describeSpecViolations } from "./skill-violation-text";

describe("describeSpecViolations", () => {
  it("collapses every missing frontmatter field into one clause", () => {
    expect(
      describeSpecViolations([
        "missing required frontmatter field: name",
        "missing required frontmatter field: description",
      ]),
    ).toBe("SKILL.md has no name and description in its frontmatter.");
  });

  it("lists three or more missing fields with a serial comma", () => {
    expect(
      describeSpecViolations([
        "missing required frontmatter field: name",
        "missing required frontmatter field: description",
        "missing required frontmatter field: license",
      ]),
    ).toBe("SKILL.md has no name, description, and license in its frontmatter.");
  });

  it("keeps other violations as their own sentences, after the missing fields", () => {
    expect(
      describeSpecViolations([
        "description exceeds 1024 characters",
        "missing required frontmatter field: name",
        "conflicting invocation keys",
      ]),
    ).toBe(
      "SKILL.md has no name in its frontmatter. Description exceeds 1024 characters. Conflicting invocation keys.",
    );
  });

  it("returns an empty string when there is nothing to report", () => {
    expect(describeSpecViolations([])).toBe("");
  });
});
