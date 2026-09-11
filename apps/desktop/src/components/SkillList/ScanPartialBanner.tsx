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
}

export function ScanPartialBanner({ observations }: ScanPartialBannerProps) {
  const [dismissed, setDismissed] = useState(false);
  if (dismissed) return null;

  return (
    <div className="flex items-start justify-between gap-3 rounded-sm border border-warning bg-warning-soft px-3.5 py-2.5 text-sm text-text-primary">
      <div className="flex-1">
        <p>Scan incomplete: {observations.length} roots were not read</p>
        {observations.length > 0 && (
          <details className="mt-1 text-text-secondary">
            <summary className="cursor-pointer select-none">Details</summary>
            <ul className="mt-1 list-disc pl-4">
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
