// ============================================================================
// SkillListTable - Searchable, sortable skill rows, rendered by SkillsView
// with whatever it has already filtered down (scope, harness, source, issue)
// ============================================================================

import { useEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import {
  defaultRangeExtractor,
  observeElementRect as observeDefaultElementRect,
  useVirtualizer,
} from "@tanstack/react-virtual";
import type { InstalledSkill, PackMember, SkillInvocationStats } from "@skill-studio/lib";
import { Button, Collapsible } from "@skill-studio/ui";
import { isFeatureEnabled } from "../../lib/feature-flags";
import { parkSkill, unparkSkill } from "../../lib/skill-api";
import { lifecycleTargetForPark } from "../../lib/skill-lifecycle-target";
import type { SortMode } from "../../lib/skill-list-sort";
import { useRowCursor, useRowCursorWindowEntry } from "../../hooks/useRowCursor";
import { useAppStore } from "../../store/appStore";
import { RichTooltipScope } from "../ui/RichTooltip";
import { PackNamePrompt } from "../Packs/PackNamePrompt";
import { GroupHead } from "./GroupHead";
import { HarnessStack } from "./HarnessStack";
import {
  LeadingCell,
  ROW_CLASS,
  SelectionCell,
  SkillNameCell,
  TokenPairCell,
  TrailingMenuCell,
} from "./SkillRowCells";
import { selectedRowClass, sortRows } from "./skill-row-format";
import { SkillLocationCell } from "./SkillLocationCell";
import { SkillRowMenuScope } from "./SkillRowMenu";
import { DEFAULT_HARNESS_LIST, rowGroup, rowState, whereFacts } from "./skill-row-state";
import type { RowGroup, RowState } from "./skill-row-state";

/** The row's leading-glyph hit box, and the icon it holds - fixed sizes. */
const GLYPH_HIT = 28;
const GLYPH_SIZE = 14;

/** Every skill row's column template: a checkbox gutter, leading glyph, name, location,
 * harnesses, tokens (the trailing Ellipsis menu lives inside that last cell). */
const COLUMNS = "[grid-template-columns:20px_var(--glyph-hit)_minmax(0,1fr)_160px_148px_104px]";

/** The three state groups, in display order, and their header labels. */
const GROUP_ORDER: RowGroup[] = ["attention", "healthy", "parked"];
const GROUP_LABEL = {
  attention: "Needs attention",
  healthy: "Healthy",
  parked: "Parked",
};

/** Fixed row/header sizes for the virtualizer - `GroupHead`'s `h-7` and `ROW_CLASS`'s `h-8`, both
 * border-box so their border doesn't add to these. No `measureElement`: every row and header is
 * the same height. */
const HEADER_HEIGHT = 28;
const ROW_HEIGHT = 32;
/** Overscan past the visible range, in items. */
const OVERSCAN = 8;

/** One row of the flat, virtualized item list built from the three state-group buckets: a
 * group's header, then (only while the group is open) one entry per skill in it. `index` is the
 * row's position in the grouped `rows` array - what `aria-rowindex` and shift-click use - which
 * stays stable even for a skill inside a collapsed group. */
type ListItem =
  | { kind: "header"; group: RowGroup }
  | { kind: "row"; group: RowGroup; skill: InstalledSkill; index: number };

/** A group's offset and total size in `ListItem[]`'s flat coordinate space - see
 * `buildListItems`. */
type GroupMeta = Record<RowGroup, { start: number; height: number }>;

/** `buildListItems`'s result: the flat item list plus each group's placement in it. */
interface ListItems {
  items: ListItem[];
  groupMeta: GroupMeta;
}

/** `useVirtualizer`'s instance holds mutable methods (`scrollToIndex`, `getVirtualItems`) that
 * can't be memoized - calling it directly makes React Compiler bail out of memoizing the whole
 * component. Isolating the call in its own hook, opted out of compilation, keeps that limitation
 * local instead. */
function useSkillListVirtualizer(
  options: Parameters<typeof useVirtualizer<HTMLElement, HTMLDivElement>>[0],
) {
  "use no memo";
  // oxlint-disable-next-line react/incompatible-library -- this hook is opted out of compilation above.
  return useVirtualizer(options); // react-doctor-disable-line react-hooks-js/incompatible-library
}

/** Builds the flat `ListItem` list `useSkillListVirtualizer` measures, plus each group's
 * `start`/`height` in that same flat coordinate space, for positioning its rows and sizing its
 * container. Pulled out of the component so its loop doesn't count against its control-flow
 * complexity. */
function buildListItems(
  buckets: Record<RowGroup, InstalledSkill[]>,
  collapsedGroups: Set<RowGroup>,
): ListItems {
  const items: ListItem[] = [];
  // SAFETY: every group below is set exactly once, before the caller reads it.
  const groupMeta = {
    attention: { start: 0, height: 0 },
    healthy: { start: 0, height: 0 },
    parked: { start: 0, height: 0 },
  } satisfies GroupMeta;
  let cumulativeSize = 0;
  let rowIndex = 0;
  for (const group of GROUP_ORDER) {
    const groupSkills = buckets[group];
    if (groupSkills.length === 0) continue;
    const open = !collapsedGroups.has(group);
    const height = HEADER_HEIGHT + (open ? groupSkills.length * ROW_HEIGHT : 0);
    groupMeta[group] = { start: cumulativeSize, height };
    items.push({ kind: "header", group });
    if (open) {
      groupSkills.forEach((skill, i) =>
        items.push({ kind: "row", group, skill, index: rowIndex + i }),
      );
    }
    rowIndex += groupSkills.length;
    cumulativeSize += height;
  }
  return { items, groupMeta };
}

interface SkillListTableProps {
  skills: InstalledSkill[];
  stats: SkillInvocationStats[];
  /** Sort order - the Sort select lives in `SkillListFilterBar`; the search box there narrows `skills` before it reaches this table. It applies inside each state group, not across the whole list. */
  sort: SortMode;
  onSelectSkill: (name: string, deploymentPath?: string) => void;
  selectedSkillName?: string | null;
  /** The skill whose page Escape/back just closed, so the cursor returns to that row instead of
   * resetting to the first one. */
  initialCursorSkillName?: string | null;
  /** Whether Skills is the view on screen right now - `false` while it's kept mounted but hidden
   * behind an open skill's page, so its window-level j/k/arrow shortcuts don't fire for a list the
   * user can't see. */
  active: boolean;
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
 * A grid of skill rows split into three sticky-headed state groups (Needs
 * attention, Healthy, Parked): the state glyph, name, disk location, harness
 * stack, and token pair. Checking a row's checkbox selects it - no separate
 * selection mode - and the action bar (Create pack, Cancel) shows once
 * anything is checked. Packs sit behind the "skill-packs" feature flag.
 */
export function SkillListTable({
  skills,
  stats,
  sort,
  onSelectSkill,
  selectedSkillName: selectedSkillNameProp,
  initialCursorSkillName: initialCursorSkillNameProp,
  active,
  deploymentPathForSkill,
  hasAnySkills: hasAnySkillsProp,
  onClearFilters,
  onAddSkill,
}: SkillListTableProps) {
  const selectedSkillName = selectedSkillNameProp ?? null;
  const initialCursorSkillName = initialCursorSkillNameProp ?? null;
  const hasAnySkills = hasAnySkillsProp ?? true;
  const [showPackPrompt, setShowPackPrompt] = useState(false);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<RowGroup>>(() => new Set());
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
  /** The scroll container the virtualizer measures against - the nearest ancestor with its own
   * scrollbar (`PageShell`'s content area), found by walking up from the grid element. */
  const [scrollElement, setScrollElement] = useState<HTMLElement | null>(null);
  const gridElRef = useRef<HTMLDivElement | null>(null);
  /** How far the grid sits below the top of the scroll container's content - `useVirtualizer`'s
   * `scrollMargin`, recomputed whenever content above the grid (filter chips, a scan banner)
   * resizes. */
  const [scrollMargin, setScrollMargin] = useState(0);
  /** The roving cursor's key, mirrored here so the virtualizer's `rangeExtractor` (a plain
   * callback, not part of render) can always keep that row's item in range without depending on
   * `useRowCursor`'s return value before it exists. */
  const cursorKeyRef = useRef<string | null>(null);

  /** The deployment path this row's selection checkbox stands for. */
  const rowPath = (skill: InstalledSkill): string | undefined =>
    deploymentPathForSkill?.(skill) ?? skill.deployments[0]?.path;

  const sorted = sortRows(skills, sort, statsBySkill);
  const statesBySkill = new Map<string, RowState | null>();
  // SAFETY: each bucket starts empty; the loop below only ever pushes `InstalledSkill` values into it.
  const buckets = {
    attention: [] as InstalledSkill[],
    healthy: [] as InstalledSkill[],
    parked: [] as InstalledSkill[],
  };
  for (const skill of sorted) {
    const state = rowState(skill);
    statesBySkill.set(skill.name, state);
    buckets[rowGroup(skill, state)].push(skill);
  }
  /** The grouped display order: `rowPath`/index below refer to this array, not `sorted`. */
  const rows = [...buckets.attention, ...buckets.healthy, ...buckets.parked];
  /** Row keys `useRowCursor` navigates, in rendered order - a collapsed group's rows drop out. */
  const visibleKeys = GROUP_ORDER.flatMap((group) =>
    collapsedGroups.has(group) ? [] : buckets[group].map((skill) => skill.name),
  );

  /** The flat item list the virtualizer measures, and each group's offset/size in that same
   * coordinate space - see `buildListItems`. */
  const { items, groupMeta } = buildListItems(buckets, collapsedGroups);

  const virtualizer = useSkillListVirtualizer({
    count: items.length,
    getScrollElement: () => scrollElement,
    estimateSize: (index) => (items[index].kind === "header" ? HEADER_HEIGHT : ROW_HEIGHT),
    overscan: OVERSCAN,
    scrollMargin,
    // Keeps a row scrolled to (Home/End, j/k past the rendered range) from surfacing under its
    // group's sticky header.
    scrollPaddingStart: HEADER_HEIGHT,
    // The roving cursor row is always in the range, mounted, so a Tab into the grid always lands
    // on it - `cursorKeyRef` (not `useRowCursor`'s return) since this callback outlives any one
    // render and `useRowCursor` itself isn't in scope yet at this point.
    rangeExtractor: (range) => {
      const base = defaultRangeExtractor(range);
      const cursorKeyValue = cursorKeyRef.current;
      const cursorIndex =
        cursorKeyValue === null
          ? -1
          : items.findIndex((item) => item.kind === "row" && item.skill.name === cursorKeyValue);
      if (cursorIndex === -1 || base.includes(cursorIndex)) return base;
      return [...base, cursorIndex].sort((a, b) => a - b);
    },
    // While the list is `hidden` behind an open skill's page, the scroll element measures 0x0 -
    // ignoring that keeps the virtualizer's last real range instead of collapsing it to nothing,
    // so the list doesn't blank for a frame when it's shown again.
    observeElementRect: (instance, cb) =>
      observeDefaultElementRect(instance, (rect) => {
        if (rect.width === 0 && rect.height === 0) return;
        cb(rect);
      }),
  });

  // Destructured (rather than kept as one `cursor` object) so each JSX use below is a plain
  // identifier, not a member access - oxlint's `react(refs)` check otherwise treats every property
  // read off a custom hook's return value as a potential ref read during render.
  const {
    rowRef,
    containerRef,
    tabIndexFor,
    onGridKeyDown,
    focusCursor,
    focusRow,
    statusText,
    cursorKey,
  } = useRowCursor({
    keys: visibleKeys,
    // The row a just-closed skill page was opened from, so Escape back out of it returns
    // focus there instead of resetting the cursor to the first row.
    initialKey: initialCursorSkillName,
    active,
    scrollToKey: (key) => {
      const index = items.findIndex((item) => item.kind === "row" && item.skill.name === key);
      if (index !== -1) virtualizer.scrollToIndex(index, { align: "auto" });
    },
    onOpen: (key) => {
      const skill = rows.find((s) => s.name === key);
      if (skill) onSelectSkill(skill.name, deploymentPathForSkill?.(skill));
    },
    onToggle: (key) => {
      const index = rows.findIndex((s) => s.name === key);
      if (index !== -1) handleRowCheckboxClick(index, false);
    },
    onExtend: (key) => {
      const skill = rows.find((s) => s.name === key);
      const path = skill && rowPath(skill);
      if (!path) return;
      const next = new Set(selectedPaths);
      next.add(path);
      selectSkills([...next]);
      syncSelectionMode(next.size);
    },
    onMenu: (_key, rowEl) => {
      const triggers = rowEl.querySelectorAll<HTMLElement>('[data-slot="dropdown-menu-trigger"]');
      triggers[triggers.length - 1]?.click();
    },
    onEscape: () => {
      if (selectedPaths.size > 0) exitSelectionMode();
    },
    onCollapseGroup: (groupId) =>
      setCollapsedGroups((prev) =>
        // SAFETY: `groupId` only ever comes from this file's own `data-group` attributes, which
        // are always one of the three `RowGroup` values.
        new Set(prev).add(groupId as RowGroup),
      ),
    onExpandGroup: (groupId) =>
      setCollapsedGroups((prev) => {
        const next = new Set(prev);
        // SAFETY: `groupId` only ever comes from this file's own `data-group` attributes, which
        // are always one of the three `RowGroup` values.
        next.delete(groupId as RowGroup);
        return next;
      }),
  });
  useRowCursorWindowEntry(active, focusCursor);

  // Mirrors `cursorKey` into a ref for the virtualizer's `rangeExtractor` above - a plain callback
  // outside render, so it reads the ref rather than closing over this render's `cursorKey`.
  useEffect(() => {
    cursorKeyRef.current = cursorKey;
  }, [cursorKey]);

  // Resolves the ancestor scroll container once the grid element attaches - `PageShell`'s
  // `overflow-y-auto` content area, an ancestor of the grid rather than the grid itself.
  function setGridRef(el: HTMLDivElement | null) {
    containerRef(el);
    gridElRef.current = el;
    if (!el) return;
    let node: HTMLElement | null = el.parentElement;
    while (node) {
      const { overflowY } = window.getComputedStyle(node);
      if (overflowY === "auto" || overflowY === "scroll") {
        setScrollElement(node);
        return;
      }
      node = node.parentElement;
    }
  }

  // Keeps `scrollMargin` (the grid's offset from the top of the scroll container's content) in
  // sync with whatever sits above the grid - the filter chips, a partial-scan banner - by watching
  // the scroll container's content child for size changes.
  useEffect(() => {
    if (!scrollElement) return;
    function recompute() {
      const gridEl = gridElRef.current;
      if (!gridEl || !scrollElement) return;
      const gridRect = gridEl.getBoundingClientRect();
      // The grid is `hidden` behind an open skill's page - keep the last real margin instead of
      // collapsing it to 0.
      if (gridRect.width === 0 && gridRect.height === 0) return;
      const scrollRect = scrollElement.getBoundingClientRect();
      setScrollMargin(gridRect.top - scrollRect.top + scrollElement.scrollTop);
    }
    recompute();
    const contentEl = scrollElement.firstElementChild;
    if (!contentEl) return;
    const observer = new ResizeObserver(recompute);
    observer.observe(contentEl);
    return () => observer.disconnect();
  }, [scrollElement]);

  /** The store's `selectionMode` mirrors "at least one row checked" - kept in sync here since a
   * checkbox now drives selection directly instead of a separate mode switch. */
  function syncSelectionMode(nextSize: number) {
    if (nextSize > 0 && !selectionMode) enterSelectionMode();
    else if (nextSize === 0 && selectionMode) exitSelectionMode();
  }

  /** Checkbox click for one row - shift-click selects every row between it and the last clicked one, in visible (grouped) order. */
  function handleRowCheckboxClick(index: number, shiftKey: boolean) {
    if (shiftKey && lastCheckedIndexRef.current !== null) {
      const [from, to] = [lastCheckedIndexRef.current, index].sort((a, b) => a - b);
      const range = rows.slice(from, to + 1).map((s) => rowPath(s));
      const next = new Set(selectedPaths);
      range.forEach((path) => path && next.add(path));
      selectSkills([...next]);
      syncSelectionMode(next.size);
    } else {
      const path = rowPath(rows[index]);
      if (path) {
        const next = new Set(selectedPaths);
        if (next.has(path)) next.delete(path);
        else next.add(path);
        toggleSkillSelection(path);
        syncSelectionMode(next.size);
      }
    }
    lastCheckedIndexRef.current = index;
  }

  function toggleGroup(group: RowGroup) {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(group)) next.delete(group);
      else next.add(group);
      return next;
    });
  }

  /** Park/Unpark act on the deployment target `HomeView` uses; every other fix (Fix YAML, Fix
   * link, Compare, Convert, Keep, Pull latest) opens the skill's own detail, since those flows
   * live there. */
  async function handleAct(label: string, skill: InstalledSkill) {
    if (label !== "Park" && label !== "Unpark") {
      onSelectSkill(skill.name, deploymentPathForSkill?.(skill));
      return;
    }
    // Hoisted out of the try/catch below - the compiler can't optimize a conditional expression
    // computed inside a try/catch statement.
    const successTitle = label === "Park" ? `Parked ${skill.name}` : `Unparked ${skill.name}`;
    const failureTitle = label === "Park" ? "Couldn't park skill" : "Couldn't unpark skill";
    try {
      if (label === "Park") await parkSkill(lifecycleTargetForPark(skill));
      else await unparkSkill(lifecycleTargetForPark(skill));
      addToast({ type: "success", title: successTitle });
    } catch (err) {
      addToast({
        type: "error",
        title: failureTitle,
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  }

  /** One skill row - `index` is its position in the grouped `rows` array, for shift-click and
   * `aria-rowindex`; `useRowCursor` (via `skill.name`) drives the roving `tabIndex` instead.
   * `style` is the virtualizer's absolute positioning within the row's group container. */
  function renderRow(skill: InstalledSkill, index: number, style: CSSProperties) {
    const checked = selectedPaths.has(rowPath(skill) ?? "");
    const state = statesBySkill.get(skill.name) ?? null;
    return (
      <div
        key={skill.name}
        ref={rowRef(skill.name)}
        role="row"
        aria-rowindex={index + 1}
        aria-selected={checked}
        tabIndex={tabIndexFor(skill.name)}
        style={style}
        // `scroll-mt-7` (28px, `HEADER_HEIGHT`) keeps a row scrolled to by `scrollIntoView` from
        // surfacing under its group's sticky header.
        className={`${ROW_CLASS} scroll-mt-7 gap-x-3 px-3 ${COLUMNS} hover:bg-bg-secondary focus-visible:outline-2 focus-visible:outline-accent -outline-offset-2 ${selectedRowClass(
          skill.name === selectedSkillName,
        )} ${skill.parked ? "text-text-tertiary" : ""}`}
        onClick={() => onSelectSkill(skill.name, deploymentPathForSkill?.(skill))}
      >
        <div role="gridcell" className="contents">
          <SelectionCell
            skill={skill}
            checked={checked}
            visible={selectedPaths.size > 0}
            onCheckedChange={(_checked, eventDetails) => {
              // SAFETY: the underlying event is a pointer or keyboard event, both of which carry `shiftKey`.
              const shiftKey = (eventDetails.event as MouseEvent | KeyboardEvent).shiftKey;
              handleRowCheckboxClick(index, shiftKey);
            }}
          />
        </div>
        {/* Not `contents`: `LeadingCell` renders nothing for a healthy row, and a `contents`
            wrapper around no children drops out of the grid, shifting every column after it. */}
        <div role="gridcell" className="flex items-center justify-center">
          <LeadingCell
            skill={skill}
            state={state}
            glyphSize={GLYPH_SIZE}
            onOpen={() => onSelectSkill(skill.name, deploymentPathForSkill?.(skill))}
            onAct={(label) => void handleAct(label, skill)}
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
            glyphSize={GLYPH_SIZE}
            visible={checked}
            onOpen={() => onSelectSkill(skill.name, deploymentPathForSkill?.(skill))}
            onAct={(label) => void handleAct(label, skill)}
            onOpenChange={(open) => {
              if (!open) focusRow(skill.name);
            }}
            onToggleSelect={() => handleRowCheckboxClick(index, false)}
          />
        </div>
      </div>
    );
  }

  const virtualItems = virtualizer.getVirtualItems();

  return (
    <RichTooltipScope>
      <SkillRowMenuScope>
        <div
          className="flex flex-col gap-3"
          style={
            // SAFETY: `--glyph-hit` is a custom property, not a known CSSProperties key; React
            // passes it through to the style attribute as-is.
            { "--glyph-hit": `${GLYPH_HIT}px` } as CSSProperties
          }
        >
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
            <div
              ref={setGridRef}
              role="grid"
              aria-label="Skills"
              aria-rowcount={rows.length}
              // `overflow-clip`, not `overflow-hidden`: both clip rows to the rounded border, but
              // `hidden` makes the grid a scroll container, which stops the group headers sticking.
              className="overflow-clip rounded-md border border-border"
              onKeyDown={onGridKeyDown}
            >
              {GROUP_ORDER.map((group) => {
                const groupSkills = buckets[group];
                if (groupSkills.length === 0) return null;
                const open = !collapsedGroups.has(group);
                const { start: groupStart, height: groupHeight } = groupMeta[group];
                // Push-style sticky headers stay scoped to this group: only the virtual row items
                // that belong to it render here, absolutely positioned within it, so the next
                // group's header pushes this one up exactly as it did unvirtualized.
                const groupRowItems = open
                  ? virtualItems.filter((virtualItem) => {
                      const item = items[virtualItem.index];
                      return item.kind === "row" && item.group === group;
                    })
                  : [];
                return (
                  <Collapsible
                    key={group}
                    role="rowgroup"
                    data-group={group}
                    open={open}
                    onOpenChange={() => toggleGroup(group)}
                    style={{ position: "relative", height: groupHeight }}
                  >
                    {/* Sticky here, not only inside `GroupHead`: a sticky element sticks within its
                        parent, and this row is the header's parent. The group container is the
                        next header's parent, so that header pushes this one up. */}
                    <div role="row" className="sticky top-0 z-2">
                      <div role="gridcell">
                        <GroupHead
                          label={GROUP_LABEL[group]}
                          count={groupSkills.length}
                          groupId={group}
                        />
                      </div>
                    </div>
                    {groupRowItems.map((virtualItem) => {
                      const item = items[virtualItem.index];
                      // SAFETY: filtered to `kind === "row"` above.
                      if (item.kind !== "row") return null;
                      return renderRow(item.skill, item.index, {
                        position: "absolute",
                        top: 0,
                        left: 0,
                        width: "100%",
                        transform: `translateY(${virtualItem.start - scrollMargin - groupStart}px)`,
                      });
                    })}
                  </Collapsible>
                );
              })}
            </div>
          )}
          {/* Visually-hidden live region: announces the cursor's position, debounced to the last move. */}
          <div role="status" aria-live="polite" className="sr-only">
            {statusText}
          </div>

          {/* A zero-height wrapper so the sticky bar never reserves flow space of its own -
              checking a row must not push any other row down. `sticky bottom-4` then docks the
              bar to the bottom of the scroll area without an enter transition. */}
          {selectedPaths.size > 0 && (
            <div className="pointer-events-none sticky inset-x-0 bottom-4 z-10 flex h-0 items-end justify-center">
              <div className="pointer-events-auto flex h-9 items-center gap-2 rounded-md border border-border bg-bg-secondary px-2 shadow">
                <span className="px-1 text-small text-text-secondary">
                  {selectedPaths.size} selected
                </span>
                {packsEnabled && (
                  <Button
                    size="sm"
                    className="rounded-sm bg-accent-solid text-text-on-accent"
                    onClick={() => setShowPackPrompt(true)}
                  >
                    Create pack
                  </Button>
                )}
                <Button
                  variant="outline"
                  size="sm"
                  className="rounded-sm text-text-tertiary"
                  onClick={exitSelectionMode}
                >
                  Cancel
                </Button>
              </div>
            </div>
          )}

          {showPackPrompt && (
            <PackNamePrompt
              members={skills.reduce<PackMember[]>((members, s) => {
                const path = rowPath(s);
                if (path !== undefined && selectedPaths.has(path))
                  members.push({ name: s.name, path });
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
      </SkillRowMenuScope>
    </RichTooltipScope>
  );
}
