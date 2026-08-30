// ============================================================================
// Skill Studio - Sidebar tests
// ============================================================================

import { describe, expect, it } from "vitest";
import { relativeScanTime } from "./Sidebar";

describe("relativeScanTime", () => {
  it("reads 'never scanned' when there is no timestamp", () => {
    expect(relativeScanTime(undefined)).toBe("never scanned");
  });

  it("reads 'just now' just under the one-minute boundary", () => {
    const scannedAt = new Date(Date.now() - 59_000).toISOString();
    expect(relativeScanTime(scannedAt)).toBe("just now");
  });

  it("reads '1m ago' at the one-minute boundary", () => {
    const scannedAt = new Date(Date.now() - 60_000).toISOString();
    expect(relativeScanTime(scannedAt)).toBe("1m ago");
  });
});
