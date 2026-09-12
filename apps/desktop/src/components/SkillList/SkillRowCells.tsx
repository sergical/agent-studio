// ============================================================================
// SkillRowCells - the shared cell vocabulary for a Stack row: the list
// header's cell class, the row frame, the name/invocation cell, the token
// pair cell and its sortable header, and the leading cell (checkbox in
// selection mode, otherwise the state glyph or a hover-only Ellipsis menu).
// ============================================================================

import type { ComponentProps, ReactNode } from "react";
import {
  ArrowDown,
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
import { Button } from "@skill-studio/ui";
import type { SortMode } from "../../lib/skill-list-sort";
import { CheckboxControl } from "../ui/CheckboxControl";
import { RichTooltip } from "../ui/RichTooltip";
import { TooltipControl } from "../ui/TooltipControl";
import { isDecision, rowState } from "./skill-row-state";
import type { RowLevel, RowState } from "./skill-row-state";

/** The shared hit box: bigger than the 14px glyph so the button is easy to land a click on. Sized
 * by the `--glyph-hit` CSS variable the row column template sets. */
const HIT_CLASS =
  "relative inline-flex size-(--glyph-hit) shrink-0 items-center justify-center rounded-sm transition-colors duration-150 hover:bg-bg-tertiary focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-border data-[popup-open]:bg-bg-tertiary";
import { SkillRowMenu } from "./SkillRowMenu";

/** The list header's cell. */
export const HEADER_CELL_CLASS =
  "h-9 text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase";

/** One row's frame: 36px, hairline below, no radius of its own. */
export const ROW_CLASS =
  "group grid h-9 w-full min-w-0 items-center rounded-none border-0 border-b border-border-subtle text-left last:border-b-0";

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

/** Which of the two Tokens numbers sorts and highlights the column: the prompt cost every
 * harness pays on every turn, or the full SKILL.md cost paid only on use. */
export type TokenSortKey = "prompt" | "full";

/** Rows in the table's sort order: `name` and `used` order as the Sort select says; `size`
 * (the "largest" option, i.e. by tokens) orders by whichever of the two token numbers
 * `tokenSort` has selected, ties broken by name. */
export function sortRows(
  skills: InstalledSkill[],
  sort: SortMode,
  statsBySkill: Map<string, SkillInvocationStats>,
  tokenSort: TokenSortKey,
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
    const value = (skill: InstalledSkill) =>
      tokenSort === "prompt" ? skill.description_tokens : skill.skill_md_tokens;
    rows.sort((a, b) => value(b) - value(a) || a.name.localeCompare(b.name));
  }
  return rows;
}

/** Both token numbers for one skill: the prompt cost first (the "name: description" line every
 * harness loads on every turn), then the full SKILL.md count. Whichever matches `sortKey` reads
 * as the column's real value; the other stays quiet. */
export function TokenPairCell({
  skill,
  sortKey,
}: {
  skill: InstalledSkill;
  sortKey: TokenSortKey;
}) {
  const promptText = skill.description_tokens > 0 ? formatTokens(skill.description_tokens) : "–";
  return (
    <RichTooltip content={<TokensTooltip skill={skill} />}>
      <span className="inline-flex items-baseline justify-end gap-1.5 tabular-nums text-small">
        <span className={sortKey === "prompt" ? "text-text-primary" : "text-text-tertiary"}>
          {promptText}
        </span>
        <span className={sortKey === "full" ? "text-text-primary" : "text-text-tertiary"}>
          {formatTokens(skill.skill_md_tokens)}
        </span>
      </span>
    </RichTooltip>
  );
}

const TOKEN_SORT_LABEL = { prompt: "Prompt", full: "Full" } satisfies Record<TokenSortKey, string>;
const TOKEN_SORT_TIP = {
  prompt: "Sort by prompt cost: the name + description line every harness loads on every turn",
  full: "Sort by SKILL.md size: loaded when the skill is used",
} satisfies Record<TokenSortKey, string>;

/** The Tokens header's two sort buttons, one per number `TokenPairCell` renders; clicking one
 * sets it as the sort key without a separate direction control (both sort descending). Only
 * changes row order while the table's own sort is "size" (largest); otherwise it just changes
 * which number reads bold. */
export function TokenPairHeader({
  sortKey,
  onSort,
}: {
  sortKey: TokenSortKey;
  onSort: (key: TokenSortKey) => void;
}) {
  return (
    <span className="inline-flex h-9 items-center justify-self-end gap-1">
      {(["prompt", "full"] as const).map((key) => {
        const active = key === sortKey;
        return (
          <TooltipControl key={key} content={TOKEN_SORT_TIP[key]}>
            <Button
              variant="ghost"
              size="xs"
              aria-pressed={active}
              className={`h-auto gap-0.5 px-1 py-0 text-caption font-medium tracking-[0.08em] uppercase ${
                active ? "text-text-primary" : "text-text-tertiary"
              }`}
              onClick={() => onSort(key)}
            >
              {TOKEN_SORT_LABEL[key]}
              {active && <ArrowDown size={10} aria-hidden />}
            </Button>
          </TooltipControl>
        );
      })}
    </span>
  );
}

type CheckboxChange = ComponentProps<typeof CheckboxControl>["onCheckedChange"];

interface LeadingCellProps {
  skill: InstalledSkill;
  selectionMode: boolean;
  checked: boolean;
  onCheckedChange: CheckboxChange;
  glyphSize: number;
  onOpen: () => void;
  onAct: (label: string) => void;
}

/** The row's leading cell: a checkbox sized to exactly fill `--glyph-hit` in select mode (zero
 * layout shift), otherwise the decision-state glyph (tooltipped) or the hover-only Ellipsis. */
export function LeadingCell({
  skill,
  selectionMode,
  checked,
  onCheckedChange,
  glyphSize,
  onOpen,
  onAct,
}: LeadingCellProps) {
  if (selectionMode) {
    return (
      <span
        className="inline-flex size-(--glyph-hit) shrink-0 items-center justify-center"
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

  const state: RowState | null = rowState(skill);
  if (state && isDecision(state)) {
    return (
      <SkillRowMenu
        skill={skill}
        state={state}
        trigger={
          <RichTooltip content={<StateTooltip state={state} />}>
            <span className={`inline-flex ${LEVEL_TEXT[state.level]}`}>
              {glyphFor(state, glyphSize)}
            </span>
          </RichTooltip>
        }
        triggerClassName={HIT_CLASS}
        triggerAriaLabel={state.label}
        onOpen={onOpen}
        onAct={onAct}
      />
    );
  }
  return (
    <SkillRowMenu
      skill={skill}
      state={state}
      trigger={<Ellipsis size={glyphSize} aria-hidden />}
      triggerClassName={`${HIT_CLASS} text-text-tertiary opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 data-[popup-open]:opacity-100 transition-opacity duration-150`}
      triggerAriaLabel={`Actions · ${skill.name}`}
      onOpen={onOpen}
      onAct={onAct}
    />
  );
}
