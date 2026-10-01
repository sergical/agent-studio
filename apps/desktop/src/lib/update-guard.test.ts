// ============================================================================
// Skill Studio - update guard coverage
// Every file that runs an update must also check for local edits first, or
// that button silently overwrites a user's edits again.
// ============================================================================

import { describe, expect, it } from "vitest";

const sources = import.meta.glob<string>("../**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

/**
 * Files that define the update commands or take them as callbacks, so they
 * trigger nothing themselves; their callers are what the checks below cover.
 */
const NOT_A_TRIGGER = [
  "skill-api.ts",
  "skill-lifecycle-target.ts",
  "home-inbox-data.ts",
  "skill-bulk-actions.ts",
  "/dev/harness/",
  ".test.ts",
  ".test.tsx",
];

const triggers = () =>
  Object.entries(sources).filter(([path]) => !NOT_A_TRIGGER.some((skip) => path.includes(skip)));

describe("update entry points", () => {
  it("every_single_skill_update_goes_through_requestUpdate_or_that_button_overwrites_silently", () => {
    const unguarded = triggers()
      .filter(([, text]) => /\b(updateSkill|updateSkillOwners)\(/.test(text))
      .filter(([, text]) => !/\brequestUpdate\b/.test(text))
      .map(([path]) => path);
    expect(unguarded).toEqual([]);
  });

  it("every_batch_update_checks_for_local_edits_first_or_update_all_overwrites_silently", () => {
    const unguarded = triggers()
      .filter(([, text]) => /\bupdateAllSkillsWithProgress\(/.test(text))
      .filter(([, text]) => !/\bskillsWithLocalEdits\(/.test(text))
      .map(([path]) => path);
    expect(unguarded).toEqual([]);
  });
});
