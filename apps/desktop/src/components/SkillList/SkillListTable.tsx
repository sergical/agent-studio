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
import { groupSkillRows } from "../../lib/skill-list-model";
import type { SortMode } from "../../lib/skill-list-sort";
import { useRowCursor, useRowCursorWindowEntry } from "../../hooks/useRowCursor";
import { RichTooltipScope } from "../ui/RichTooltip";
import { PackNamePrompt } from "../Packs/PackNamePrompt";
import { SkillRowMenuScope } from "./SkillRowMenu";
import { SkillListEmptyState } from "./SkillListEmptyState";
import { SkillListGroup } from "./SkillListGroup";
import { SkillListRow } from "./SkillListRow";
import { SkillListSelectionBar } from "./SkillListSelectionBar";
import { useSkillListAct } from "./useSkillListAct";
import { useSkillListScrollMargin } from "./useSkillListScrollMargin";
import { useSkillListSelection } from "./useSkillListSelection";
import type { RowGroup } from "./skill-row-state";

/** The row's leading-glyph hit box, and the icon it holds - fixed sizes. */
const GLYPH_HIT = 28;
const GLYPH_SIZE = 14;

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
export type ListItem =
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
  const virtualizer = useVirtualizer(options); // react-doctor-disable-line react-hooks-js/incompatible-library
  // Read here, not in the caller: the instance keeps its identity, so a compiled caller would
  // cache the first (empty) range.
  return { virtualizer, virtualItems: virtualizer.getVirtualItems() };
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
 * anything is checked. Pack creation is deferred (unit 4.3), so the bar's
 * Create pack button never renders.
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
  const packsEnabled = false;
  /** The roving cursor's key, mirrored here so the virtualizer's `rangeExtractor` (a plain
   * callback, not part of render) can always keep that row's item in range without depending on
   * `useRowCursor`'s return value before it exists. */
  const cursorKeyRef = useRef<string | null>(null);

  /** The deployment path this row's selection checkbox stands for. */
  const rowPath = (skill: InstalledSkill): string | undefined =>
    deploymentPathForSkill?.(skill) ?? skill.deployments[0]?.path;

  const { buckets, statesBySkill, rows } = groupSkillRows(skills, sort, stats);
  /** Row keys `useRowCursor` navigates, in rendered order - a collapsed group's rows drop out. */
  const visibleKeys = GROUP_ORDER.flatMap((group) =>
    collapsedGroups.has(group) ? [] : buckets[group].map((skill) => skill.name),
  );

  const {
    selectedPaths,
    clearSkillSelection,
    selectSkills,
    exitSelectionMode,
    syncSelectionMode,
    handleRowCheckboxClick,
  } = useSkillListSelection(rows, rowPath);
  const handleAct = useSkillListAct(onSelectSkill, deploymentPathForSkill);
  const { scrollElement, scrollMargin, setGridElement } = useSkillListScrollMargin();

  /** The flat item list the virtualizer measures, and each group's offset/size in that same
   * coordinate space - see `buildListItems`. */
  const { items, groupMeta } = buildListItems(buckets, collapsedGroups);

  const { virtualizer, virtualItems } = useSkillListVirtualizer({
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

  // Forwards the grid element to both `containerRef` (roving-cursor navigation) and
  // `setGridElement` (scroll-container/scrollMargin tracking) - both need the same node.
  function setGridRef(el: HTMLDivElement | null) {
    containerRef(el);
    setGridElement(el);
  }

  function toggleGroup(group: RowGroup) {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(group)) next.delete(group);
      else next.add(group);
      return next;
    });
  }

  /** One skill row - `index` is its position in the grouped `rows` array, for shift-click and
   * `aria-rowindex`; `useRowCursor` (via `skill.name`) drives the roving `tabIndex` instead.
   * `top` is the virtualizer's offset within the row's group container. */
  function renderRow(skill: InstalledSkill, index: number, top: number) {
    const checked = selectedPaths.has(rowPath(skill) ?? "");
    const state = statesBySkill.get(skill.name) ?? null;
    return (
      <SkillListRow
        key={skill.name}
        skill={skill}
        index={index}
        style={{
          position: "absolute",
          top: 0,
          left: 0,
          width: "100%",
          transform: `translateY(${top}px)`,
        }}
        state={state}
        checked={checked}
        hasSelection={selectedPaths.size > 0}
        isOpenSkill={skill.name === selectedSkillName}
        tabIndex={tabIndexFor(skill.name)}
        glyphSize={GLYPH_SIZE}
        rowRef={rowRef(skill.name)}
        onOpen={() => onSelectSkill(skill.name, deploymentPathForSkill?.(skill))}
        onAct={(label) => void handleAct(label, skill)}
        onCheckedChange={(shiftKey) => handleRowCheckboxClick(index, shiftKey)}
        onMenuOpenChange={(open) => {
          if (!open) focusRow(skill.name);
        }}
      />
    );
  }

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
            <SkillListEmptyState
              hasAnySkills={hasAnySkills}
              onClearFilters={onClearFilters}
              onAddSkill={onAddSkill}
            />
          ) : (
            <div
              ref={setGridRef}
              role="grid"
              aria-label="Skills"
              aria-rowcount={rows.length}
              onKeyDown={onGridKeyDown}
            >
              {GROUP_ORDER.map((group) => {
                const groupSkills = buckets[group];
                if (groupSkills.length === 0) return null;
                const open = !collapsedGroups.has(group);
                const { start: groupStart, height: groupHeight } = groupMeta[group];
                return (
                  <SkillListGroup
                    key={group}
                    group={group}
                    label={GROUP_LABEL[group]}
                    count={groupSkills.length}
                    open={open}
                    start={groupStart}
                    height={groupHeight}
                    items={items}
                    virtualItems={virtualItems}
                    scrollMargin={scrollMargin}
                    onToggle={() => toggleGroup(group)}
                    renderRow={(item, top) => renderRow(item.skill, item.index, top)}
                  />
                );
              })}
            </div>
          )}
          {/* Visually-hidden live region: announces the cursor's position, debounced to the last move. */}
          <div role="status" aria-live="polite" className="sr-only">
            {statusText}
          </div>

          {selectedPaths.size > 0 && (
            <SkillListSelectionBar
              count={selectedPaths.size}
              packsEnabled={packsEnabled}
              onCreatePack={() => setShowPackPrompt(true)}
              onCancel={exitSelectionMode}
            />
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
