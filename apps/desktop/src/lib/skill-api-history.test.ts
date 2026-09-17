import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { invalidateSkillEventReads, listSkillEvents } from "./skill-api";

function deferred() {
  let resolve: (value: []) => void = () => undefined;
  const promise = new Promise<[]>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

describe("History API", () => {
  beforeEach(() => vi.stubGlobal("window", {}));
  afterEach(() => {
    invalidateSkillEventReads();
    clearMocks();
    vi.unstubAllGlobals();
  });
  it("starts a new transport after a snapshot revision invalidates an in-flight read", async () => {
    const old = deferred();
    const fresh = deferred();
    const invoke = vi.fn().mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
    mockIPC(invoke);

    const before = listSkillEvents();
    await Promise.resolve();
    invalidateSkillEventReads();
    const after = listSkillEvents();

    expect(after).not.toBe(before);
    old.resolve([]);
    fresh.resolve([]);
    await expect(after).resolves.toEqual([]);
    expect(invoke).toHaveBeenCalledTimes(2);
  });
});
