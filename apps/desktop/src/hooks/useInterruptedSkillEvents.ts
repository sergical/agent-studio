// ============================================================================
// Skill Studio - useInterruptedSkillEvents
// Reads the bounded recovery state when the skill snapshot changes.
// ============================================================================

import { useCallback, useEffect, useRef, useState } from "react";
import { hasInterruptedSkillEvents } from "../lib/skill-api";

export type InterruptedSkillEventsStatus =
  | { kind: "loading" }
  | { kind: "ready"; hasInterrupted: boolean }
  | { kind: "unavailable"; error: string };

interface LoadInterruptedSkillEventsOptions {
  isCurrent: () => boolean;
  read: () => Promise<boolean>;
  setStatus: (status: InterruptedSkillEventsStatus) => void;
}

interface InterruptedSkillEventsLoaderOptions {
  isMounted: () => boolean;
  read: () => Promise<boolean>;
  setStatus: (status: InterruptedSkillEventsStatus) => void;
}

/** Applies one read only while its component and request are still current. */
export async function loadInterruptedSkillEvents({
  isCurrent,
  read,
  setStatus,
}: LoadInterruptedSkillEventsOptions): Promise<void> {
  try {
    const hasInterrupted = await read();
    if (isCurrent()) setStatus({ kind: "ready", hasInterrupted });
  } catch (error) {
    if (isCurrent()) {
      setStatus({
        kind: "unavailable",
        error: error instanceof Error ? error.message : "Could not check recovery status",
      });
    }
  }
}

/** Coalesces refresh requests so a blocked event store has one queued recheck. */
export function createInterruptedSkillEventsLoader({
  isMounted,
  read,
  setStatus,
}: InterruptedSkillEventsLoaderOptions) {
  let inFlight = false;
  let refreshQueued = false;
  let requestVersion = 0;

  const readLatest = async () => {
    inFlight = true;
    do {
      refreshQueued = false;
      const currentVersion = requestVersion;
      await loadInterruptedSkillEvents({
        isCurrent: () => isMounted() && requestVersion === currentVersion,
        read,
        setStatus,
      });
    } while (refreshQueued && isMounted());
    inFlight = false;
  };

  return {
    refresh: () => {
      requestVersion += 1;
      if (inFlight) {
        refreshQueued = true;
        return;
      }
      void readLatest();
    },
  };
}

interface UseInterruptedSkillEventsResult {
  retry: () => void;
  status: InterruptedSkillEventsStatus;
}

/**
 * Reads once on mount and after each published snapshot revision. It does not
 * poll. A refresh retains a known status until the replacement result arrives.
 */
export function useInterruptedSkillEvents(
  snapshotRevision: number | undefined,
): UseInterruptedSkillEventsResult {
  const [status, setStatus] = useState<InterruptedSkillEventsStatus>({ kind: "loading" });
  const lifecycleVersion = useRef(0);
  const loader = useRef<ReturnType<typeof createInterruptedSkillEventsLoader> | undefined>(
    undefined,
  );

  useEffect(() => {
    const instanceVersion = lifecycleVersion.current + 1;
    lifecycleVersion.current = instanceVersion;
    const instanceLoader = createInterruptedSkillEventsLoader({
      isMounted: () => lifecycleVersion.current === instanceVersion,
      read: hasInterruptedSkillEvents,
      setStatus,
    });
    loader.current = instanceLoader;
    return () => {
      if (lifecycleVersion.current === instanceVersion) lifecycleVersion.current += 1;
      if (loader.current === instanceLoader) loader.current = undefined;
    };
  }, []);

  useEffect(() => {
    setStatus((previous) =>
      previous.kind === "ready" && snapshotRevision !== undefined ? previous : { kind: "loading" },
    );
    loader.current?.refresh();
  }, [snapshotRevision]);

  const retry = useCallback(() => {
    setStatus((previous) => (previous.kind === "ready" ? previous : { kind: "loading" }));
    loader.current?.refresh();
  }, []);
  return { status, retry };
}
