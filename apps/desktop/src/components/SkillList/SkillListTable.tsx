// ============================================================================
// SkillListTable - Searchable, sortable skill rows, rendered by SkillsView
// with whatever it has already filtered down (scope, harness, source, issue)
// ============================================================================

import { useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { formatTokens, pluginLabelForSkill } from "@skill-studio/lib";
import type { InstalledSkill, PackMember, SkillInvocationStats } from "@skill-studio/lib";
import { isFeatureEnabled } from "../../lib/feature-flags";
import type { SortMode } from "../../lib/skill-list-sort";
import { useAppStore } from "../../store/appStore";
import { PackNamePrompt } from "../Packs/PackNamePrompt";
import { CheckboxControl } from "../ui/CheckboxControl";
import { SkillLocationCell } from "./SkillLocationCell";
import { TooltipControl } from "../ui/TooltipControl";

/** "User only" / "Model only" chip label, `null` for the default "both" policy. */
function invocationChipLabel(invocation: InstalledSkill["invocation"]): string | null {
  if (invocation === "user-only") return "User only";
  if (invocation === "model-only") return "Model only";
  return null;
}

interface SkillListTableProps {
  skills: InstalledSkill[];
  stats: SkillInvocationStats[];
  /** Sort order - the Sort select lives in `SkillListFilterBar`; the search box there narrows `skills` before it reaches this table. */
  sort: SortMode;
  onSelectSkill: (name: string, deploymentPath?: string) => void;
  selectedSkillName?: string | null;
  /** Resolves which deployment a row's click should open in the detail drawer, when the caller knows it. */
  deploymentPathForSkill?: (skill: InstalledSkill) => string | undefined;
  /** False when the caller's underlying list (before any filter) is empty, for the right empty state. */
  hasAnySkills?: boolean;
  /** Resets the caller's filter, for the "No skills match" empty state. */
  onClearFilters?: () => void;
  /** Opens the add-skill sheet, for the "You haven't added a skill yet" empty state. */
  onAddSkill?: () => void;
}

/**
 * Toolbar (Select, filter, sort) above a list of skill rows: name, one-line
 * description, location chips, 30-day use count and SKILL.md token count.
 * Clicking a row opens the skill, unless selection mode is on, where it
 * toggles the row instead. Selection and packs sit behind the "skill-packs"
 * feature flag.
 */
export function SkillListTable({
  skills,
  stats,
  sort,
  onSelectSkill,
  selectedSkillName = null,
  deploymentPathForSkill,
  hasAnySkills = true,
  onClearFilters,
  onAddSkill,
}: SkillListTableProps) {
  const [showPackPrompt, setShowPackPrompt] = useState(false);
  const packsEnabled = isFeatureEnabled("skill-packs");
  const statsBySkill = new Map(stats.map((s) => [s.skill, s]));
  const selectedPaths = useAppStore((state) => state.selectedSkillPaths);
  const toggleSkillSelection = useAppStore((state) => state.toggleSkillSelection);
  const clearSkillSelection = useAppStore((state) => state.clearSkillSelection);
  const selectSkills = useAppStore((state) => state.selectSkills);
  const selectionMode = useAppStore((state) => state.selectionMode);
  const enterSelectionMode = useAppStore((state) => state.enterSelectionMode);
  const exitSelectionMode = useAppStore((state) => state.exitSelectionMode);
  /** Index of the last row checked by click (not shift-click), for shift-click range-select. */
  const lastCheckedIndexRef = useRef<number | null>(null);
  /** Whether Shift was held for the row checkbox click in progress - CheckboxControl's onCheckedChange doesn't carry the native event, so this is captured a beat earlier, on pointerdown. */
  const shiftKeyRef = useRef(false);

  /** The deployment path this row's selection checkbox stands for. */
  const rowPath = (skill: InstalledSkill): string | undefined =>
    deploymentPathForSkill?.(skill) ?? skill.deployments[0]?.path;

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
    rows.sort((a, b) => b.skill_md_tokens - a.skill_md_tokens);
  }

  const allVisibleSelected =
    rows.length > 0 && rows.every((s) => selectedPaths.has(rowPath(s) ?? ""));

  /** Checkbox click for one row - shift-click selects every row between it and the last clicked one, in visible order. */
  function handleRowCheckboxClick(index: number, shiftKey: boolean) {
    if (shiftKey && lastCheckedIndexRef.current !== null) {
      const [from, to] = [lastCheckedIndexRef.current, index].sort((a, b) => a - b);
      const range = rows.slice(from, to + 1).map((s) => rowPath(s));
      const next = new Set(selectedPaths);
      range.forEach((path) => path && next.add(path));
      selectSkills([...next]);
    } else {
      const path = rowPath(rows[index]);
      if (path) toggleSkillSelection(path);
    }
    lastCheckedIndexRef.current = index;
  }

  function handleHeaderCheckboxChange() {
    const next = new Set(selectedPaths);
    if (allVisibleSelected) {
      rows.forEach((s) => {
        const path = rowPath(s);
        if (path) next.delete(path);
      });
    } else {
      rows.forEach((s) => {
        const path = rowPath(s);
        if (path) next.add(path);
      });
    }
    selectSkills([...next]);
  }

  function handleRowClick(index: number, skill: InstalledSkill) {
    if (selectionMode) {
      handleRowCheckboxClick(index, false);
      return;
    }
    onSelectSkill(skill.name, deploymentPathForSkill?.(skill));
  }

  /** Escape exits selection mode, mirroring the selection bar's Cancel button. */
  function handleTableKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    if (e.key === "Escape" && selectionMode) {
      e.preventDefault();
      exitSelectionMode();
    }
  }

  return (
    <div className="flex flex-col gap-3" onKeyDown={handleTableKeyDown}>
      {(selectionMode || packsEnabled) && (
        <div className="flex items-center gap-2">
          {selectionMode ? (
            <>
              {/* The header checkbox sits in the same w-11 rail as the row
                  checkboxes below, so entering selection mode doesn't shift it. */}
              <div className="flex w-11 shrink-0 items-center justify-center [&_.checkbox-control-root]:before:absolute [&_.checkbox-control-root]:before:-inset-3 [&_.checkbox-control-root]:before:content-['']">
                <CheckboxControl
                  checked={allVisibleSelected}
                  onCheckedChange={handleHeaderCheckboxChange}
                  disabled={rows.length === 0}
                  ariaLabel="Select all visible skills"
                />
              </div>
              <span className="text-small text-text-secondary">{selectedPaths.size} selected</span>
              <button
                className="ml-auto cursor-pointer rounded-sm bg-accent px-3 py-1.5 text-small font-semibold text-text-on-accent disabled:cursor-not-allowed disabled:opacity-50"
                onClick={() => setShowPackPrompt(true)}
                disabled={selectedPaths.size === 0}
              >
                Create pack
              </button>
              <button
                className="cursor-pointer rounded-sm border border-border bg-transparent px-3 py-1.5 text-small text-text-tertiary"
                onClick={exitSelectionMode}
              >
                Cancel
              </button>
            </>
          ) : (
            <button
              className="h-(--control-height) shrink-0 cursor-pointer rounded-sm border border-border bg-transparent px-3 text-body text-text-secondary transition-colors hover:bg-bg-hover hover:text-text-primary"
              onClick={enterSelectionMode}
            >
              Select
            </button>
          )}
        </div>
      )}

      {rows.length === 0 ? (
        <div className="flex flex-col items-start gap-2 text-pretty text-small text-text-tertiary">
          {hasAnySkills ? (
            <>
              <p className="m-0">No skills match</p>
              {onClearFilters && (
                <button
                  className="h-8 cursor-pointer rounded-sm border border-border bg-bg-tertiary px-3 text-small text-text-primary transition-colors hover:bg-bg-hover"
                  onClick={onClearFilters}
                >
                  Clear filters
                </button>
              )}
            </>
          ) : (
            <>
              <p className="m-0">You haven't added a skill yet</p>
              {onAddSkill && (
                <button
                  className="h-8 cursor-pointer rounded-sm border border-border bg-bg-tertiary px-3 text-small text-text-primary transition-colors hover:bg-bg-hover"
                  onClick={onAddSkill}
                >
                  Add skill
                </button>
              )}
            </>
          )}
        </div>
      ) : (
        <div className="flex flex-col gap-1.5">
          <div className="flex items-stretch">
            {selectionMode && <span className="w-11 shrink-0" />}
            <div className="grid min-w-0 flex-1 items-center gap-3 border border-transparent px-3 [grid-template-columns:minmax(0,1.2fr)_minmax(0,1.8fr)_140px_48px_64px]">
              <span />
              <span />
              <span />
              <TooltipControl content="Invocations in the last 30 days">
                <span className="text-right text-caption text-text-tertiary">Uses</span>
              </TooltipControl>
              <TooltipControl content="SKILL.md tokens the model reads">
                <span className="text-right text-caption text-text-tertiary">Tokens</span>
              </TooltipControl>
            </div>
          </div>
          {rows.map((skill, index) => {
            const stat = statsBySkill.get(skill.name);
            const selected = skill.name === selectedSkillName;
            return (
              <div key={skill.name} className="flex items-stretch">
                {selectionMode && (
                  <label
                    className="flex w-11 shrink-0 cursor-pointer items-center justify-center"
                    aria-label={`Select ${skill.name}`}
                    onPointerDownCapture={(e) => {
                      shiftKeyRef.current = e.shiftKey;
                    }}
                  >
                    <CheckboxControl
                      checked={selectedPaths.has(rowPath(skill) ?? "")}
                      onCheckedChange={() => {
                        handleRowCheckboxClick(index, shiftKeyRef.current);
                        // Consumed: a later keyboard toggle must not reuse a pointer's Shift.
                        shiftKeyRef.current = false;
                      }}
                    />
                  </label>
                )}
                <button
                  className={`grid h-11 min-w-0 flex-1 cursor-pointer items-center gap-3 overflow-hidden rounded-md border border-border bg-bg-secondary px-3 text-left transition-colors [grid-template-columns:minmax(0,1.2fr)_minmax(0,1.8fr)_140px_48px_64px] hover:bg-bg-hover active:bg-bg-active ${
                    selected
                      ? "border-accent bg-accent-softer shadow-[inset_2px_0_0_var(--color-accent)]"
                      : ""
                  }`}
                  onClick={(e) => {
                    if (selectionMode && e.shiftKey) {
                      handleRowCheckboxClick(index, true);
                      return;
                    }
                    handleRowClick(index, skill);
                  }}
                >
                  <span className="flex min-w-0 items-center gap-1.5">
                    <span
                      className="truncate text-body font-semibold text-text-primary"
                      title={skill.name}
                    >
                      {skill.name}
                    </span>
                    {skill.has_update && (
                      <span className="inline-flex shrink-0 rounded-sm bg-accent-soft px-1.5 py-px text-caption font-semibold text-accent">
                        Update
                      </span>
                    )}
                    {pluginLabelForSkill(skill) && (
                      <span className="inline-flex shrink-0 rounded-sm bg-bg-tertiary px-1.5 py-px text-caption font-semibold text-text-secondary">
                        plugin · {pluginLabelForSkill(skill)}
                      </span>
                    )}
                    {skill.parked && (
                      <span className="inline-flex shrink-0 rounded-sm bg-bg-tertiary px-1.5 py-px text-caption font-semibold text-text-secondary">
                        Parked
                      </span>
                    )}
                    {invocationChipLabel(skill.invocation) && (
                      <span className="inline-flex shrink-0 rounded-sm bg-bg-tertiary px-1.5 py-px text-caption font-semibold text-text-secondary">
                        {invocationChipLabel(skill.invocation)}
                      </span>
                    )}
                  </span>
                  <span
                    className="truncate text-small text-text-tertiary"
                    title={skill.description ?? ""}
                  >
                    {skill.description ?? ""}
                  </span>
                  <SkillLocationCell skill={skill} />
                  <span
                    className={`whitespace-nowrap text-right text-small tabular-nums ${
                      (stat?.last_30_days ?? 0) === 0
                        ? "text-text-quaternary"
                        : "text-text-tertiary"
                    }`}
                  >
                    {stat?.last_30_days ?? 0}
                  </span>
                  <span className="whitespace-nowrap text-right text-small tabular-nums text-text-tertiary">
                    {formatTokens(skill.skill_md_tokens)}
                  </span>
                </button>
              </div>
            );
          })}
        </div>
      )}

      {showPackPrompt && (
        <PackNamePrompt
          members={skills.reduce<PackMember[]>((members, s) => {
            const path = rowPath(s);
            if (path !== undefined && selectedPaths.has(path)) members.push({ name: s.name, path });
            return members;
          }, [])}
          onClose={() => setShowPackPrompt(false)}
          onCreated={() => {
            setShowPackPrompt(false);
            clearSkillSelection();
          }}
        />
      )}
    </div>
  );
}
