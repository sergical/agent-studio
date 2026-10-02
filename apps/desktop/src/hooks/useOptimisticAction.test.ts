import { describe, expect, it, vi } from "vitest";
import {
  clearIfCurrent,
  performOptimisticAction,
  resolveOptimisticValue,
} from "./useOptimisticAction";

describe("resolveOptimisticValue", () => {
  it("a click before the snapshot arrives shows the new value instead of the old server value", () => {
    expect(resolveOptimisticValue("both", { base: "both", value: "user" })).toBe("user");
  });

  it("a snapshot that shows the new value hands the control back to the server value", () => {
    expect(resolveOptimisticValue("user", { base: "both", value: "user" })).toBe("user");
  });

  it("a snapshot with a different value than the click wins over the stale override", () => {
    expect(resolveOptimisticValue("model", { base: "both", value: "user" })).toBe("model");
  });

  it("no click shows the server value", () => {
    expect(resolveOptimisticValue(true, null)).toBe(true);
  });
});

describe("performOptimisticAction", () => {
  it("a rejected call reverts the value and reports the error message", async () => {
    const onRevert = vi.fn();
    const onError = vi.fn();
    await performOptimisticAction(() => Promise.reject(new Error("disk is full")), {
      onRevert,
      onError,
    });
    expect(onRevert).toHaveBeenCalledOnce();
    expect(onError).toHaveBeenCalledWith("disk is full");
  });

  it("a call that returns false reverts without a second error toast", async () => {
    const onRevert = vi.fn();
    const onError = vi.fn();
    await performOptimisticAction(() => Promise.resolve(false), { onRevert, onError });
    expect(onRevert).toHaveBeenCalledOnce();
    expect(onError).not.toHaveBeenCalled();
  });

  it("a successful call keeps the new value so it stays until the snapshot arrives", async () => {
    const onRevert = vi.fn();
    const onError = vi.fn();
    await performOptimisticAction(() => Promise.resolve(), { onRevert, onError });
    expect(onRevert).not.toHaveBeenCalled();
    expect(onError).not.toHaveBeenCalled();
  });
});

describe("clearIfCurrent", () => {
  it("a failed click clears its own override", () => {
    const mine = { base: "both", value: "user" };
    expect(clearIfCurrent(mine)(mine)).toBeNull();
  });

  it("a failed click leaves a newer click's override in place", () => {
    const first = { base: "both", value: "user" };
    const second = { base: "both", value: "model" };
    expect(clearIfCurrent(first)(second)).toBe(second);
  });
});
