// ============================================================================
// ActivityParts - Small pieces the Activity page's sections share: the
// uppercase section header, muted inline text, and the "Show N more" cap on
// a long list.
// ============================================================================

import { useState } from "react";
import type { ReactNode } from "react";
import { Button } from "@skill-studio/ui";

export const LIST_CAP = 10;

export function SectionHeader({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="flex min-h-7 items-center justify-between gap-3">
      <h2 className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
        {title}
      </h2>
      {children}
    </div>
  );
}

export function Muted({ children }: { children: ReactNode }) {
  return <span className="text-small text-text-tertiary tabular-nums">{children}</span>;
}

/** Holds the first `LIST_CAP` items back behind a "Show N more" row. */
export function CappedList<T>({
  items,
  render,
  moreClassName,
}: {
  items: T[];
  render: (item: T) => ReactNode;
  /** Lets the "Show N more" row match rows that bleed into the list's gutter. */
  moreClassName?: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const shown = expanded ? items : items.slice(0, LIST_CAP);
  const hidden = items.length - LIST_CAP;
  return (
    <div className="flex flex-col">
      {shown.map(render)}
      {hidden > 0 && (
        <Button
          variant="ghost"
          size="sm"
          className={`mt-1 self-start text-text-tertiary ${moreClassName ?? ""}`}
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? "Show fewer" : `Show ${hidden} more`}
        </Button>
      )}
    </div>
  );
}
