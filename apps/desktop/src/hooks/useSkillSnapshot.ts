// ============================================================================
// Skill Studio - useSkillSnapshot
// Subscribes to the background refresh thread's skill snapshot
// ============================================================================

import { useCallback, useEffect, useRef, useState } from "react";
import { getSkillSnapshot, onSkillSnapshot, requestSkillRescan } from "../lib/skill-api";
import type { SkillSnapshot } from "@skill-studio/lib";

/** Select a snapshot only when its publication revision advances. */
export function selectNewerSkillSnapshot(
  current: SkillSnapshot | undefined,
  candidate: SkillSnapshot | undefined,
): SkillSnapshot | undefined {
  if (!candidate) return current;
  if (!current || candidate.revision > current.revision) return candidate;
  return current;
}

interface SkillSnapshotSubscription {
  isCancelled: () => boolean;
  listen: typeof onSkillSnapshot;
  read: typeof getSkillSnapshot;
  onSnapshot: (snapshot: SkillSnapshot, source: "initial" | "event") => void;
  onError: (message: string) => void;
  onSettled: () => void;
}

/** Register the snapshot listener before reading and dispose late registrations after unmount. */
export async function startSkillSnapshotSubscription({
  isCancelled,
  listen,
  read,
  onSnapshot,
  onError,
  onSettled,
}: SkillSnapshotSubscription): Promise<(() => void) | undefined> {
  let unlisten: (() => void) | undefined;
  try {
    unlisten = await listen((candidate) => {
      if (!isCancelled()) onSnapshot(candidate, "event");
    });
    if (isCancelled()) {
      unlisten();
      return undefined;
    }
    const initial = await read();
    if (isCancelled()) {
      unlisten();
      return undefined;
    }
    if (initial) onSnapshot(initial, "initial");
    return unlisten;
  } catch (error) {
    unlisten?.();
    if (!isCancelled()) {
      onError(error instanceof Error ? error.message : "Failed to load skill snapshot");
    }
    return undefined;
  } finally {
    if (!isCancelled()) onSettled();
  }
}

interface UseSkillSnapshotResult {
  snapshot: SkillSnapshot | undefined;
  /** Latest backend revision received through `skills://snapshot`, excluding the initial read. */
  emittedSnapshotRevision: number | undefined;
  isLoading: boolean;
  error: string | null;
  /** Loads the list again. After a failed first load it runs the subscription again; once
   *  subscribed it asks the background refresh thread to rebuild and resolves when that request
   *  lands, not when the new snapshot arrives. */
  requestRescan: () => Promise<void>;
}

/** Where the `skills://snapshot` subscription stands. A failed one has disposed its listener,
 *  so a backend rebuild would go unheard until the subscription is started again. */
type SnapshotSubscriptionState = "starting" | "live" | "failed";

/**
 * Reads the current skill snapshot on mount and stays subscribed to
 * `skills://snapshot` for every rebuild after that (install/remove/update,
 * a background scan, or an explicit `requestRescan`). Never polls.
 */
export function useSkillSnapshot(): UseSkillSnapshotResult {
  const [snapshot, setSnapshot] = useState<SkillSnapshot | undefined>(undefined);
  const [emittedSnapshotRevision, setEmittedSnapshotRevision] = useState<number | undefined>(
    undefined,
  );
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const subscriptionRef = useRef<SnapshotSubscriptionState>("starting");
  const restartSubscriptionRef = useRef<(() => void) | undefined>(undefined);
  const isMountedRef = useRef(true);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    isMountedRef.current = true;

    const applySnapshot = (candidate: SkillSnapshot, source: "initial" | "event") => {
      if (!cancelled) {
        setSnapshot((current) => selectNewerSkillSnapshot(current, candidate));
        if (source === "event") setEmittedSnapshotRevision(candidate.revision);
        setIsLoading(false);
      }
    };

    const subscribe = () => {
      subscriptionRef.current = "starting";
      void startSkillSnapshotSubscription({
        isCancelled: () => cancelled,
        listen: onSkillSnapshot,
        read: getSkillSnapshot,
        onSnapshot: applySnapshot,
        onError: setError,
        onSettled: () => setIsLoading(false),
      }).then((registeredUnlisten) => {
        if (cancelled) {
          registeredUnlisten?.();
          return;
        }
        unlisten = registeredUnlisten;
        subscriptionRef.current = registeredUnlisten ? "live" : "failed";
      });
    };

    subscribe();
    restartSubscriptionRef.current = subscribe;

    return () => {
      cancelled = true;
      isMountedRef.current = false;
      restartSubscriptionRef.current = undefined;
      unlisten?.();
    };
  }, []);

  // Memoized so an effect keyed on it (the load-error toast in App) runs once
  // per failure, not once per render. A retry starts clean: a stale reason
  // would otherwise hide a repeat of the same failure.
  const requestRescan = useCallback(async () => {
    setError(null);
    if (subscriptionRef.current === "failed") {
      setIsLoading(true);
      restartSubscriptionRef.current?.();
      return;
    }
    try {
      await requestSkillRescan();
    } catch (err) {
      if (isMountedRef.current) {
        setError(err instanceof Error ? err.message : "Failed to request rescan");
      }
      throw err;
    }
  }, []);

  return { snapshot, emittedSnapshotRevision, isLoading, error, requestRescan };
}
