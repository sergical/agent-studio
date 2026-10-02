// ============================================================================
// useOptimisticAction - shows a control's new value the moment it is clicked,
// while a backend call that takes seconds runs. The new value overrides the
// server value until the next snapshot moves the server value (then the
// server's value wins) or the call fails (then the old value returns and an
// error toast appears).
// ============================================================================

import { useState } from "react";
import { invokeErrorMessage } from "../lib/skill-api";
import { useAppStore } from "../store/appStore";

interface OptimisticOverride<T> {
  /** The server value when the click happened. The override ends once the server value differs. */
  base: T;
  value: T;
}

/** The value a control shows: the override while the server value still equals `base`, else the server's. */
export function resolveOptimisticValue<T>(
  serverValue: T,
  override: OptimisticOverride<T> | null,
): T {
  if (override === null) return serverValue;
  return Object.is(serverValue, override.base) ? override.value : serverValue;
}

interface OptimisticFailureHandlers {
  onRevert: () => void;
  onError: (message: string) => void;
}

/**
 * Awaits `action`. A throw reverts and reports the error message; a `false` result reverts
 * without a message, for an action that already showed its own error toast. Resolves true on success.
 */
export async function performOptimisticAction(
  action: () => Promise<boolean | void>,
  { onRevert, onError }: OptimisticFailureHandlers,
): Promise<boolean> {
  let failed = false;
  let message: string | null = null;
  try {
    const result = await action();
    failed = result === false;
  } catch (err) {
    failed = true;
    message = invokeErrorMessage(err);
  }
  if (!failed) return true;
  onRevert();
  if (message !== null) onError(message);
  return false;
}

export interface OptimisticAction<T> {
  /** The value to show: the new one while pending, else the server's. */
  value: T;
  /** True from the click until the server value changes or the call fails. Show a spinner. */
  pending: boolean;
  /** Shows `next` at once and awaits `action`. Never rejects; resolves true when the action succeeded. */
  run: (next: T, action: () => Promise<boolean | void>, errorTitle: string) => Promise<boolean>;
}

/** How long a successful save keeps its override when no snapshot changes the server value. */
const SETTLE_MS = 5000;

export function useOptimisticAction<T>(serverValue: T): OptimisticAction<T> {
  const addToast = useAppStore((state) => state.addToast);
  const [override, setOverride] = useState<OptimisticOverride<T> | null>(null);

  // The server value moved: the snapshot caught up, so drop the override now. Left in state, it
  // would apply again if the server value later returned to `base`.
  if (override !== null && !Object.is(serverValue, override.base)) setOverride(null);

  const run = async (next: T, action: () => Promise<boolean | void>, errorTitle: string) => {
    const mine = { base: serverValue, value: next };
    setOverride(mine);
    const saved = await performOptimisticAction(action, {
      onRevert: () => setOverride(null),
      onError: (message) => addToast({ type: "error", title: errorTitle, message }),
    });
    // A save that leaves the server value unchanged never ends the override on its own; drop it
    // once the snapshot has had time to land so the spinner cannot stick.
    if (saved)
      setTimeout(() => setOverride((current) => (current === mine ? null : current)), SETTLE_MS);
    return saved;
  };

  return {
    value: resolveOptimisticValue(serverValue, override),
    pending: override !== null && Object.is(serverValue, override.base),
    run,
  };
}
