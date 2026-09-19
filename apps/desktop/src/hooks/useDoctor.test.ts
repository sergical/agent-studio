import { describe, expect, it } from "vitest";
import { createDoctorReportHandlers, startDoctorReportSubscription } from "./useDoctor";
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

  it("skips the fallback when createDoctorReportHandlers already recorded a report, or names the extra run", async () => {
    let fallbackCalls = 0;
    const hasReportRef = { current: false };
    const handlers = createDoctorReportHandlers(
      hasReportRef,
      () => undefined,
      () => undefined,
      () => {
        fallbackCalls += 1;
      },
    );
    // A report already landed (e.g. the startup pass fired before this
    // listener finished registering) - onReport ran and flipped the ref.
    handlers.onReport(report(1));

    await startDoctorReportSubscription({
      isCancelled: () => false,
      listen: async () => () => undefined,
      ...handlers,
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

  it("clears a stale error and stores the report through createDoctorReportHandlers, or names the surviving error", () => {
    let error: string | null = "a previous manual run failed";
    let stored: DoctorReport | null = null;
    const hasReportRef = { current: false };
    const handlers = createDoctorReportHandlers(
      hasReportRef,
      (nextError) => {
        error = nextError;
      },
      (nextReport) => {
        stored = nextReport;
      },
      () => undefined,
    );

    handlers.onReport(report(5));

    expect(error).toBeNull();
    expect(stored).toEqual(report(5));
    expect(hasReportRef.current).toBe(true);
  });

  it("flips hasReportRef synchronously inside onReport, or names the race with the mount fallback", () => {
    const hasReportRef = { current: false };
    const handlers = createDoctorReportHandlers(
      hasReportRef,
      () => undefined,
      () => undefined,
      () => undefined,
    );

    expect(handlers.hasReport()).toBe(false);
    handlers.onReport(report(1));
    // No render/effect cycle runs between these two lines - if the ref only
    // flipped through a `report`-keyed effect, this assertion would still see
    // `false` here.
    expect(handlers.hasReport()).toBe(true);
  });
});
