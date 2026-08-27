// ============================================================================
// SkillListTable - Searchable, sortable skill rows, rendered by SkillsView
// with whatever it has already filtered down (scope, harness, source, issue)
// ============================================================================

import { useMemo, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { Search } from "lucide-react";
import { pluginLabelForSkill } from "../../lib/skill-plugin-partition";
import type { SkillRunSummary } from "../../lib/skill-run-history-types";
import { formatBytes, formatRelativeTime, formatTokens } from "../../lib/skill-stats";
import type { InstalledSkill, PackMember, SkillInvocationStats } from "../../lib/skill-types";
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
  if (!lastTest) return <span className="skill-list-table-tested">—</span>;
  return (
    <span className="skill-list-table-tested">
      <span
        className={`skill-list-table-tested-dot ${lastTest.passed === false ? "error" : "success"}`}
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
    <div className="skill-list-table" onKeyDown={handleTableKeyDown}>
      <div className="skill-list-table-toolbar">
        {selectionMode ? (
          <CheckboxControl
            checked={allVisibleSelected}
            onCheckedChange={handleHeaderCheckboxChange}
            disabled={rows.length === 0}
            ariaLabel="Select all visible skills"
          />
        ) : (
          <button className="skill-list-table-select-button" onClick={enterSelectionMode}>
            Select
          </button>
        )}
        <div className="skill-list-table-search">
          <Search size={13} />
          <input
            className="text-control"
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
      </div>

      {selectionMode && (
        <div className="skill-list-table-selection-bar">
          <span>{selectedPaths.size} selected</span>
          <button
            className="skill-list-table-selection-bar-create"
            onClick={() => setShowPackPrompt(true)}
            disabled={selectedPaths.size === 0}
          >
            Create pack
          </button>
          <button className="skill-list-table-selection-bar-clear" onClick={exitSelectionMode}>
            Cancel
          </button>
        </div>
      )}

      {rows.length === 0 ? (
        <div className="skill-list-table-empty">
          {hasAnySkills ? (
            <>
              <p>No skills match</p>
              {onClearFilters && (
                <button className="skill-list-table-empty-action" onClick={onClearFilters}>
                  Clear filters
                </button>
              )}
            </>
          ) : (
            <>
              <p>You haven't added a skill yet</p>
              {onAddSkill && (
                <button className="skill-list-table-empty-action" onClick={onAddSkill}>
                  Add skill
                </button>
              )}
            </>
          )}
        </div>
      ) : (
        <div className="skill-list-table-rows">
          {rows.map((skill, index) => {
            const stat = statsBySkill.get(skill.name);
            return (
              <div key={skill.name} className="skill-list-table-row-wrap">
                {selectionMode && (
                  <label
                    className="skill-list-table-row-checkbox"
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
                  className={`skill-list-table-row ${skill.name === selectedSkillName ? "selected" : ""}`}
                  onClick={(e) => {
                    if (selectionMode && e.shiftKey) {
                      handleRowCheckboxClick(index, true);
                      return;
                    }
                    handleRowClick(index, skill);
                  }}
                >
                  <span className="skill-list-table-name">
                    {skill.name}
                    {skill.has_update && (
                      <span className="skill-list-table-update-chip">Update</span>
                    )}
                    {pluginLabelForSkill(skill) && (
                      <span className="skill-list-table-provenance-chip">
                        plugin · {pluginLabelForSkill(skill)}
                      </span>
                    )}
                    {skill.parked && <span className="skill-list-table-parked-chip">Parked</span>}
                    {invocationChipLabel(skill.invocation) && (
                      <span className="skill-list-table-invocation-chip">
                        {invocationChipLabel(skill.invocation)}
                      </span>
                    )}
                  </span>
                  <span className="skill-list-table-description">{skill.description ?? ""}</span>
                  {showPluginVersion ? (
                    <span className="skill-list-table-chips">
                      {skill.deployments.map((d, i) => (
                        <span key={`${d.agent}-${d.scope}-${i}`} className="skill-list-table-chip">
                          {d.plugin?.version ? `v${d.plugin.version}` : d.agent}
                        </span>
                      ))}
                    </span>
                  ) : (
                    <SkillLocationCell skill={skill} />
                  )}
                  <span className="skill-list-table-stat">{stat?.last_30_days ?? 0}</span>
                  <span className="skill-list-table-stat">{formatBytes(skill.folder_bytes)}</span>
                  <span className="skill-list-table-stat" title="SKILL.md tokens">
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
