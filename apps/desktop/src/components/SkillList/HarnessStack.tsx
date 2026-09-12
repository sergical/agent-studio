// ============================================================================
// HarnessStack - the avatar-group stack: the reached harnesses overlap into
// one shape, the Universal folder leads when present, and past `MAX_DISCS`
// the rest fold into a "+N" disc.
// ============================================================================

import type { ReactNode } from "react";
import type { InstalledSkill } from "@skill-studio/lib";
import { RichTooltip } from "../ui/RichTooltip";
import { HarnessGlyph, HarnessMark, UniversalMark } from "./HarnessMark";
import { whereFacts } from "./skill-row-state";
import type { HarnessListEntry, HarnessReach } from "./skill-row-state";

/** Past this many discs (the Universal folder counted as one), the rest fold into a "+N" disc. */
const MAX_DISCS = 5;

/** The "+N" disc's tooltip: one line per hidden harness, glyph and label only - no how word,
 * since the row already shows how. */
function OverflowTooltip({ overflow }: { overflow: HarnessReach[] }) {
  return (
    <div className="flex flex-col gap-1 text-small">
      {overflow.map((reach) => (
        <span key={reach.harness} className="flex items-center gap-1.5 text-text-tertiary">
          <HarnessGlyph harness={reach.harness} size={14} />
          {reach.label}
        </span>
      ))}
    </div>
  );
}

/** One disc in the stack: the avatar-group convention - `size-5 rounded-full
 * bg-bg-secondary`, ringed in the page's own background colour so the ring is
 * what visually separates neighbours rather than a border against the fill,
 * `-ml-1` (4px) overlap after the first so the group reads as one shape until
 * a disc is singled out on hover, and `group-hover:ring-bg-secondary` so the
 * row hover doesn't leave the ring looking detached from its fill. A disabled
 * or parked disc only mutes its glyph - `HarnessMark` already does that, so
 * the disc itself needs no separate disabled treatment. */
function Disc({ first, children }: { first: boolean; children: ReactNode }) {
  return (
    <span
      className={`relative inline-flex size-5 items-center justify-center rounded-full bg-bg-secondary ring-2 ring-bg-primary group-hover:ring-bg-secondary hover:z-10 ${
        first ? "" : "-ml-1"
      }`}
    >
      {children}
    </span>
  );
}

/** The reached-harness stack: the Universal folder disc first when present, then one disc per
 * reached harness up to `MAX_DISCS`, then a "+N" overflow disc. */
export function HarnessStack({
  skill,
  harnessList,
}: {
  skill: InstalledSkill;
  harnessList: HarnessListEntry[];
}) {
  const facts = whereFacts(skill, harnessList);
  const { universal, harnesses } = facts;
  const reached = harnesses.filter((h) => h.reached);
  if (!universal.present && reached.length === 0) {
    return <span className="text-small text-text-tertiary">—</span>;
  }
  const discs: ReactNode[] = [];
  if (universal.present) {
    discs.push(
      <Disc key="universal" first={discs.length === 0}>
        <UniversalMark universal={universal} size={11} />
      </Disc>,
    );
  }
  const budget = MAX_DISCS - discs.length;
  const shown = reached.slice(0, budget);
  const overflow = reached.slice(budget);
  for (const reach of shown) {
    discs.push(
      <Disc key={reach.harness} first={discs.length === 0}>
        <HarnessMark reach={reach} size={11} parked={skill.parked} />
      </Disc>,
    );
  }
  if (overflow.length > 0) {
    discs.push(
      <Disc key="overflow" first={discs.length === 0}>
        <RichTooltip content={<OverflowTooltip overflow={overflow} />}>
          <span className="text-caption tabular-nums text-text-tertiary">+{overflow.length}</span>
        </RichTooltip>
      </Disc>,
    );
  }
  return <span className="inline-flex items-center">{discs}</span>;
}
