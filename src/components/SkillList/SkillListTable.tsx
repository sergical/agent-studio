// ============================================================================
// SkillListTable - Searchable, sortable skill rows, rendered by SkillsView
// with whatever it has already filtered down (scope, harness, source, issue)
// ============================================================================

import { useMemo, useRef, useState } from "react";
import { Link2, Search, Unlink } from "lucide-react";
import { deploymentLinkKind } from "../../lib/skill-coverage";
import { pluginLabelForSkill } from "../../lib/skill-plugin-partition";
import type { SkillRunSummary } from "../../lib/skill-run-history-types";
import { formatBytes, formatRelativeTime } from "../../lib/skill-stats";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import type { InstalledSkill, PackMember, SkillInvocationStats } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { PackNamePrompt } from "../Packs/PackNamePrompt";

type SortMode = "name" | "used" | "size";

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
 * Toolbar (filter + sort) above a list of skill rows: name, one-line
 * description, agent chips (or plugin version chips), 30-day use count, and
 * folder size. Clicking a row selects the skill in the detail drawer.
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
  /** Index of the last row checked by click (not shift-click), for shift-click range-select. */
  const lastCheckedIndexRef = useRef<number | null>(null);

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

  return (
    <div className="skill-list-table">
      <div className="skill-list-table-toolbar">
        <label className="skill-list-table-header-checkbox" aria-label="Select all visible skills">
          <input
            type="checkbox"
            checked={allVisibleSelected}
            disabled={rows.length === 0}
            onChange={handleHeaderCheckboxChange}
          />
        </label>
        <div className="skill-list-table-search">
          <Search size={13} />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter by name or description…"
          />
        </div>
        <select
          className="skill-list-table-sort"
          value={sort}
          // SAFETY: the <select>'s options are the three SortMode literals below.
          onChange={(e) => setSort(e.target.value as SortMode)}
        >
          <option value="name">Name</option>
          <option value="used">Most used</option>
          <option value="size">Largest</option>
        </select>
      </div>

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
                <label
                  className="skill-list-table-row-checkbox"
                  aria-label={`Select ${skill.name}`}
                >
                  <input
                    type="checkbox"
                    checked={selectedPaths.has(rowPath(skill) ?? "")}
                    onClick={(e) => handleRowCheckboxClick(index, e.shiftKey)}
                    // The click handler above already applies the change - this
                    // onChange only exists to keep React's controlled-input
                    // contract happy (it fires after onClick, redundantly).
                    onChange={() => {}}
                  />
                </label>
                <button
                  className={`skill-list-table-row ${skill.name === selectedSkillName ? "selected" : ""}`}
                  onClick={() => onSelectSkill(skill.name, deploymentPathForSkill?.(skill))}
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
                  <span className="skill-list-table-chips">
                    {skill.deployments.map((d, i) => {
                      const harnessId = harnessIdFromLabel(d.agent);
                      const linkKind = deploymentLinkKind(d);
                      return (
                        <span
                          key={`${d.agent}-${d.scope}-${i}`}
                          className={`skill-list-table-chip ${d.disabled ? "disabled" : ""}`}
                        >
                          {harnessId && <HarnessIcon harness={harnessId} size={12} />}
                          {showPluginVersion && d.plugin?.version
                            ? `v${d.plugin.version}`
                            : d.agent}
                          {linkKind === "linked-to-shared" && (
                            <span
                              className="skill-list-table-chip-marker"
                              title="Symlink to the shared folder"
                            >
                              <Link2 size={10} />
                            </span>
                          )}
                          {linkKind === "broken" && (
                            <span
                              className="skill-list-table-chip-marker broken"
                              title="Broken link"
                            >
                              <Unlink size={10} />
                            </span>
                          )}
                          {d.disabled && (
                            <span
                              className="skill-list-table-chip-disabled-marker"
                              title="Disabled"
                            >
                              Disabled
                            </span>
                          )}
                        </span>
                      );
                    })}
                  </span>
                  <span className="skill-list-table-stat">{stat?.last_30_days ?? 0}</span>
                  <span className="skill-list-table-stat">{formatBytes(skill.folder_bytes)}</span>
                  <TestedCell lastTest={lastTestBySkill?.[skill.name]} />
                </button>
              </div>
            );
          })}
        </div>
      )}

      {selectedPaths.size > 0 && (
        <div className="skill-list-table-selection-bar">
          <span>{selectedPaths.size} selected</span>
          <button
            className="skill-list-table-selection-bar-create"
            onClick={() => setShowPackPrompt(true)}
          >
            Create pack
          </button>
          <button className="skill-list-table-selection-bar-clear" onClick={clearSkillSelection}>
            Clear
          </button>
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
