import { describe, expect, it, vi } from "vitest";
import type { SkillEvent } from "@skill-studio/lib";
import { SkillHistoryReader } from "./skill-history-reader";

function deferred() {
  let resolve: (events: SkillEvent[]) => void = () => undefined;
  const promise = new Promise<SkillEvent[]>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

describe("SkillHistoryReader", () => {
  it("shares equivalent pending reads without caching completed rows", async () => {
    const response = deferred();
    const fetch = vi.fn(() => response.promise);
    const reader = new SkillHistoryReader(fetch);
    const first = reader.read();
    expect(reader.read(200)).toBe(first);
    await Promise.resolve();
    expect(fetch).toHaveBeenCalledExactlyOnceWith(200, undefined);
    response.resolve([]);
    await first;
    expect(reader.read()).not.toBe(first);
  });

  it("starts a fresh read after invalidation and ignores the older settlement", async () => {
    const old = deferred();
    const fresh = deferred();
    const fetch = vi
      .fn<() => Promise<SkillEvent[]>>()
      .mockReturnValueOnce(old.promise)
      .mockReturnValueOnce(fresh.promise);
    const reader = new SkillHistoryReader(fetch);
    const before = reader.read();
    await Promise.resolve();
    reader.invalidate();
    const after = reader.read();
    old.resolve([]);
    await before;
    expect(reader.read()).toBe(after);
    fresh.resolve([]);
    await after;
  });
});
