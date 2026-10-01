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
 * The only files allowed to mention an update command, even as a value: the
 * IPC wrapper, the guard hook, and the batch wrappers, whose callers must run
 * `skillsWithLocalEdits` first (the second test pins that).
 */
const ALLOWED = [
  "/skill-api.ts",
  "/useGuardedSkillUpdate.tsx",
  "/skillBatchUpdates.ts",
  "/skill-lifecycle-target.ts",
  "/dev/harness/",
  ".test.ts",
  ".test.tsx",
];

const UPDATE_COMMANDS =
  /\b(updateSkill|updateSkillOwners|updateAllSkillsWithProgress|updateAllSkills)\b|["']update_skill["']/;

const outsideAllowlist = () =>
  Object.entries(sources).filter(([path]) => !ALLOWED.some((ok) => path.includes(ok)));

describe("update entry points", () => {
  it("no_file_outside_the_allowlist_references_an_update_command_or_it_can_bypass_the_edit_check", () => {
    const bypasses = outsideAllowlist()
      .filter(([, text]) => UPDATE_COMMANDS.test(text))
      .map(([path]) => path);
    // Callers use `requestUpdate` (guard hook) or the batch helpers, never the commands.
    expect(bypasses).toEqual([]);
  });

  it("every_caller_of_a_batch_update_helper_checks_for_local_edits_first_or_update_all_overwrites_silently", () => {
    const unguarded = Object.entries(sources)
      .filter(([path]) => !path.includes(".test.") && !path.includes("/skillBatchUpdates.ts"))
      .filter(([, text]) => /\b(runHomeUpdateAll|runListUpdate)\(/.test(text))
      .filter(([, text]) => !/\bskillsWithLocalEdits\(/.test(text))
      .map(([path]) => path);
    expect(unguarded).toEqual([]);
  });
});
