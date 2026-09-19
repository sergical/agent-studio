import { describe, expect, it } from "vitest";
import { startDoctorReportSubscription } from "./useDoctor";
import type { DoctorReport } from "@skill-studio/lib";

function report(checked: number): DoctorReport {
  return { violations: [], checked };
}

describe("startDoctorReportSubscription", () => {
  it("delivers a report that arrives after registration", async () => {
    let listener: ((report: DoctorReport) => void) | undefined;
    const received: number[] = [];

    const unlisten = await startDoctorReportSubscription({
      isCancelled: () => false,
      listen: async (registeredListener) => {
        listener = registeredListener;
        return () => undefined;
      },
      onReport: (candidate) => received.push(candidate.checked),
      hasReport: () => received.length > 0,
      runFallback: () => undefined,
    });

    listener?.(report(3));

    expect(received).toEqual([3]);
    expect(unlisten).toBeTypeOf("function");
  });

  it("disposes a listener that finishes registering after unmount, or names the leaked listener", async () => {
    let disposed = false;
    let finishListen: ((unlisten: () => void) => void) | undefined;
    let cancelled = false;
    const listen = new Promise<() => void>((resolve) => {
      finishListen = resolve;
    });

    const subscription = startDoctorReportSubscription({
      isCancelled: () => cancelled,
      listen: () => listen,
      onReport: () => undefined,
      hasReport: () => false,
      runFallback: () => undefined,
    });

    cancelled = true;
    finishListen?.(() => {
      disposed = true;
    });
    const unlisten = await subscription;

    expect(disposed).toBe(true);
    expect(unlisten).toBeUndefined();
  });

  it("a listener that never resolves (no event backend) resolves undefined instead of rejecting", async () => {
    const unlisten = await startDoctorReportSubscription({
      isCancelled: () => false,
      listen: () => Promise.reject(new Error("no tauri event backend")),
      onReport: () => undefined,
      hasReport: () => false,
      runFallback: () => undefined,
    });

    expect(unlisten).toBeUndefined();
  });

  it("runs the fallback once the listener settles with no report yet, or names the missed run", async () => {
    let fallbackCalls = 0;

    await startDoctorReportSubscription({
      isCancelled: () => false,
      listen: async () => () => undefined,
      onReport: () => undefined,
      hasReport: () => false,
      runFallback: () => {
        fallbackCalls += 1;
      },
    });

    expect(fallbackCalls).toBe(1);
  });

  it("skips the fallback when a report already landed before the listener settled", async () => {
    let fallbackCalls = 0;

    await startDoctorReportSubscription({
      isCancelled: () => false,
      listen: async () => () => undefined,
      onReport: () => undefined,
      hasReport: () => true,
      runFallback: () => {
        fallbackCalls += 1;
      },
    });

    expect(fallbackCalls).toBe(0);
  });

  it("runs the fallback when there is no event backend and no report yet, or names the stuck card", async () => {
    let fallbackCalls = 0;

    await startDoctorReportSubscription({
      isCancelled: () => false,
      listen: () => Promise.reject(new Error("no tauri event backend")),
      onReport: () => undefined,
      hasReport: () => false,
      runFallback: () => {
        fallbackCalls += 1;
      },
    });

    expect(fallbackCalls).toBe(1);
  });

  it("clears a stale error when a report event arrives, or names the surviving error", async () => {
    let listener: ((report: DoctorReport) => void) | undefined;
    let error: string | null = "a previous manual run failed";
    let received: DoctorReport | null = null;

    await startDoctorReportSubscription({
      isCancelled: () => false,
      listen: async (registeredListener) => {
        listener = registeredListener;
        return () => undefined;
      },
      onReport: (candidate) => {
        error = null;
        received = candidate;
      },
      hasReport: () => received !== null,
      runFallback: () => undefined,
    });

    listener?.(report(5));

    expect(error).toBeNull();
    expect(received).toEqual(report(5));
  });
});
