// ============================================================================
// Skill Studio - skill-api error normalization tests
// Tauri rejects a failed command with the Rust `Result::Err` string directly,
// not an `Error`. Every catch site across the app does
// `err instanceof Error ? err.message : "Unknown error"`, so a raw string
// rejection reads as "Unknown error" unless `callCommand` normalizes it
// first. `mockIPC` stands in for the real Tauri bridge without mocking the
// `skill-api` module itself - the wrapper under test runs unchanged.
// ============================================================================

import { afterEach, describe, expect, it } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { removeSkill, restoreSkillEvent } from "./skill-api";
import { shouldOfferForceRestore } from "../components/Activity/skill-history-restore-policy";

// This suite runs under Vitest's plain Node environment (no jsdom in this
// workspace) - `@tauri-apps/api/mocks` only needs a `window` object to hang
// `__TAURI_INTERNALS__` off, so `globalThis` stands in for it rather than
// pulling in a DOM implementation this file doesn't otherwise need.
// SAFETY: no DOM lib is loaded in this Node test environment, so `window`
// is never declared - assigning `globalThis` under that name is additive,
// not a narrowing of an existing value.
(globalThis as { window?: typeof globalThis }).window = globalThis;

afterEach(() => {
  clearMocks();
});

describe("callCommand normalizes a raw string rejection to an Error", () => {
  it("a_backend_string_rejection_keeps_its_text_instead_of_reading_unknown_error", async () => {
    mockIPC(() => {
      throw "skill not found: find-bugs";
    });

    await expect(removeSkill({ deployment_id: "find-bugs" })).rejects.toThrow(
      "skill not found: find-bugs",
    );
  });

  it("an_already_thrown_error_passes_through_unwrapped", async () => {
    const original = new Error("network unreachable");
    mockIPC(() => {
      throw original;
    });

    await expect(removeSkill({ deployment_id: "find-bugs" })).rejects.toBe(original);
  });

  it("a_drift_refusal_string_reaches_the_restore_catch_site_intact_so_force_restore_stays_reachable", async () => {
    mockIPC(() => {
      throw "/root has changed since the event";
    });

    let caught: unknown;
    try {
      await restoreSkillEvent("event-1", false);
    } catch (err) {
      caught = err;
    }

    // The exact catch-site logic from SkillHistorySection.tsx's useRestoreEvent: before the
    // fix, a raw string rejection here always read "Unknown error", which never matches
    // isDriftRefusal - the force-restore dialog was unreachable no matter what the backend said.
    const message = caught instanceof Error ? caught.message : "Unknown error";
    expect(message).toBe("/root has changed since the event");
    expect(
      shouldOfferForceRestore(
        {
          id: "event-1",
          ts: "2026-09-05T00:00:00Z",
          kind: "explode_shared_dir",
          skill: "find-bugs",
          status: "done",
          restorable: true,
          force_restorable: true,
          harness: null,
          project_path: null,
          reverted_by: null,
          scope: null,
        },
        message,
      ),
    ).toBe(true);
  });
});
