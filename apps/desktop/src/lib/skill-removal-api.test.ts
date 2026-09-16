import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { removeSkill } from "./skill-api";

describe("removeSkill", () => {
  beforeEach(() => {
    vi.stubGlobal("window", {});
  });

  afterEach(() => {
    clearMocks();
    vi.unstubAllGlobals();
  });

  it("preserves native string failures for Error-based UI feedback", async () => {
    mockIPC(() => Promise.reject("Removal failed after changing files"));
    await expect(removeSkill({ owner_id: "owner-1" })).rejects.toThrow(
      "Removal failed after changing files",
    );
  });

  it("preserves existing Error identity", async () => {
    const failure = new Error("Provider unavailable");
    mockIPC(() => Promise.reject(failure));
    await expect(removeSkill({ owner_id: "owner-1" })).rejects.toBe(failure);
  });

  it("uses a removal fallback for empty native failures", async () => {
    mockIPC(() => Promise.reject(""));
    await expect(removeSkill({ owner_id: "owner-1" })).rejects.toThrow("Removal failed");
  });

  it.each([true, false])("preserves returned success=%s and exact target", async (success) => {
    const result = { success, skill_name: "selected-skill" };
    const target = { deployment_id: "deployment-1" };
    const handler = vi.fn(() => result);
    mockIPC(handler);
    await expect(removeSkill(target)).resolves.toBe(result);
    expect(handler).toHaveBeenCalledExactlyOnceWith("remove_skill", { target });
  });
});
