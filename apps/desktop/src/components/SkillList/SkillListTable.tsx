// ============================================================================
// SkillListTable - Searchable, sortable skill rows, rendered by SkillsView
// with whatever it has already filtered down (scope, harness, source, issue)
// ============================================================================

import { useMemo, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { Search } from "lucide-react";
import { pluginLabelForSkill } from "@skill-studio/lib";
import type { SkillRunSummary } from "@skill-studio/lib";
import { formatBytes, formatRelativeTime, formatTokens } from "@skill-studio/lib";
import type { InstalledSkill, PackMember, SkillInvocationStats } from "@skill-studio/lib";
import { useAppStore } from "../../store/appStore";
import { PackNamePrompt } from "../Packs/PackNamePrompt";
import { CheckboxControl } from "../ui/CheckboxControl";
import { SelectControl } from "../ui/SelectControl";
import { SkillLocationCell } from "./SkillLocationCell";

type SortMode = "name" | "used" | "size";

const SORT_ITEMS: { value: SortMode; label: string }[] = [
  { value: "name", label: "Name" },
  { value: "used", label: "Most used" },
  { value: "size", label: "Largest" },
];

function isSortMode(value: string): value is SortMode {
  return value === "name" || value === "used" || value === "size";
}

/** The row's own-relation and location chips (SkillLocationCell) share this base look. */
const CHIP_CLASS =
  "inline-flex items-center gap-1 whitespace-nowrap rounded-sm bg-bg-tertiary px-1.5 py-0.5 text-caption tracking-[0.02em] text-text-secondary";

/** "User only" / "Model only" chip label, `null` for the default "both" policy. */
function invocationChipLabel(invocation: InstalledSkill["invocation"]): string | null {
  if (invocation === "user-only") return "User only";
  if (invocation === "model-only") return "Model only";
  return null;
}

interface SkillListTableProps {
  skills: InstalledSkill[];
  stats: SkillInvocationStats[];
  onSelectSkill: (name: string, deploymentPath?: string) => void;
  selectedSkillName?: string | null;
  /** Plugins view: show each deployment's plugin version instead of the agent label. */
  showPluginVersion?: boolean;
  /** Resolves which deployment a row's click should open in the detail drawer, when the caller knows it. */
  deploymentPathForSkill?: (skill: InstalledSkill) => string | undefined;
  /** The newest "Test" run outcome per skill name, for the "Tested" column. */
  lastTestBySkill?: Record<string, SkillRunSummary>;
  /** False when the caller's underlying list (before any filter) is empty, for the right empty state. */
  hasAnySkills?: boolean;
  /** Resets the caller's filter, for the "No skills match" empty state. */
  onClearFilters?: () => void;
  /** Opens the add-skill sheet, for the "You haven't added a skill yet" empty state. */
  onAddSkill?: () => void;
}

/** "2 h ago" with a 6px outcome dot, or "—" when the skill was never tested. */
function TestedCell({ lastTest }: { lastTest: SkillRunSummary | undefined }) {
  const testedClass =
    "inline-flex items-center justify-end gap-[5px] whitespace-nowrap text-right text-caption text-text-tertiary";
  if (!lastTest) return <span className={testedClass}>—</span>;
  return (
    <span className={testedClass}>
      <span
        className={`size-1.5 shrink-0 rounded-full ${lastTest.passed === false ? "bg-error" : "bg-success"}`}
      />
      {formatRelativeTime(lastTest.at)}
    </span>
  );
}

/**
 * Toolbar (Select, filter, sort) above a list of skill rows: name, one-line
 * description, location chips (or plugin version chips), 30-day use count,
 * and folder size. Clicking a row selects the skill in the detail drawer,
 * unless selection mode is on, where it toggles the row instead.
 */
export function SkillListTable({
  skills,
  stats,
  onSelectSkill,
  selectedSkillName = null,
  showPluginVersion = false,
  deploymentPathForSkill,
  lastTestBySkill,
  hasAnySkills = true,
  onClearFilters,
  onAddSkill,
}: SkillListTableProps) {
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortMode>("name");
  const [showPackPrompt, setShowPackPrompt] = useState(false);
  const statsBySkill = useMemo(() => new Map(stats.map((s) => [s.skill, s])), [stats]);
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

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = q
      ? skills.filter(
          (s) =>
            s.name.toLowerCase().includes(q) || (s.description ?? "").toLowerCase().includes(q),
        )
      : skills;

    const sorted = [...filtered];
    if (sort === "name") {
      sorted.sort((a, b) => a.name.localeCompare(b.name));
    } else if (sort === "used") {
      sorted.sort(
        (a, b) =>
          (statsBySkill.get(b.name)?.last_30_days ?? 0) -
          (statsBySkill.get(a.name)?.last_30_days ?? 0),
      );
    } else {
      sorted.sort((a, b) => b.folder_bytes - a.folder_bytes);
    }
    return sorted;
  }, [skills, query, sort, statsBySkill]);

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
      <div className="flex items-center gap-2">
        <div className="flex w-[76px] shrink-0 items-center justify-center [&_.checkbox-control-root]:before:absolute [&_.checkbox-control-root]:before:-inset-3 [&_.checkbox-control-root]:before:content-['']">
          {selectionMode ? (
            <CheckboxControl
              checked={allVisibleSelected}
              onCheckedChange={handleHeaderCheckboxChange}
              disabled={rows.length === 0}
              ariaLabel="Select all visible skills"
            />
          ) : (
            <button
              className="h-(--control-height) w-full cursor-pointer rounded-sm border border-border bg-transparent px-3 text-body text-text-secondary transition-colors hover:bg-bg-hover hover:text-text-primary"
              onClick={enterSelectionMode}
            >
              Select
            </button>
          )}
        </div>
        {selectionMode ? (
          <div className="flex h-(--control-height) flex-1 items-center gap-3 rounded-md border border-border bg-bg-elevated px-3.5 text-small text-text-secondary">
            <span>{selectedPaths.size} selected</span>
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
          </div>
        ) : (
          <>
            <div className="relative flex max-w-[320px] flex-1 items-center text-text-tertiary">
              <Search size={13} className="pointer-events-none absolute left-3" />
              <input
                className="h-(--control-height) w-full rounded-sm border border-border bg-bg-primary py-0 pr-3 pl-8 text-body text-text-primary transition-colors placeholder:text-text-quaternary focus-visible:border-border-focus"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Filter by name or description…"
              />
            </div>
            <SelectControl
              value={sort}
              onValueChange={(v) => {
                if (isSortMode(v)) setSort(v);
              }}
              items={SORT_ITEMS}
              ariaLabel="Sort"
            />
          </>
        )}
      </div>

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
                  className={`grid h-11 min-w-0 flex-1 cursor-pointer items-center gap-3 overflow-hidden rounded-md border border-border bg-bg-secondary px-3 text-left transition-colors [grid-template-columns:minmax(0,1fr)_minmax(0,2fr)_auto_auto_auto_auto_72px] hover:bg-bg-hover ${
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
                  <span className="truncate text-body font-semibold text-text-primary">
                    {skill.name}
                    {skill.has_update && (
                      <span className="ml-1.5 inline-flex rounded-sm bg-warning-soft px-1.5 py-px text-caption font-semibold text-warning">
                        Update
                      </span>
                    )}
                    {pluginLabelForSkill(skill) && (
                      <span className="ml-1.5 inline-flex rounded-sm bg-bg-tertiary px-1.5 py-px text-caption font-semibold text-text-secondary">
                        plugin · {pluginLabelForSkill(skill)}
                      </span>
                    )}
                    {skill.parked && (
                      <span className="ml-1.5 inline-flex rounded-sm bg-bg-tertiary px-1.5 py-px text-caption font-semibold text-text-secondary">
                        Parked
                      </span>
                    )}
                    {invocationChipLabel(skill.invocation) && (
                      <span className="ml-1.5 inline-flex rounded-sm bg-bg-tertiary px-1.5 py-px text-caption font-semibold text-text-secondary">
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
                  {showPluginVersion ? (
                    <span className="flex min-w-0 flex-wrap gap-1">
                      {skill.deployments.map((d, i) => (
                        <span key={`${d.agent}-${d.scope}-${i}`} className={CHIP_CLASS}>
                          {d.plugin?.version ? `v${d.plugin.version}` : d.agent}
                        </span>
                      ))}
                    </span>
                  ) : (
                    <SkillLocationCell skill={skill} />
                  )}
                  <span className="whitespace-nowrap text-right text-caption tabular-nums text-text-tertiary">
                    {stat?.last_30_days ?? 0}
                  </span>
                  <span className="whitespace-nowrap text-right text-caption tabular-nums text-text-tertiary">
                    {formatBytes(skill.folder_bytes)}
                  </span>
                  <span
                    className="whitespace-nowrap text-right text-caption tabular-nums text-text-tertiary"
                    title="SKILL.md tokens"
                  >
                    {formatTokens(skill.skill_md_tokens)}
                  </span>
                  <TestedCell lastTest={lastTestBySkill?.[skill.name]} />
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
