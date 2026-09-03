// ============================================================================
// StatusIcon - the one status mechanism the Locations card uses (see
// status-spec.md §1): a severity dot overlaid on the bottom-right corner of an
// identity icon. The icon itself never swaps, tints, or dims - only the dot
// (and its tooltip) carries the status.
// ============================================================================

import type { ReactNode } from "react";
import { TooltipControl } from "./TooltipControl";
import type { TooltipLine } from "./TooltipControl";
import type { StatusLevel } from "../SkillDetail/skill-location-status";

const DOT_COLOR = {
  error: "bg-error",
  warning: "bg-warning",
  off: "bg-text-secondary",
} satisfies Record<StatusLevel, string>;

/** 6px dot + 1.5px ring on a 16-18px row icon, 5px dot + 1px ring on a 12px stack glyph, see status-spec.md §1. */
const DOT_SIZE = {
  18: "size-1.5 ring-[1.5px]",
  16: "size-1.5 ring-[1.5px]",
  12: "size-[5px] ring-1",
} satisfies Record<16 | 12 | 18, string>;

interface StatusIconProps {
  icon: ReactNode;
  level?: StatusLevel;
  tip?: TooltipLine[];
  size?: 16 | 12 | 18;
}

/** An identity icon with an optional severity dot ringed in the row background - never a tinted or swapped icon, see status-spec.md §1. */
export function StatusIcon({ icon, level, tip, size = 16 }: StatusIconProps) {
  const content = (
    <span className="relative inline-flex shrink-0 items-center justify-center text-text-secondary">
      {icon}
      {level && (
        <span
          className={`absolute right-[-2px] bottom-[-2px] rounded-full ring-bg-primary ${DOT_COLOR[level]} ${DOT_SIZE[size]}`}
          aria-hidden="true"
        />
      )}
    </span>
  );
  if (!tip || tip.length === 0) return content;
  return <TooltipControl content={tip}>{content}</TooltipControl>;
}
