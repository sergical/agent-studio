// ============================================================================
// Skill Studio - skill-pack-name tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { validatePackName } from "./skill-pack-name";

describe("validatePackName", () => {
  it("accepts lowercase letters, digits, and dashes", () => {
    expect(validatePackName("my-skills-2")).toBeUndefined();
  });

  it("rejects an empty name", () => {
    expect(validatePackName("")).toBeDefined();
  });

  it("rejects uppercase letters and spaces", () => {
    expect(validatePackName("My Skills")).toBeDefined();
  });

  it("rejects a name longer than 64 characters", () => {
    expect(validatePackName("a".repeat(65))).toBeDefined();
  });

  it("accepts a name exactly 64 characters long", () => {
    expect(validatePackName("a".repeat(64))).toBeUndefined();
  });

  it("rejects a name starting with a dash", () => {
    expect(validatePackName("-my-skills")).toBeDefined();
  });
});
