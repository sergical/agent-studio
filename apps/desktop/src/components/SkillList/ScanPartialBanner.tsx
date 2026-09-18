// ============================================================================
// ScanPartialBanner - Warns when the background scan's read budget ran out
// before every root could be reached, so the skill list may be missing
// entries. See `SkillSnapshot.scan_partial`/`scan_observations`
// (skill_refresh.rs).
// ============================================================================

import { useState } from "react";
import { Button } from "@skill-studio/ui";

interface ScanPartialBannerProps {
  observations: string[];
  /** `snapshot.unread_roots.length` - a whole root or a single unreadable
   * skill directory, so "locations" rather than "roots". */
  unreadLocationCount: number;
}

export function ScanPartialBanner({ observations, unreadLocationCount }: ScanPartialBannerProps) {
  const [dismissed, setDismissed] = useState(false);
  if (dismissed) return null;

  return (
    <div
      role="status"
      className="flex items-start justify-between gap-3 rounded-sm border border-warning bg-warning-soft px-3.5 py-2.5 text-sm text-text-primary"
    >
      <div className="flex-1">
        <p className="select-text">
          Scan incomplete: {unreadLocationCount} locations were not read. Showing the last
          complete result for those locations.
        </p>
        {observations.length > 0 && (
          <details className="mt-1 text-text-secondary">
            <summary className="select-none">Details</summary>
            <ul className="mt-1 list-disc pl-4 select-text">
              {observations.map((observation) => (
                <li key={observation}>{observation}</li>
              ))}
            </ul>
          </details>
        )}
      </div>
      <Button
        variant="ghost"
        size="icon-xs"
        onClick={() => setDismissed(true)}
        className="shrink-0 text-text-tertiary"
        aria-label="Dismiss"
      >
        ×
      </Button>
    </div>
  );
}
