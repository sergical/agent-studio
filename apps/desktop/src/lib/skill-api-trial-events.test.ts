import { expect, it, vi } from "vitest";
import { subscribeToTrialExpiryFailures } from "./skill-api";
import type { TrialExpiryFailure } from "./skill-api";
import { trialExpiryFailureToast } from "./trial-expiry-notification";

const validFailure = {
  name: "find-bugs",
  scope: "global" as const,
  project_path: null,
  recovery_required: false,
  message: "admission refused",
};

function subscribeForTest(received: (payload: TrialExpiryFailure) => void) {
  let callback: ((event: { payload: unknown }) => void) | undefined;
  const stop = subscribeToTrialExpiryFailures((_eventName, nextCallback) => {
    callback = nextCallback;
    return Promise.resolve(() => {});
  }, received);
  return { callback, stop };
}

it("delivers valid failure payloads and rejects malformed payloads", () => {
  const received = vi.fn();
  const subscription = subscribeForTest(received);

  subscription.callback?.({ payload: validFailure });
  subscription.callback?.({ payload: { ...validFailure, recovery_required: "false" } });

  expect(received).toHaveBeenCalledTimes(1);
  expect(received).toHaveBeenCalledWith(validFailure);
  subscription.stop();
});

it("cleans up an expiry-failure listener that resolves after unmount", async () => {
  let callback: ((event: { payload: unknown }) => void) | undefined;
  let resolveListener: ((unlisten: () => void) => void) | undefined;
  const received = vi.fn();
  const unlisten = vi.fn();
  const stop = subscribeToTrialExpiryFailures(
    (_eventName, nextCallback) =>
      new Promise<() => void>((resolve) => {
        callback = nextCallback;
        resolveListener = resolve;
      }),
    received,
  );

  stop();
  resolveListener?.(unlisten);
  await Promise.resolve();
  callback?.({
    payload: {
      name: "find-bugs",
      scope: "global",
      project_path: null,
      recovery_required: false,
      message: "admission refused",
    },
  });

  expect(unlisten).toHaveBeenCalledOnce();
  expect(received).not.toHaveBeenCalled();
});

it("presents retry and recovery failures with stable, distinct toast identities", () => {
  const retry = trialExpiryFailureToast(validFailure);
  const repeatedRetry = trialExpiryFailureToast(validFailure);
  const recovery = trialExpiryFailureToast({
    ...validFailure,
    scope: "project",
    project_path: "/work/project",
    recovery_required: true,
  });

  expect(retry.duration).toBe(15000);
  expect(retry.description).toContain("The expiry will retry.");
  expect(recovery.description).toContain("attempt recovery before retrying.");
  expect(retry.id).toBe(repeatedRetry.id);
  expect(retry.id).not.toBe(recovery.id);

  const colonInPath = trialExpiryFailureToast({
    ...validFailure,
    scope: "project",
    project_path: "/work:a",
    name: "b",
  });
  const colonInName = trialExpiryFailureToast({
    ...validFailure,
    scope: "project",
    project_path: "/work",
    name: "a:b",
  });
  expect(colonInPath.id).not.toBe(colonInName.id);
});
