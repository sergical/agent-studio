// ============================================================================
// SkillListFilterBar - The Skills view's one filter row: scope, harness,
// source, coverage toggle, and the result count. Filters live here, not in
// the sidebar - see the design rule in spec-ux-1.md section C.
// ============================================================================

import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { ChevronDown, X } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { FIRST_CLASS_AGENTS } from "../../lib/skill-health";
import {
  defaultSkillListFilter,
  isProjectScope,
  type SkillListFilter,
} from "../../lib/skill-list-filter";
import type { SkillSourceKind } from "../../lib/skill-types";
import { SOURCE_KIND_LABELS } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";

const SOURCE_OPTIONS: SkillSourceKind[] = ["dotagents", "skills-sh", "plugin", "manual", "fork"];

/** The project menu's roving-focus items, in DOM order: each project's radio then its "Stop tracking" menuitem, then "Add project…". */
const MENU_ITEM_SELECTOR = '[role="menuitem"], [role="menuitemradio"]';

interface SkillListFilterBarProps {
  filter: SkillListFilter;
  onChange: (filter: SkillListFilter) => void;
  projects: string[];
  onAddProject: () => void;
  /** Stops tracking `path` - "Stop tracking" on each project menu row. */
  onRemoveProject: (path: string) => void | Promise<void>;
  showCoverage: boolean;
  onToggleCoverage: (show: boolean) => void;
  resultCount: number;
}

/** The project scope button's label: the folder name of the selected project, else "Project". */
function projectScopeLabel(filter: SkillListFilter): string {
  if (!isProjectScope(filter.scope)) return "Project";
  return filter.scope.project.split("/").filter(Boolean).pop() ?? filter.scope.project;
}

/** How many of `filter`'s optional fields are set - the chip row only shows once more than one is active. */
function activeFilterCount(filter: SkillListFilter): number {
  let count = filter.scope !== "all" ? 1 : 0;
  if (filter.harness) count += 1;
  if (filter.source) count += 1;
  if (filter.issue) count += 1;
  if (filter.query.trim()) count += 1;
  return count;
}

export function SkillListFilterBar({
  filter,
  onChange,
  projects,
  onAddProject,
  onRemoveProject,
  showCoverage,
  onToggleCoverage,
  resultCount,
}: SkillListFilterBarProps) {
  const [projectMenuOpen, setProjectMenuOpen] = useState(false);
  const projectMenuRef = useRef<HTMLDivElement>(null);
  const projectMenuPopupRef = useRef<HTMLDivElement>(null);
  const projectMenuTriggerRef = useRef<HTMLButtonElement>(null);
  const focusProjectMenuOnce = useAppStore((state) => state.focusProjectMenuOnce);
  const clearFocusProjectMenuOnce = useAppStore((state) => state.clearFocusProjectMenuOnce);

  // Home's "projects" link asks the trigger to take focus (not to open the
  // menu) - a keyboard user then opens it themselves with Enter/Space/Down.
  useEffect(() => {
    if (!focusProjectMenuOnce) return;
    projectMenuTriggerRef.current?.focus();
    clearFocusProjectMenuOnce();
  }, [focusProjectMenuOnce, clearFocusProjectMenuOnce]);

  // ARIA menu pattern: opening focuses the checked project (or the first
  // item) so arrow keys have somewhere to start from.
  useEffect(() => {
    if (!projectMenuOpen) return;
    const items =
      projectMenuPopupRef.current?.querySelectorAll<HTMLButtonElement>(MENU_ITEM_SELECTOR);
    if (!items || items.length === 0) return;
    const checkedIndex = [...items].findIndex(
      (item) => item.getAttribute("aria-checked") === "true",
    );
    items[checkedIndex >= 0 ? checkedIndex : 0]?.focus();
  }, [projectMenuOpen]);

  useEffect(() => {
    if (!projectMenuOpen) return;
    function onPointerDown(e: MouseEvent) {
      // SAFETY: `e.target` on a DOM event is always a `Node` (or `null`),
      // never a non-DOM value - `Node.contains` accepts exactly that.
      if (!projectMenuRef.current?.contains(e.target as Node)) setProjectMenuOpen(false);
    }
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [projectMenuOpen]);

  function closeProjectMenu(returnFocus: boolean) {
    setProjectMenuOpen(false);
    if (returnFocus) projectMenuTriggerRef.current?.focus();
  }

  function focusMenuItemAt(index: number) {
    const items =
      projectMenuPopupRef.current?.querySelectorAll<HTMLButtonElement>(MENU_ITEM_SELECTOR);
    if (!items || items.length === 0) return;
    items[((index % items.length) + items.length) % items.length]?.focus();
  }

  function handleTriggerKeyDown(e: KeyboardEvent<HTMLButtonElement>) {
    if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
      e.preventDefault();
      setProjectMenuOpen(true);
    }
  }

  /**
   * Roving focus across the flat item list: each project row contributes
   * two items (its `menuitemradio` and a "Stop tracking" `menuitem`), then
   * "Add project…" - ArrowUp/Down and Home/End move between all of them.
   */
  function handlePopupKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const items =
      projectMenuPopupRef.current?.querySelectorAll<HTMLButtonElement>(MENU_ITEM_SELECTOR);
    const currentIndex = items
      ? [...items].findIndex((item) => item === document.activeElement)
      : -1;

    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        focusMenuItemAt(currentIndex + 1);
        break;
      case "ArrowUp":
        e.preventDefault();
        focusMenuItemAt(currentIndex - 1);
        break;
      case "Home":
        e.preventDefault();
        focusMenuItemAt(0);
        break;
      case "End":
        e.preventDefault();
        focusMenuItemAt((items?.length ?? 1) - 1);
        break;
      case "Escape":
        e.preventDefault();
        closeProjectMenu(true);
        break;
      case "Tab":
        closeProjectMenu(false);
        break;
    }
  }

  function setScope(scope: SkillListFilter["scope"]) {
    onChange({ ...filter, scope });
  }

  function toggleHarness(harness: string) {
    onChange({ ...filter, harness: filter.harness === harness ? undefined : harness });
  }

  async function handleStopTracking(path: string) {
    const folder = path.split("/").filter(Boolean).pop() ?? path;
    const confirmed = await ask(`Stop tracking ${folder}? Skills stay on disk.`, {
      title: "Stop tracking project",
      kind: "warning",
    });
    if (!confirmed) return;
    await onRemoveProject(path);
  }

  const activeCount = activeFilterCount(filter);

  return (
    <div className="skill-list-filter-bar">
      <div className="skill-list-filter-row">
        <div className="skill-list-filter-scope" role="group" aria-label="Scope">
          <button
            className={`skill-list-filter-scope-item ${filter.scope === "all" ? "active" : ""}`}
            onClick={() => setScope("all")}
          >
            All
          </button>
          <button
            className={`skill-list-filter-scope-item ${filter.scope === "global" ? "active" : ""}`}
            onClick={() => setScope("global")}
          >
            Global
          </button>
          <div className="skill-list-filter-project-menu" ref={projectMenuRef}>
            <button
              ref={projectMenuTriggerRef}
              className={`skill-list-filter-scope-item ${isProjectScope(filter.scope) ? "active" : ""}`}
              aria-haspopup="menu"
              aria-expanded={projectMenuOpen}
              onClick={() => setProjectMenuOpen((open) => !open)}
              onKeyDown={handleTriggerKeyDown}
            >
              {projectScopeLabel(filter)}
              <ChevronDown size={12} />
            </button>
            {projectMenuOpen && (
              <div
                ref={projectMenuPopupRef}
                className="skill-list-filter-project-menu-popup"
                role="menu"
                onKeyDown={handlePopupKeyDown}
              >
                {projects.length === 0 && (
                  <p className="skill-list-filter-project-menu-empty">No projects tracked yet</p>
                )}
                {projects.map((path) => {
                  const basename = path.split("/").filter(Boolean).pop() ?? path;
                  const isSelected = isProjectScope(filter.scope) && filter.scope.project === path;
                  return (
                    <div key={path} className="skill-list-filter-project-menu-row">
                      <button
                        role="menuitemradio"
                        aria-checked={isSelected}
                        className="skill-list-filter-project-menu-item"
                        title={path}
                        onClick={() => {
                          setScope({ project: path });
                          closeProjectMenu(true);
                        }}
                      >
                        {basename}
                      </button>
                      <button
                        role="menuitem"
                        className="skill-list-filter-project-menu-remove"
                        aria-label={`Stop tracking ${basename}`}
                        onClick={(e) => {
                          e.stopPropagation();
                          handleStopTracking(path);
                        }}
                      >
                        <X size={13} />
                      </button>
                    </div>
                  );
                })}
                <button
                  role="menuitem"
                  className="skill-list-filter-project-menu-item skill-list-filter-project-menu-add"
                  onClick={() => {
                    closeProjectMenu(true);
                    onAddProject();
                  }}
                >
                  Add project…
                </button>
              </div>
            )}
          </div>
        </div>

        <div className="skill-list-filter-harnesses" role="group" aria-label="Harness">
          {FIRST_CLASS_AGENTS.map((harness) => {
            const harnessId = harnessIdFromLabel(harness);
            return (
              <button
                key={harness}
                className={`skill-list-filter-chip ${filter.harness === harness ? "active" : ""}`}
                onClick={() => toggleHarness(harness)}
                aria-pressed={filter.harness === harness}
              >
                {harnessId && <HarnessIcon harness={harnessId} size={12} />}
                {harness}
              </button>
            );
          })}
        </div>

        <label className="skill-list-filter-source">
          <span className="sr-only">Source</span>
          <select
            value={filter.source ?? ""}
            onChange={(e) =>
              onChange({
                ...filter,
                // SAFETY: the <select>'s options are "" or a SkillSourceKind literal.
                source: (e.target.value || undefined) as SkillSourceKind | undefined,
              })
            }
          >
            <option value="">Any source</option>
            {SOURCE_OPTIONS.map((source) => (
              <option key={source} value={source}>
                {SOURCE_KIND_LABELS[source]}
              </option>
            ))}
          </select>
        </label>

        <div className="skill-list-filter-spacer" />

        <label className="skill-list-filter-coverage-toggle">
          <input
            type="checkbox"
            checked={showCoverage}
            onChange={(e) => onToggleCoverage(e.target.checked)}
          />
          Show coverage
        </label>

        <span className="skill-list-filter-count count-tabular">
          {resultCount} skill{resultCount !== 1 ? "s" : ""}
        </span>
      </div>

      {activeCount > 1 && (
        <div className="skill-list-filter-active-chips">
          {filter.scope !== "all" && (
            <button className="skill-list-filter-active-chip" onClick={() => setScope("all")}>
              {filter.scope === "global"
                ? "Global"
                : filter.scope === "parked"
                  ? "Parked"
                  : projectScopeLabel(filter)}
              <X size={11} />
            </button>
          )}
          {filter.harness && (
            <button
              className="skill-list-filter-active-chip"
              onClick={() => onChange({ ...filter, harness: undefined })}
            >
              {filter.harness}
              <X size={11} />
            </button>
          )}
          {filter.source && (
            <button
              className="skill-list-filter-active-chip"
              onClick={() => onChange({ ...filter, source: undefined })}
            >
              {SOURCE_KIND_LABELS[filter.source]}
              <X size={11} />
            </button>
          )}
          {filter.issue && (
            <button
              className="skill-list-filter-active-chip"
              onClick={() => onChange({ ...filter, issue: undefined })}
            >
              {filter.issue === "any" ? "Has issues" : filter.issue}
              <X size={11} />
            </button>
          )}
          {filter.query.trim() && (
            <button
              className="skill-list-filter-active-chip"
              onClick={() => onChange({ ...filter, query: "" })}
            >
              “{filter.query.trim()}”
              <X size={11} />
            </button>
          )}
          <button
            className="skill-list-filter-clear-all"
            onClick={() => onChange(defaultSkillListFilter())}
          >
            Clear all
          </button>
        </div>
      )}
    </div>
  );
}
