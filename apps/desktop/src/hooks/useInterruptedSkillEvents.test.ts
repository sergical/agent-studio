import { describe, expect, it } from "vitest";
import {
  createInterruptedSkillEventsLoader,
  loadInterruptedSkillEvents,
  type InterruptedSkillEventsStatus,
} from "./useInterruptedSkillEvents";

describe("loadInterruptedSkillEvents", () => {
  it("becomes available after retrying a failed check", async () => {
    const statuses: InterruptedSkillEventsStatus[] = [];
    await loadInterruptedSkillEvents({
      isCurrent: () => true,
      read: async () => {
        throw new Error("Event store is unavailable");
      },
      setStatus: (status) => statuses.push(status),
    });
    await loadInterruptedSkillEvents({
      isCurrent: () => true,
      read: async () => false,
      setStatus: (status) => statuses.push(status),
    });

    expect(statuses).toEqual([
      { kind: "unavailable", error: "Event store is unavailable" },
      { kind: "ready", hasInterrupted: false },
    ]);
  });

  it("does not apply a stale or unmounted response", async () => {
    let finishFirst: ((value: boolean) => void) | undefined;
    let finishSecond: ((value: boolean) => void) | undefined;
    let currentRequest = 1;
    const statuses: InterruptedSkillEventsStatus[] = [];
    const first = loadInterruptedSkillEvents({
      isCurrent: () => currentRequest === 1,
      read: () =>
        new Promise((resolve) => {
          finishFirst = resolve;
        }),
      setStatus: (status) => statuses.push(status),
    });
    currentRequest = 2;
    const second = loadInterruptedSkillEvents({
      isCurrent: () => currentRequest === 2,
      read: () =>
        new Promise((resolve) => {
          finishSecond = resolve;
        }),
      setStatus: (status) => statuses.push(status),
    });

    finishSecond?.(true);
    await second;
    finishFirst?.(false);
    await first;

    expect(statuses).toEqual([{ kind: "ready", hasInterrupted: true }]);
  });
});

describe("createInterruptedSkillEventsLoader", () => {
  it("does not revive a completed effect instance", async () => {
    let finish: ((value: boolean) => void) | undefined;
    let lifecycleVersion = 1;
    const statuses: InterruptedSkillEventsStatus[] = [];
    const instanceVersion = lifecycleVersion;
    const loader = createInterruptedSkillEventsLoader({
      isMounted: () => lifecycleVersion === instanceVersion,
      read: () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
      setStatus: (status) => statuses.push(status),
    });

    loader.refresh();
    lifecycleVersion += 1;
    lifecycleVersion += 1;
    finish?.(true);
    await Promise.resolve();

    expect(statuses).toEqual([]);
  });

  it("coalesces a burst of refreshes into one follow-up read", async () => {
    let finishFirst: ((value: boolean) => void) | undefined;
    let finishSecond: ((value: boolean) => void) | undefined;
    let signalSecondRead: (() => void) | undefined;
    const statuses: InterruptedSkillEventsStatus[] = [];
    const secondReadStarted = new Promise<void>((resolve) => {
      signalSecondRead = resolve;
    });
    const reads: Array<() => Promise<boolean>> = [
      () =>
        new Promise((resolve) => {
          finishFirst = resolve;
        }),
      () =>
        new Promise((resolve) => {
          signalSecondRead?.();
          finishSecond = resolve;
        }),
    ];
    const read = () => reads.shift()?.() ?? Promise.reject(new Error("unexpected read"));
    const loader = createInterruptedSkillEventsLoader({
      isMounted: () => true,
      read,
      setStatus: (status) => statuses.push(status),
    });

    loader.refresh();
    loader.refresh();
    loader.refresh();
    expect(reads).toHaveLength(1);

    finishFirst?.(false);
    await secondReadStarted;
    expect(reads).toHaveLength(0);
    finishSecond?.(true);
    await Promise.resolve();

    expect(statuses).toEqual([{ kind: "ready", hasInterrupted: true }]);
  });
});
