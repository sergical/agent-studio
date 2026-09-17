import { afterEach, expect, it, vi } from "vitest";
import type { SkillRefreshPosition, SkillSnapshot } from "@skill-studio/lib";
import { SkillRefreshWaiter, snapshotCoversRefresh } from "./skill-refresh-waiter";

function snapshot(full_refresh: SkillRefreshPosition | null, revision = 1): SkillSnapshot {
  return {
    full_refresh,
    revision,
    skills: [],
    projects: [],
    invocations: [],
    heatmap: { days: {} },
    scanned_at: "fixture",
    last_test_by_skill: {},
    update_check: { checked_at: null, gh_status: "ok", message: null, updates_available: 0 },
  };
}
const receipt = { instance_id: "fixture-instance", generation: "2" };
afterEach(() => {
  vi.useRealTimers();
});

it("requires full coverage from the same instance even above JavaScript's safe integer range", () => {
  const large = { ...receipt, generation: "9007199254740993" };
  expect(snapshotCoversRefresh(snapshot({ ...large, generation: "9007199254740992" }), large)).toBe(
    false,
  );
  expect(snapshotCoversRefresh(snapshot(large), large)).toBe(true);
  expect(
    snapshotCoversRefresh(snapshot({ ...large, instance_id: "previous-instance" }), large),
  ).toBe(false);
  expect(snapshotCoversRefresh(snapshot(null, 999), large)).toBe(false);
  expect(snapshotCoversRefresh(snapshot({ ...large, generation: "1e20" }), large)).toBe(false);
});

it("shares one pending request and ignores newer snapshots without required full coverage", async () => {
  const request = vi.fn(async () => receipt);
  const waiter = new SkillRefreshWaiter({
    request,
    read: async () => undefined,
    publish: () => undefined,
  });
  const pending = waiter.request();
  expect(waiter.request()).toBe(pending);
  let completed = false;
  void pending.then(() => {
    completed = true;
  });
  await vi.waitFor(() => expect(request).toHaveBeenCalledOnce());
  waiter.accept(snapshot({ ...receipt, generation: "1" }, 999));
  await Promise.resolve();
  expect(completed).toBe(false);
  waiter.accept(snapshot(receipt, 1000));
  await pending;
  expect(completed).toBe(true);
  waiter.dispose();
});

it("accepts a covering event delivered before the request response", async () => {
  let acknowledge: (value: SkillRefreshPosition) => void = () => undefined;
  const response = new Promise<SkillRefreshPosition>((resolve) => {
    acknowledge = resolve;
  });
  const read = vi.fn(async () => undefined);
  const waiter = new SkillRefreshWaiter({
    request: () => response,
    read,
    publish: () => undefined,
  });
  const pending = waiter.request();
  waiter.accept(snapshot(receipt, 2));
  waiter.accept(snapshot(null, 1));
  acknowledge(receipt);
  await pending;
  expect(read).not.toHaveBeenCalled();
  waiter.dispose();
});

it("catches up a lost event through a cached read near the deadline", async () => {
  vi.useFakeTimers();
  let latest: SkillSnapshot | undefined;
  const publish = vi.fn();
  const read = vi.fn(async () => latest);
  const waiter = new SkillRefreshWaiter({
    request: async () => receipt,
    read,
    publish,
    timeoutMs: 10_000,
  });
  const pending = waiter.request();
  await vi.advanceTimersByTimeAsync(0);
  latest = snapshot(receipt);
  await vi.advanceTimersByTimeAsync(9000);
  await pending;
  expect(read).toHaveBeenCalledTimes(2);
  expect(publish).toHaveBeenCalledWith(latest);
  expect(vi.getTimerCount()).toBe(0);
  waiter.dispose();
});

it("times out without cancelling the shared rebuild or accepting another instance", async () => {
  vi.useFakeTimers();
  const waiter = new SkillRefreshWaiter({
    request: async () => receipt,
    read: async () => snapshot({ ...receipt, instance_id: "other" }),
    publish: () => undefined,
    timeoutMs: 10_000,
  });
  const pending = expect(waiter.request()).rejects.toThrow("taking longer");
  await vi.advanceTimersByTimeAsync(10_000);
  await pending;
  expect(vi.getTimerCount()).toBe(0);
  waiter.dispose();
});

it("disposes pending waits and ignores a late cached result", async () => {
  vi.useFakeTimers();
  let finishRead: (value: SkillSnapshot) => void = () => undefined;
  const cached = new Promise<SkillSnapshot>((resolve) => {
    finishRead = resolve;
  });
  const publish = vi.fn();
  const waiter = new SkillRefreshWaiter({
    request: async () => receipt,
    read: () => cached,
    publish,
  });
  const pending = expect(waiter.request()).rejects.toThrow("cancelled");
  await vi.advanceTimersByTimeAsync(0);
  waiter.dispose();
  finishRead(snapshot(receipt));
  await pending;
  await vi.advanceTimersByTimeAsync(0);
  expect(publish).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});

it("rejects a failed request and permits a later retry", async () => {
  const request = vi
    .fn()
    .mockRejectedValueOnce(new Error("private failure"))
    .mockResolvedValue(receipt);
  const waiter = new SkillRefreshWaiter({
    request,
    read: async () => snapshot(receipt),
    publish: () => undefined,
  });
  await expect(waiter.request()).rejects.toThrow("Failed to request refresh");
  await waiter.request();
  expect(request).toHaveBeenCalledTimes(2);
  waiter.dispose();
});
