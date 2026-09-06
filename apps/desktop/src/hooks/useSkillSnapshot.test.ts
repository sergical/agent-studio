import { describe, expect, it } from "vitest";
import { selectNewerSkillSnapshot, startSkillSnapshotSubscription } from "./useSkillSnapshot";
import type { SkillSnapshot } from "@skill-studio/lib";

function snapshot(revision: number, scannedAt: string): SkillSnapshot {
  return {
    revision,
    skills: [],
    projects: [],
    invocations: [],
    heatmap: { days: {} },
    scanned_at: scannedAt,
    last_test_by_skill: {},
    update_check: {
      checked_at: null,
      gh_status: "ok",
      message: null,
      updates_available: 0,
    },
  };
}

describe("selectNewerSkillSnapshot", () => {
  it("keeps an event that arrives before an older initial read completes", () => {
    const eventSnapshot = snapshot(4, "event");
    const afterEvent = selectNewerSkillSnapshot(undefined, eventSnapshot);

    expect(selectNewerSkillSnapshot(afterEvent, snapshot(3, "initial"))).toBe(eventSnapshot);
  });

  it("accepts revision zero only as the initial legacy bootstrap", () => {
    const legacy = snapshot(0, "legacy");

    expect(selectNewerSkillSnapshot(undefined, legacy)).toBe(legacy);
    expect(selectNewerSkillSnapshot(legacy, snapshot(0, "second legacy"))).toBe(legacy);
    expect(selectNewerSkillSnapshot(legacy, snapshot(1, "current"))?.revision).toBe(1);
  });
});

describe("startSkillSnapshotSubscription", () => {
  it("registers before reading and rejects a stale initial read", async () => {
    let current: SkillSnapshot | undefined;
    let listener: ((snapshot: SkillSnapshot) => void) | undefined;
    let finishRead: ((snapshot: SkillSnapshot) => void) | undefined;
    const read = new Promise<SkillSnapshot>((resolve) => {
      finishRead = resolve;
    });
    const subscription = startSkillSnapshotSubscription({
      isCancelled: () => false,
      listen: async (registeredListener) => {
        listener = registeredListener;
        return () => undefined;
      },
      read: () => read,
      onSnapshot: (candidate) => {
        current = selectNewerSkillSnapshot(current, candidate);
      },
      onError: () => undefined,
      onSettled: () => undefined,
    });
    await Promise.resolve();
    listener?.(snapshot(2, "event"));
    finishRead?.(snapshot(1, "initial"));
    await subscription;

    expect(current?.scanned_at).toBe("event");
  });

  it("disposes a listener that finishes registering after unmount", async () => {
    let cancelled = false;
    let finishListen: ((unlisten: () => void) => void) | undefined;
    let disposed = false;
    let readCalled = false;
    const listen = new Promise<() => void>((resolve) => {
      finishListen = resolve;
    });
    const subscription = startSkillSnapshotSubscription({
      isCancelled: () => cancelled,
      listen: () => listen,
      read: async () => {
        readCalled = true;
        return snapshot(1, "initial");
      },
      onSnapshot: () => undefined,
      onError: () => undefined,
      onSettled: () => undefined,
    });

    cancelled = true;
    finishListen?.(() => {
      disposed = true;
    });
    await subscription;

    expect(disposed).toBe(true);
    expect(readCalled).toBe(false);
  });
});
