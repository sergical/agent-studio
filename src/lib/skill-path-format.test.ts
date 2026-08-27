// ============================================================================
// Skill Studio - skill-path-format tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { homeRelativePath, shortProjectPath } from "./skill-path-format";

describe("homeRelativePath", () => {
  it("swaps a guessed /Users/<user> prefix for ~", () => {
    expect(homeRelativePath("/Users/x/src/a")).toBe("~/src/a");
  });

  it("swaps a guessed /home/<user> prefix for ~", () => {
    expect(homeRelativePath("/home/x/src/a")).toBe("~/src/a");
  });

  it("uses an explicit home dir over the guess", () => {
    expect(homeRelativePath("/Users/x/src/a", "/Users/x/src")).toBe("~/a");
  });

  it("returns the path unchanged when it isn't under any home directory", () => {
    expect(homeRelativePath("/opt/skills/find-bugs")).toBe("/opt/skills/find-bugs");
  });
});

describe("shortProjectPath", () => {
  it("keeps only the last two segments, prefixed with ~/", () => {
    expect(shortProjectPath("/Users/x/src/sentry-javascript")).toBe("~/src/sentry-javascript");
  });
});
