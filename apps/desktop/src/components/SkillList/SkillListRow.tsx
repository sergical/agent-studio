// ============================================================================
// SkillListRow - One row of SkillListTable: the state glyph, name, disk
// location, harness stack, and token pair for one skill, absolutely
// positioned within its group by the caller's virtualizer.
// ============================================================================

import type { CSSProperties } from "react";
import type { InstalledSkill } from "@skill-studio/lib";
import { HarnessStack } from "./HarnessStack";
import {
  LeadingCell,
  ROW_CLASS,
  SelectionCell,
  SkillNameCell,
  TokenPairCell,
  TrailingMenuCell,
} from "./SkillRowCells";
import { selectedRowClass } from "./skill-row-format";
import { SkillLocationCell } from "./SkillLocationCell";
import { DEFAULT_HARNESS_LIST, whereFacts } from "./skill-row-state";
import type { RowState } from "./skill-row-state";

interface SkillListRowProps {
  skill: InstalledSkill;
  /** This row's position in the grouped `rows` array - `aria-rowindex` and shift-click. */
  index: number;
  /** The virtualizer's absolute positioning within the row's group container. */
  style: CSSProperties;
  state: RowState | null;
  checked: boolean;
  /** Whether any row in the table is checked, for the always-visible selection gutter. */
  hasSelection: boolean;
  /** Whether this row is the skill currently open in the detail panel. */
  isOpenSkill: boolean;
  tabIndex: number;
  glyphSize: number;
  rowRef: (el: HTMLDivElement | null) => void;
  onOpen: () => void;
  onAct: (label: string) => void;
  onCheckedChange: (shiftKey: boolean) => void;
  onMenuOpenChange: (open: boolean) => void;
}

/** One skill row - `index` is its position in the grouped `rows` array, for shift-click and
 * `aria-rowindex`; `useRowCursor` (via `skill.name`) drives the roving `tabIndex` instead.
 * `style` is the virtualizer's absolute positioning within the row's group container. */
export function SkillListRow({
  skill,
  index,
  style,
  state,
  checked,
  hasSelection,
  isOpenSkill,
  tabIndex,
  glyphSize,
  rowRef,
  onOpen,
  onAct,
  onCheckedChange,
  onMenuOpenChange,
}: SkillListRowProps) {
  return (
    <div
      ref={rowRef}
      role="row"
      aria-rowindex={index + 1}
      aria-selected={checked}
      tabIndex={tabIndex}
      style={style}
      // `scroll-mt-7` (28px, `HEADER_HEIGHT`) keeps a row scrolled to by `scrollIntoView` from
      // surfacing under its group's sticky header.
      className={`${ROW_CLASS} scroll-mt-7 gap-x-3 px-3 [grid-template-columns:20px_var(--glyph-hit)_minmax(0,1fr)_160px_148px_104px] hover:bg-bg-secondary focus-visible:outline-2 focus-visible:outline-accent -outline-offset-2 ${selectedRowClass(
        isOpenSkill,
      )} ${skill.parked ? "text-text-tertiary" : ""}`}
      onClick={onOpen}
    >
      <div role="gridcell" className="contents">
        <SelectionCell
          skill={skill}
          checked={checked}
          visible={hasSelection}
          onCheckedChange={(_checked, eventDetails) => {
            // SAFETY: the underlying event is a pointer or keyboard event, both of which carry `shiftKey`.
            const shiftKey = (eventDetails.event as MouseEvent | KeyboardEvent).shiftKey;
            onCheckedChange(shiftKey);
          }}
        />
      </div>
      {/* Not `contents`: `LeadingCell` renders nothing for a healthy row, and a `contents`
          wrapper around no children drops out of the grid, shifting every column after it. */}
      <div role="gridcell" className="flex items-center justify-center">
        <LeadingCell
          skill={skill}
          state={state}
          glyphSize={glyphSize}
          onOpen={onOpen}
          onAct={onAct}
        />
      </div>
      <div role="gridcell" className="contents">
        <SkillNameCell skill={skill} />
      </div>
      <div role="gridcell" className="contents">
        <SkillLocationCell locations={whereFacts(skill, DEFAULT_HARNESS_LIST).locations} />
      </div>
      <div role="gridcell" className="contents">
        <HarnessStack skill={skill} harnessList={DEFAULT_HARNESS_LIST} />
      </div>
      <div role="gridcell" className="flex items-center justify-end gap-1">
        <TokenPairCell skill={skill} />
        <TrailingMenuCell
          skill={skill}
          state={state}
          glyphSize={glyphSize}
          visible={checked}
          onOpen={onOpen}
          onAct={onAct}
          onOpenChange={onMenuOpenChange}
          onToggleSelect={() => onCheckedChange(false)}
        />
      </div>
    </div>
  );
}
