import { describe, expect, it } from "vitest";
import {
  applyAddSkillOperationEvent,
  listenForAddSkillOperation,
  startAddSkillOperationSubscription,
} from "./useAddSkillOperation";
import type { AddSkillOperationEvent } from "@skill-studio/lib";
import { shouldConsumeAddSkillOperation } from "@skill-studio/lib";

function event(
  operationId: string,
  sequence: number,
  phase: AddSkillOperationEvent["phase"],
): AddSkillOperationEvent {
  return {
    operation_id: operationId,
    sequence,
    phase,
    message: phase,
  };
}

describe("applyAddSkillOperationEvent", () => {
  it("rejects a stale sequence after a newer event", () => {
    const current = event("op-1", 4, "installing");
    expect(applyAddSkillOperationEvent(current, event("op-1", 2, "queued"), "op-1")).toBe(current);
  });

  it("permits a fresh submit after decline and ignores a late parent event", () => {
    let trackedOperationId: string | undefined;
    expect(applyAddSkillOperationEvent(undefined, event("op-1", 5, "cancelled"), trackedOperationId))
      .toBeUndefined();

    trackedOperationId = "op-2";
    const fresh = event("op-2", 1, "queued");
    const lateParent = event("op-1", 5, "cancelled");
    expect(applyAddSkillOperationEvent(fresh, lateParent, trackedOperationId)).toBe(fresh);
  });
});

describe("listenForAddSkillOperation", () => {
  it("registers before any start or status read", async () => {
    const calls: string[] = [];
    let listener: ((event: AddSkillOperationEvent) => void) | undefined;
    await listenForAddSkillOperation({
      isCancelled: () => false,
      listen: async (registeredListener) => {
        calls.push("listen");
        listener = registeredListener;
        return () => undefined;
      },
      onEvent: () => {
        calls.push("event");
      },
    });
    expect(calls).toEqual(["listen"]);
    listener?.(event("op-1", 1, "queued"));
    expect(calls).toEqual(["listen", "event"]);
  });
});

describe("startAddSkillOperationSubscription", () => {
  it("registers before reading and rejects a stale initial read", async () => {
    let current: AddSkillOperationEvent | undefined;
    let listener: ((event: AddSkillOperationEvent) => void) | undefined;
    let finishRead: ((event: AddSkillOperationEvent) => void) | undefined;
    const read = new Promise<AddSkillOperationEvent>((resolve) => {
      finishRead = resolve;
    });
    const subscription = startAddSkillOperationSubscription({
      isCancelled: () => false,
      listen: async (registeredListener) => {
        listener = registeredListener;
        return () => undefined;
      },
      read: () => read,
      operationId: "op-1",
      onEvent: (candidate) => {
        current = applyAddSkillOperationEvent(current, candidate, "op-1");
      },
      onError: () => undefined,
    });
    await Promise.resolve();
    listener?.(event("op-1", 2, "installing"));
    finishRead?.(event("op-1", 1, "queued"));
    await subscription;

    expect(current?.phase).toBe("installing");
    expect(current?.sequence).toBe(2);
  });

  it("disposes a listener that finishes registering after unmount", async () => {
    let cancelled = false;
    let finishListen: ((unlisten: () => void) => void) | undefined;
    let disposed = false;
    let readCalled = false;
    const listen = new Promise<() => void>((resolve) => {
      finishListen = resolve;
    });
    const subscription = startAddSkillOperationSubscription({
      isCancelled: () => cancelled,
      listen: () => listen,
      read: async () => {
        readCalled = true;
        return event("op-1", 1, "queued");
      },
      operationId: "op-1",
      onEvent: () => undefined,
      onError: () => undefined,
    });

    cancelled = true;
    finishListen?.(() => {
      disposed = true;
    });
    await subscription;

    expect(disposed).toBe(true);
    expect(readCalled).toBe(false);
  });

  it("does not consume a terminal event twice", () => {
    const completed = event("op-1", 8, "completed");
    expect(shouldConsumeAddSkillOperation(completed, undefined)).toBe(true);
    expect(shouldConsumeAddSkillOperation(completed, "op-1")).toBe(false);
  });
});
