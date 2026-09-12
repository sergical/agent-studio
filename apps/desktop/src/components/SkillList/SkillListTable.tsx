// ============================================================================
// SkillListTable - Searchable, sortable skill rows, rendered by SkillsView
// with whatever it has already filtered down (scope, harness, source, issue)
// ============================================================================

import { useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent } from "react";
import type { InstalledSkill, PackMember, SkillInvocationStats } from "@skill-studio/lib";
import { Button } from "@skill-studio/ui";
import { isFeatureEnabled } from "../../lib/feature-flags";
import { parkSkill, unparkSkill } from "../../lib/skill-api";
import { lifecycleTargetForPark } from "../../lib/skill-lifecycle-target";
import type { SortMode } from "../../lib/skill-list-sort";
import { useAppStore } from "../../store/appStore";
import { PackNamePrompt } from "../Packs/PackNamePrompt";
import { CheckboxControl } from "../ui/CheckboxControl";
import { HarnessStack } from "./HarnessStack";
import {
  HEADER_CELL_CLASS,
  LeadingCell,
  ROW_CLASS,
  selectedRowClass,
  SkillNameCell,
  sortRows,
  TokenPairCell,
  TokenPairHeader,
} from "./SkillRowCells";
import type { TokenSortKey } from "./SkillRowCells";
import { SkillLocationCell } from "./SkillLocationCell";
import { DEFAULT_HARNESS_LIST, whereFacts } from "./skill-row-state";

/** The row's leading-glyph hit box, and the icon it holds - fixed sizes. */
const GLYPH_HIT = 28;
const GLYPH_SIZE = 14;

/** Every skill row's column template: leading glyph, name, location, harnesses, tokens. */
const COLUMNS = "[grid-template-columns:var(--glyph-hit)_minmax(0,1fr)_160px_148px_104px]";

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
 * Toolbar (Select, filter, sort) above a list of skill rows: the state
 * glyph, name, disk location, harness stack, and token pair. Clicking a row
 * opens the skill, unless selection mode is on, where it toggles the row
 * instead. Selection and packs sit behind the "skill-packs" feature flag.
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
  const [tokenSort, setTokenSort] = useState<TokenSortKey>("full");
  const packsEnabled = isFeatureEnabled("skill-packs");
  const statsBySkill = new Map(stats.map((s) => [s.skill, s]));
  const selectedPaths = useAppStore((state) => state.selectedSkillPaths);
  const toggleSkillSelection = useAppStore((state) => state.toggleSkillSelection);
  const clearSkillSelection = useAppStore((state) => state.clearSkillSelection);
  const selectSkills = useAppStore((state) => state.selectSkills);
  const selectionMode = useAppStore((state) => state.selectionMode);
  const enterSelectionMode = useAppStore((state) => state.enterSelectionMode);
  const exitSelectionMode = useAppStore((state) => state.exitSelectionMode);
  const addToast = useAppStore((state) => state.addToast);
  /** Index of the last row checked by click (not shift-click), for shift-click range-select. */
  const lastCheckedIndexRef = useRef<number | null>(null);

  /** The deployment path this row's selection checkbox stands for. */
  const rowPath = (skill: InstalledSkill): string | undefined =>
    deploymentPathForSkill?.(skill) ?? skill.deployments[0]?.path;

  const rows = sortRows(skills, sort, statsBySkill, tokenSort);

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

  /** Park/Unpark act on the deployment target `HomeView` uses; every other fix (Fix YAML, Fix
   * link, Compare, Convert, Keep, Pull latest) opens the skill's own detail, since those flows
   * live there. */
  async function handleAct(label: string, skill: InstalledSkill) {
    if (label !== "Park" && label !== "Unpark") {
      onSelectSkill(skill.name, deploymentPathForSkill?.(skill));
      return;
    }
    try {
      if (label === "Park") await parkSkill(lifecycleTargetForPark(skill));
      else await unparkSkill(lifecycleTargetForPark(skill));
      addToast({
        type: "success",
        title: label === "Park" ? `Parked ${skill.name}` : `Unparked ${skill.name}`,
      });
    } catch (err) {
      addToast({
        type: "error",
        title: label === "Park" ? "Couldn't park skill" : "Couldn't unpark skill",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  }

  return (
    <div
      className="flex flex-col gap-3"
      style={
        // SAFETY: `--glyph-hit` is a custom property, not a known CSSProperties key; React
        // passes it through to the style attribute as-is.
        { "--glyph-hit": `${GLYPH_HIT}px` } as CSSProperties
      }
      onKeyDown={handleTableKeyDown}
    >
      {(selectionMode || packsEnabled) && (
        <div className="flex items-center gap-2">
          {selectionMode ? (
            <>
              <span className="text-small text-text-secondary">{selectedPaths.size} selected</span>
              <Button
                size="sm"
                className="ml-auto rounded-sm bg-accent text-text-on-accent"
                onClick={() => setShowPackPrompt(true)}
                disabled={selectedPaths.size === 0}
              >
                Create pack
              </Button>
              <Button
                variant="outline"
                size="sm"
                className="rounded-sm text-text-tertiary"
                onClick={exitSelectionMode}
              >
                Cancel
              </Button>
            </>
          ) : (
            <Button
              variant="outline"
              className="h-(--control-height) shrink-0 rounded-sm px-3 text-body text-text-secondary"
              onClick={enterSelectionMode}
            >
              Select
            </Button>
          )}
        </div>
      )}

      {rows.length === 0 ? (
        <div className="flex flex-col items-start gap-2 text-pretty text-small text-text-tertiary">
          {hasAnySkills ? (
            <>
              <p className="m-0">No skills match</p>
              {onClearFilters && (
                <Button
                  variant="secondary"
                  className="rounded-sm border border-border text-text-primary"
                  onClick={onClearFilters}
                >
                  Clear filters
                </Button>
              )}
            </>
          ) : (
            <>
              <p className="m-0">You haven't added a skill yet</p>
              {onAddSkill && (
                <Button
                  variant="secondary"
                  className="rounded-sm border border-border text-text-primary"
                  onClick={onAddSkill}
                >
                  Add skill
                </Button>
              )}
            </>
          )}
        </div>
      ) : (
        <div className="overflow-hidden rounded-md border border-border">
          <div className="flex items-center border-b border-border-subtle bg-bg-secondary px-3">
            <div className={`grid flex-1 items-center gap-x-3 ${HEADER_CELL_CLASS} ${COLUMNS}`}>
              {selectionMode ? (
                <CheckboxControl
                  checked={allVisibleSelected}
                  onCheckedChange={handleHeaderCheckboxChange}
                  disabled={rows.length === 0}
                  ariaLabel="Select all visible skills"
                />
              ) : (
                <span aria-hidden />
              )}
              <span>Skill</span>
              <span>Location</span>
              <span>Harnesses</span>
              <TokenPairHeader sortKey={tokenSort} onSort={setTokenSort} />
            </div>
          </div>
          {rows.map((skill, index) => {
            const selected = skill.name === selectedSkillName;
            return (
              <div
                key={skill.name}
                className={`${ROW_CLASS} gap-x-3 px-3 ${COLUMNS} hover:bg-bg-secondary ${selectedRowClass(
                  selected,
                )} ${skill.parked ? "text-text-tertiary" : ""}`}
                onClick={(e) => {
                  if (selectionMode && e.shiftKey) {
                    handleRowCheckboxClick(index, true);
                    return;
                  }
                  handleRowClick(index, skill);
                }}
              >
                <LeadingCell
                  skill={skill}
                  selectionMode={selectionMode}
                  checked={selectedPaths.has(rowPath(skill) ?? "")}
                  onCheckedChange={(_checked, eventDetails) => {
                    // SAFETY: the underlying event is a pointer or keyboard event, both of which carry `shiftKey`.
                    const shiftKey = (eventDetails.event as MouseEvent | KeyboardEvent).shiftKey;
                    handleRowCheckboxClick(index, shiftKey);
                  }}
                  glyphSize={GLYPH_SIZE}
                  onOpen={() => onSelectSkill(skill.name, deploymentPathForSkill?.(skill))}
                  onAct={(label) => void handleAct(label, skill)}
                />
                <SkillNameCell skill={skill} />
                <SkillLocationCell locations={whereFacts(skill, DEFAULT_HARNESS_LIST).locations} />
                <HarnessStack skill={skill} harnessList={DEFAULT_HARNESS_LIST} />
                <TokenPairCell skill={skill} sortKey={tokenSort} />
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
