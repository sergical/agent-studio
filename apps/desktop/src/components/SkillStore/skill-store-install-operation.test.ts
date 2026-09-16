import { describe, expect, it } from "vitest";
import { createStoreInstallOperationController } from "./skill-store-install-operation";
import type { AddSkillOperationEvent, AddSkillRequest } from "@skill-studio/lib";

const request: AddSkillRequest = {
  source: { kind: "github", repo: "owner/repo", path: "skill", skillName: "skill" },
  method: "skills-sh",
  destination: "universal",
  agents: [],
  disabled_harnesses: [],
  scope: "global",
  trial: false,
};

function event(
  operationId: string,
  sequence: number,
  phase: AddSkillOperationEvent["phase"],
): AddSkillOperationEvent {
  return { operation_id: operationId, sequence, phase, message: phase };
}

describe("Browse install operation controller", () => {
  it("subscribes before starting and keeps a live worker after a catch-up read failure", async () => {
    const calls: string[] = [];
    const events: AddSkillOperationEvent[] = [];
    const statusErrors: string[] = [];
    let listener: ((status: AddSkillOperationEvent) => void) | undefined;
    const controller = createStoreInstallOperationController({
      api: {
        listen: async (callback) => {
          calls.push("listen");
          listener = callback;
          return () => undefined;
        },
        start: async (id) => {
          calls.push("start");
          return event(id, 1, "installing");
        },
        read: async () => Promise.reject(new Error("status unavailable")),
        cancel: async (id) => event(id, 2, "cancelled"),
      },
      createOperationId: () => "op-1",
      onEvent: (status) => events.push(status),
      onStatusError: (message) => statusErrors.push(message),
    });

    await controller.start(request);

    expect(calls).toEqual(["listen", "start"]);
    expect(controller.activeOperationId()).toBe("op-1");
    expect(statusErrors).toEqual(["status unavailable"]);
    listener?.(event("op-1", 2, "completed"));
    expect(events[events.length - 1]?.phase).toBe("completed");
  });

  it("invalidates ownership before a pending listener finishes registering", async () => {
    let resolveListen: ((unlisten: () => void) => void) | undefined;
    let unlistened = false;
    let startCalled = false;
    const cancellations: string[] = [];
    const controller = createStoreInstallOperationController({
      api: {
        listen: () =>
          new Promise((resolve) => {
            resolveListen = resolve;
          }),
        start: async (id) => {
          startCalled = true;
          return event(id, 1, "queued");
        },
        read: async (id) => event(id, 1, "queued"),
        cancel: async (id) => {
          cancellations.push(id);
          return event(id, 1, "cancelled");
        },
      },
      createOperationId: () => "op-1",
      onEvent: () => undefined,
      onStatusError: () => undefined,
    });

    const pendingStart = controller.start(request);
    controller.dispose();
    resolveListen?.(() => {
      unlistened = true;
    });
    await pendingStart;

    expect(controller.activeOperationId()).toBeUndefined();
    expect(unlistened).toBe(true);
    expect(startCalled).toBe(false);
    expect(cancellations).toEqual(["op-1"]);
  });

  it("cancels again when disposal races a pending start", async () => {
    let resolveStart: ((status: AddSkillOperationEvent) => void) | undefined;
    let startEntered: (() => void) | undefined;
    const starting = new Promise<void>((resolve) => {
      startEntered = resolve;
    });
    const cancellations: string[] = [];
    let readCalled = false;
    const controller = createStoreInstallOperationController({
      api: {
        listen: async () => () => undefined,
        start: () =>
          new Promise((resolve) => {
            resolveStart = resolve;
            startEntered?.();
          }),
        read: async (id) => {
          readCalled = true;
          return event(id, 1, "queued");
        },
        cancel: async (id) => {
          cancellations.push(id);
          return event(id, 1, "cancelled");
        },
      },
      createOperationId: () => "op-1",
      onEvent: () => undefined,
      onStatusError: () => undefined,
    });

    const pendingStart = controller.start(request);
    await starting;
    controller.dispose();
    resolveStart?.(event("op-1", 1, "installing"));
    await pendingStart;

    expect(cancellations).toEqual(["op-1", "op-1"]);
    expect(readCalled).toBe(false);
  });

  it("rejects stale callbacks after a released operation and preserves cancellation acknowledgement", async () => {
    const events: AddSkillOperationEvent[] = [];
    let listener: ((status: AddSkillOperationEvent) => void) | undefined;
    let resolveRead: ((status: AddSkillOperationEvent) => void) | undefined;
    let readStarted: (() => void) | undefined;
    const reading = new Promise<void>((resolve) => {
      readStarted = resolve;
    });
    let reads = 0;
    const operationIds = ["op-1", "op-2"];
    const controller = createStoreInstallOperationController({
      api: {
        listen: async (callback) => {
          listener = callback;
          return () => undefined;
        },
        start: async (id) => event(id, 1, "installing"),
        read: (id) => {
          reads += 1;
          if (reads > 1) return Promise.resolve(event(id, 2, "installing"));
          return new Promise((resolve) => {
            resolveRead = resolve;
            readStarted?.();
          });
        },
        cancel: async (id) => event(id, 2, "installing"),
      },
      createOperationId: () => operationIds.shift()!,
      onEvent: (status) => events.push(status),
      onStatusError: () => undefined,
    });

    const firstStart = controller.start(request);
    await reading;
    controller.release("op-1");
    resolveRead?.(event("op-1", 2, "completed"));
    await firstStart;
    await controller.start(request);
    await controller.cancel();

    expect(events.map((status) => status.phase)).toEqual([
      "queued",
      "installing",
      "queued",
      "installing",
      "installing",
    ]);
    expect(controller.activeOperationId()).toBe("op-2");
    listener?.(event("op-2", 3, "cancelled"));
    expect(events[events.length - 1]?.phase).toBe("cancelled");
  });

  it("reports a start refusal as terminal feedback", async () => {
    const events: AddSkillOperationEvent[] = [];
    const controller = createStoreInstallOperationController({
      api: {
        listen: async () => () => undefined,
        start: async () => Promise.reject(new Error("Operation slots are full")),
        read: async (id) => event(id, 1, "queued"),
        cancel: async (id) => event(id, 1, "cancelled"),
      },
      createOperationId: () => "op-1",
      onEvent: (status) => events.push(status),
      onStatusError: () => undefined,
    });

    await controller.start(request);

    expect(events[events.length - 1]).toMatchObject({
      phase: "failed",
      error: "Operation slots are full",
    });
  });
});
