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
    });

    expect(unlisten).toBeUndefined();
  });
});
