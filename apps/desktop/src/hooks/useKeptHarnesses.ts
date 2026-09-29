// ============================================================================
// useKeptHarnesses - the harness ids the user kept on the first-run screen.
// Empty until the saved choice loads, and when there is none.
// ============================================================================

import { useEffect, useState } from "react";
import { getHarnessesChoice } from "../lib/skill-api";

export function useKeptHarnesses(): string[] {
  const [kept, setKept] = useState<string[]>([]);
  useEffect(() => {
    let cancelled = false;
    getHarnessesChoice()
      .then((choice) => {
        if (!cancelled) setKept(choice?.kept ?? []);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);
  return kept;
}
