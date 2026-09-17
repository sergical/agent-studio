// ============================================================================
// GroupHead - a list's sticky group header: chevron, label, count, and an
// optional trailing action. Shared by Home's inbox groups and the Skills
// list's state groups so both read as one product.
// ============================================================================

import type { ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import { CollapsibleTrigger } from "@skill-studio/ui";

/** A group's sticky header: chevron, label, count, spacer, optional extra action. Sits inside a
 * `Collapsible`, whose `data-panel-open` drives the chevron. The trigger's accessible name spells
 * out label and count, since the visible count is a bare number. */
export function GroupHead({
  label,
  count,
  extra,
  groupId,
}: {
  label: string;
  count: number;
  extra?: ReactNode;
  /** Marks the trigger with `data-group-header` so `useRowCursor`'s ArrowLeft/ArrowRight handling
   * can find and focus it without a ref. */
  groupId?: string;
}) {
  return (
    <div className="sticky top-0 z-1 flex h-7 w-full items-center gap-2 rounded-sm bg-bg-secondary px-3">
      <CollapsibleTrigger
        data-group-header={groupId}
        className="group/head flex h-full flex-1 items-center gap-2 text-left text-small font-medium text-text-secondary"
        aria-label={`${label}, ${count} skill${count === 1 ? "" : "s"}`}
      >
        <ChevronDown
          className="size-3.5 shrink-0 -rotate-90 text-text-tertiary transition-transform motion-reduce:transition-none group-data-panel-open/head:rotate-0"
          aria-hidden
        />
        {label}
        <span className="font-normal text-text-tertiary tabular-nums">{count}</span>
      </CollapsibleTrigger>
      {extra}
    </div>
  );
}
