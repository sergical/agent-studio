// ============================================================================
// SkillRowCells - the shared cell vocabulary for a Stack row: the row frame,
// the name/invocation cell, the token pair cell, the selection checkbox
// gutter, and the leading (state glyph) and trailing (Ellipsis menu) cells.
// ============================================================================

import type { ComponentProps, ReactNode } from "react";
import {
  CircleAlert,
  CircleArrowDown,
  CirclePause,
  Ellipsis,
  Hourglass,
  OctagonAlert,
  Sparkles,
  TriangleAlert,
  UserRound,
} from "lucide-react";
import { formatTokens } from "@skill-studio/lib";
import type { InstalledSkill, SkillInvocationStats } from "@skill-studio/lib";
import type { SortMode } from "../../lib/skill-list-sort";
import { CheckboxControl } from "../ui/CheckboxControl";
import { RichTooltip } from "../ui/RichTooltip";
import { isDecision } from "./skill-row-state";
import type { RowLevel, RowState } from "./skill-row-state";

/** The shared hit box: bigger than the 14px glyph so the button is easy to land a click on. Sized
 * by the `--glyph-hit` CSS variable the row column template sets. */
const HIT_CLASS =
  "relative inline-flex size-(--glyph-hit) shrink-0 items-center justify-center rounded-sm transition-colors duration-150 hover:bg-bg-tertiary focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent data-[popup-open]:bg-bg-tertiary";
import { SkillRowMenu } from "./SkillRowMenu";

/** One row's frame: 32px, hairline below, no radius of its own. */
export const ROW_CLASS =
  "group grid h-8 w-full min-w-0 items-center rounded-none border-0 border-b border-border-subtle text-left last:border-b-0";

const LEVEL_TEXT = {
  error: "text-error",
  warning: "text-warning",
  info: "text-accent",
  muted: "text-text-tertiary",
} satisfies Record<RowLevel, string>;

/** Selected-row treatment shared by the row: an accent border, softer fill, and a left accent bar. */
export function selectedRowClass(selected: boolean): string {
  return selected
    ? "border-accent bg-accent-softer shadow-[inset_2px_0_0_var(--color-accent)]"
    : "";
}

function glyphFor(state: RowState, size = 14): ReactNode {
  switch (state.kind) {
    case "violation":
      return <OctagonAlert size={size} aria-hidden />;
    case "rollup":
      return state.level === "error" ? (
        <CircleAlert size={size} aria-hidden />
      ) : (
        <TriangleAlert size={size} aria-hidden />
      );
    case "trial":
      return <Hourglass size={size} aria-hidden />;
    case "update":
      return <CircleArrowDown size={size} aria-hidden />;
    case "parked":
      return <CirclePause size={size} aria-hidden />;
  }
}

/** The state glyph's tooltip: the label in its level colour, the detail line when there is one,
 * and a hint that the glyph opens a menu of fixes. */
function StateTooltip({ state }: { state: RowState }) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className={`font-medium ${LEVEL_TEXT[state.level]}`}>{state.label}</span>
      {state.detail && <span className="text-text-secondary">{state.detail}</span>}
      <span className="text-caption text-text-tertiary">Click for actions</span>
    </div>
  );
}

/** The Tokens cell's tooltip: the two costs a skill's Tokens column tracks - the prompt line
 * every harness loads on every turn, and the full SKILL.md paid only when the skill runs. */
function TokensTooltip({ skill }: { skill: InstalledSkill }) {
  return (
    <div className="grid grid-cols-1 gap-1 text-small">
      <div className="flex flex-col gap-0.5">
        <span className="tabular-nums text-text-primary">
          {skill.description_tokens.toLocaleString()} tokens · prompt cost
        </span>
        <span className="text-caption text-text-tertiary">
          name + description, loaded every turn by every harness that reaches it
        </span>
      </div>
      <div className="flex flex-col gap-0.5">
        <span className="tabular-nums text-text-primary">
          {skill.skill_md_tokens.toLocaleString()} tokens · full SKILL.md
        </span>
        <span className="text-caption text-text-tertiary">loaded when the skill is used</span>
      </div>
    </div>
  );
}

const INVOCATION_HEADER = {
  "user-only": "You run it",
  "model-only": "The model runs it",
} satisfies Record<"user-only" | "model-only", string>;

/** Whether a skill is user-invoked only or model-invoked only; "both" needs no mark, so this
 * renders nothing for the common case and the row stays quiet. The tooltip is one line - no
 * per-harness control table, no invoke syntax. */
function InvocationMark({ skill }: { skill: InstalledSkill }) {
  if (skill.invocation === "both") return null;
  const header = INVOCATION_HEADER[skill.invocation];
  return (
    <RichTooltip content={<span className="text-small">{header}</span>}>
      <span
        role="img"
        aria-label={header}
        className="inline-flex size-4 shrink-0 items-center justify-center text-text-tertiary"
      >
        {skill.invocation === "user-only" ? <UserRound size={12} /> : <Sparkles size={12} />}
      </span>
    </RichTooltip>
  );
}

/** The skill's name: a truncated label plus its `InvocationMark`. Descriptions live in the
 * detail panel, not the row. */
export function SkillNameCell({ skill }: { skill: InstalledSkill }) {
  return (
    <span className="flex min-w-0 items-center gap-1.5">
      <span className="min-w-0 truncate text-body text-text-primary">{skill.name}</span>
      <InvocationMark skill={skill} />
    </span>
  );
}

/** Rows in the table's sort order: `name` and `used` order as the Sort select says; `size`
 * (the "largest" option) orders by the full SKILL.md token count, ties broken by name. */
export function sortRows(
  skills: InstalledSkill[],
  sort: SortMode,
  statsBySkill: Map<string, SkillInvocationStats>,
): InstalledSkill[] {
  const rows = [...skills];
  if (sort === "name") {
    rows.sort((a, b) => a.name.localeCompare(b.name));
  } else if (sort === "used") {
    rows.sort(
      (a, b) =>
        (statsBySkill.get(b.name)?.last_30_days ?? 0) -
        (statsBySkill.get(a.name)?.last_30_days ?? 0),
    );
  } else {
    rows.sort((a, b) => b.skill_md_tokens - a.skill_md_tokens || a.name.localeCompare(b.name));
  }
  return rows;
}

/** Both token numbers for one skill: the prompt cost first (the "name: description" line every
 * harness loads on every turn), then the full SKILL.md count, which is the one "Largest" sorts
 * by and so the one that reads bold. */
export function TokenPairCell({ skill }: { skill: InstalledSkill }) {
  const promptText = skill.description_tokens > 0 ? formatTokens(skill.description_tokens) : "–";
  return (
    <RichTooltip content={<TokensTooltip skill={skill} />}>
      <span className="inline-flex items-baseline justify-end gap-1.5 tabular-nums text-small">
        <span className="text-text-tertiary">{promptText}</span>
        <span className="text-text-primary">{formatTokens(skill.skill_md_tokens)}</span>
      </span>
    </RichTooltip>
  );
}

type CheckboxChange = ComponentProps<typeof CheckboxControl>["onCheckedChange"];

/** The state glyph, tooltipped, with an always-present `sr-only` name so the severity reads
 * without a hover even though the icon itself is `aria-hidden`. `null` when the skill has
 * nothing to say - the caller's cell renders empty rather than a placeholder. */
export function RowGlyph({ state, size = 14 }: { state: RowState | null; size?: number }) {
  if (!state) return null;
  return (
    <RichTooltip content={<StateTooltip state={state} />}>
      <span className={`inline-flex items-center gap-1 ${LEVEL_TEXT[state.level]}`}>
        {glyphFor(state, size)}
        <span className="sr-only">{state.label}</span>
      </span>
    </RichTooltip>
  );
}

interface LeadingCellProps {
  skill: InstalledSkill;
  state: RowState | null;
  glyphSize: number;
  onOpen: () => void;
  onAct: (label: string) => void;
}

/** The row's leading cell: the decision-state glyph behind a fixes menu, or nothing - the
 * hover-only Ellipsis lives in `TrailingMenuCell` instead, since selection now has its own
 * gutter column. */
export function LeadingCell({ skill, state, glyphSize, onOpen, onAct }: LeadingCellProps) {
  if (!isDecision(state)) return null;
  return (
    <SkillRowMenu
      skill={skill}
      state={state}
      trigger={<RowGlyph state={state} size={glyphSize} />}
      triggerClassName={HIT_CLASS}
      triggerAriaLabel={state?.label}
      onOpen={onOpen}
      onAct={onAct}
    />
  );
}

interface TrailingMenuCellProps {
  skill: InstalledSkill;
  state: RowState | null;
  glyphSize: number;
  visible: boolean;
  onOpen: () => void;
  onAct: (label: string) => void;
}

/** The row's trailing Ellipsis menu: revealed on row hover/focus-within, or held visible while
 * the row is checked (`visible`) or its own popup is open. No opacity transition. */
export function TrailingMenuCell({
  skill,
  state,
  glyphSize,
  visible,
  onOpen,
  onAct,
}: TrailingMenuCellProps) {
  return (
    <SkillRowMenu
      skill={skill}
      state={state}
      trigger={<Ellipsis size={glyphSize} aria-hidden />}
      triggerClassName={`${HIT_CLASS} text-text-tertiary data-[popup-open]:opacity-100 ${
        visible ? "opacity-100" : "opacity-0 group-hover:opacity-100 group-focus-within:opacity-100"
      }`}
      triggerAriaLabel={`Actions · ${skill.name}`}
      onOpen={onOpen}
      onAct={onAct}
    />
  );
}

interface SelectionCellProps {
  skill: InstalledSkill;
  checked: boolean;
  visible: boolean;
  onCheckedChange: CheckboxChange;
}

/** The 20px checkbox gutter: `opacity-0` until the row is hovered, focus-within, checked, or any
 * row in the table is checked (`visible`). No transition. */
export function SelectionCell({ skill, checked, visible, onCheckedChange }: SelectionCellProps) {
  return (
    <span
      className={`inline-flex w-5 shrink-0 items-center justify-center ${
        checked || visible
          ? "opacity-100"
          : "opacity-0 group-hover:opacity-100 group-focus-within:opacity-100"
      }`}
      onClick={(e) => e.stopPropagation()}
    >
      <CheckboxControl
        checked={checked}
        onCheckedChange={onCheckedChange}
        ariaLabel={`Select ${skill.name}`}
      />
    </span>
  );
}
