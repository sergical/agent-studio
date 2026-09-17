// ============================================================================
// perf-marks tests
// Guards the paint mark's uniqueness: two calls of the same command that both
// resolve before the next animation frame must not clobber each other's mark.
// ============================================================================

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { recordIpcCall, subscribe } from "./perf-marks";
import type { PerfEntry } from "./perf-marks";

describe("recordIpcCall", () => {
  let rafCallbacks: FrameRequestCallback[] = [];

  beforeEach(() => {
    rafCallbacks = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      rafCallbacks.push(callback);
      return rafCallbacks.length;
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("does not throw for two same-command calls resolving before the next frame", () => {
    let latest: PerfEntry[] = [];
    const unsubscribe = subscribe((entries) => {
      latest = entries;
    });

    expect(() => {
      recordIpcCall("scan_skills", 5);
      recordIpcCall("scan_skills", 7);
      for (const callback of rafCallbacks) callback(0);
    }).not.toThrow();

    const recorded = latest.slice(-2);
    expect(recorded).toHaveLength(2);
    for (const entry of recorded) {
      expect(entry.command).toBe("scan_skills");
      expect(entry.paintMs).not.toBeNull();
    }

    unsubscribe();
  });
});
