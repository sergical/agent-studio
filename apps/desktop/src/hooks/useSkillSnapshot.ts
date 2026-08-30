// ============================================================================
// Skill Studio - useSkillSnapshot
// Subscribes to the background refresh thread's skill snapshot
// ============================================================================

import { useCallback, useEffect, useState } from "react";
import { getSkillSnapshot, onSkillSnapshot, requestSkillRescan } from "../lib/skill-api";
import type { SkillSnapshot } from "@skill-studio/lib";

interface UseSkillSnapshotResult {
  snapshot: SkillSnapshot | undefined;
  isLoading: boolean;
  error: string | null;
  /** Ask the background refresh thread to rebuild; resolves once the request lands, not once the new snapshot arrives. */
  requestRescan: () => Promise<void>;
}

/**
 * Reads the current skill snapshot on mount and stays subscribed to
 * `skills://snapshot` for every rebuild after that (install/remove/update,
 * a background scan, or an explicit `requestRescan`). Never polls.
 */
export function useSkillSnapshot(): UseSkillSnapshotResult {
  const [snapshot, setSnapshot] = useState<SkillSnapshot | undefined>(undefined);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    getSkillSnapshot()
      .then((initial) => {
        if (!cancelled) {
          setSnapshot(initial);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load skill snapshot");
        }
      })
      .finally(() => {
        if (!cancelled) {
          setIsLoading(false);
        }
      });

    const unlisten = onSkillSnapshot((next) => {
      if (!cancelled) {
        setSnapshot(next);
        setIsLoading(false);
      }
    });

    return () => {
      cancelled = true;
      unlisten();
    };
  }, []);

  const requestRescan = useCallback(async () => {
    try {
      await requestSkillRescan();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to request rescan");
    }
  }, []);

  return { snapshot, isLoading, error, requestRescan };
}
