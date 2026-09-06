// ============================================================================
// Skill Studio - Sidebar tests
// ============================================================================

import { describe, expect, it } from "vitest";
import {
  hasNewerSkillSnapshotEmission,
  relativeScanTime,
  rescanTooltip,
} from "../../lib/sidebar-nav";

describe("relativeScanTime", () => {
  it("reads 'Never' when there is no timestamp", () => {
    expect(relativeScanTime(undefined)).toBe("Never");
  });

  it("reads 'Just now' just under the one-minute boundary", () => {
    const scannedAt = new Date(Date.now() - 59_000).toISOString();
    expect(relativeScanTime(scannedAt)).toBe("Just now");
  });

  it("reads '1m ago' at the one-minute boundary", () => {
    const scannedAt = new Date(Date.now() - 60_000).toISOString();
    expect(relativeScanTime(scannedAt)).toBe("1m ago");
  });
});

describe("rescanTooltip", () => {
  it("shows 'Last sync: Never' when timestamp is missing", () => {
    expect(rescanTooltip(undefined)).toBe("Refresh skills from disk\nLast sync: Never");
  });

  it("shows 'Last sync: Just now' for recent scans", () => {
    const scannedAt = new Date(Date.now() - 10_000).toISOString();
    expect(rescanTooltip(scannedAt)).toBe("Refresh skills from disk\nLast sync: Just now");
  });
});

describe("hasNewerSkillSnapshotEmission", () => {
  it("does not complete from an initial snapshot read", () => {
    expect(hasNewerSkillSnapshotEmission(0, undefined)).toBe(false);
  });

  it("completes when a listener delivers a newer backend revision", () => {
    expect(hasNewerSkillSnapshotEmission(4, 5)).toBe(true);
  });
});
