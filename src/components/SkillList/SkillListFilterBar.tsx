// ============================================================================
// SkillListFilterBar - The Skills view's one filter row: scope, harness,
// source, coverage toggle, and the result count. Filters live here, not in
// the sidebar - see the design rule in spec-ux-1.md section C.
// ============================================================================

import { ChevronDown, X } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { harnessesPresent } from "../../lib/home-summary";
import {
  defaultSkillListFilter,
  isProjectScope,
  type SkillListFilter,
} from "../../lib/skill-list-filter";
import { shortProjectPath } from "../../lib/skill-path-format";
import type { InstalledSkill, SkillSnapshot, SkillSourceKind } from "../../lib/skill-types";
import { SOURCE_KIND_LABELS } from "../../lib/skill-types";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import {
  MenuControl,
  MenuItem,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
} from "../ui/MenuControl";
import { SelectControl } from "../ui/SelectControl";
import { SwitchControl } from "../ui/SwitchControl";

const SOURCE_KINDS: SkillSourceKind[] = [
  "dotagents",
  "skills-sh",
  "plugin",
  "in-repo",
  "manual",
  "fork",
];

const SOURCE_OPTIONS: { value: string; label: string }[] = [
  { value: "", label: "Any source" },
  ...SOURCE_KINDS.map((source) => ({ value: source, label: SOURCE_KIND_LABELS[source] })),
];

const INVOCATION_LABELS = {
  both: "You or the model",
  "model-only": "Model only",
  "user-only": "You only",
} as const satisfies Record<InstalledSkill["invocation"], string>;

const USAGE_LABELS = {
  "used-30d": "Used in 30 days",
  "unused-30d": "Not used in 30 days",
} as const satisfies Record<NonNullable<SkillListFilter["usage"]>, string>;

interface SkillListFilterBarProps {
  filter: SkillListFilter;
  onChange: (filter: SkillListFilter) => void;
  projects: string[];
  onAddProject: () => void;
  /** Stops tracking `path` - "Stop tracking" on the project menu's row for the selected project. */
  onRemoveProject: (path: string) => void | Promise<void>;
  showCoverage: boolean;
  onToggleCoverage: (show: boolean) => void;
  resultCount: number;
  /** For the harness chip row - only harnesses with at least one deployment in this snapshot are shown. */
  snapshot: SkillSnapshot | undefined;
}

/** The project menu's basename label, e.g. "/Users/x/my-app" -> "my-app". */
function basename(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? path;
}

/** The project scope button's label: the folder name of the selected project, else "Project". */
function projectScopeLabel(filter: SkillListFilter): string {
  if (!isProjectScope(filter.scope)) return "Project";
  return basename(filter.scope.project);
}

/** One removable chip in the active-filters row: a label and an "×" that clears that field. */
function ActiveChip({ label, onClear }: { label: string; onClear: () => void }) {
  return (
    <button className="skill-list-filter-active-chip" onClick={onClear}>
      {label}
      <X size={11} />
    </button>
  );
}

/** How many of `filter`'s optional fields are set - the chip row only shows once more than one is active. */
function activeFilterCount(filter: SkillListFilter): number {
  let count = filter.scope !== "all" ? 1 : 0;
  if (filter.harness) count += 1;
  if (filter.source) count += 1;
  if (filter.issue) count += 1;
  if (filter.invocation) count += 1;
  if (filter.usage) count += 1;
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
  snapshot,
}: SkillListFilterBarProps) {
  const harnesses = snapshot ? harnessesPresent(snapshot) : [];

  function setScope(scope: SkillListFilter["scope"]) {
    onChange({ ...filter, scope });
  }

  function toggleHarness(harness: string) {
    onChange({ ...filter, harness: filter.harness === harness ? undefined : harness });
  }

  async function handleStopTracking(path: string) {
    const folder = basename(path);
    const confirmed = await ask(`Stop tracking ${folder}? Skills stay on disk.`, {
      title: "Stop tracking project",
      kind: "warning",
    });
    if (!confirmed) return;
    await onRemoveProject(path);
  }

  const activeCount = activeFilterCount(filter);
  const selectedProject = isProjectScope(filter.scope) ? filter.scope.project : undefined;

  return (
    <div className="skill-list-filter-bar">
      <div className="skill-list-filter-row">
        <div className="segmented" role="group" aria-label="Scope">
          <button
            className="segmented-item"
            aria-pressed={filter.scope === "all"}
            onClick={() => setScope("all")}
          >
            All
          </button>
          <button
            className="segmented-item"
            aria-pressed={filter.scope === "global"}
            onClick={() => setScope("global")}
          >
            Global
          </button>
          <MenuControl
            triggerClassName="segmented-item"
            triggerAriaLabel="Project"
            trigger={
              <>
                {projectScopeLabel(filter)}
                <ChevronDown size={12} />
              </>
            }
          >
            {projects.length === 0 && <p className="menu-control-empty">No projects tracked yet</p>}
            <MenuRadioGroup
              value={selectedProject ?? null}
              onValueChange={(value) => {
                // SAFETY: every MenuRadioItem in this group carries a string project path.
                setScope({ project: value as string });
              }}
            >
              {projects.map((path) => (
                <MenuRadioItem key={path} value={path} closeOnClick className="menu-control-item">
                  <span className="menu-control-item-text" title={basename(path)}>
                    {basename(path)}
                  </span>
                  <span className="menu-control-item-secondary">{shortProjectPath(path)}</span>
                </MenuRadioItem>
              ))}
            </MenuRadioGroup>
            {selectedProject && (
              <>
                <MenuSeparator className="menu-control-separator" />
                <MenuItem
                  closeOnClick
                  className="menu-control-item"
                  onClick={() => handleStopTracking(selectedProject)}
                >
                  Stop tracking {basename(selectedProject)}…
                </MenuItem>
              </>
            )}
            <MenuSeparator className="menu-control-separator" />
            <MenuItem closeOnClick className="menu-control-item" onClick={onAddProject}>
              Add project…
            </MenuItem>
          </MenuControl>
        </div>

        {harnesses.length > 1 && (
          <div className="skill-list-filter-harnesses" role="group" aria-label="Harness">
            {harnesses.map((harness) => {
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
        )}

        <SelectControl
          value={filter.source ?? ""}
          onValueChange={(value) => {
            // SAFETY: `value` is one of SOURCE_OPTIONS' values - "" or a SkillSourceKind literal.
            const source = (value || undefined) as SkillSourceKind | undefined;
            onChange({ ...filter, source });
          }}
          items={SOURCE_OPTIONS}
          ariaLabel="Source"
        />

        <div className="skill-list-filter-spacer" />

        <label className="switch-label">
          <SwitchControl checked={showCoverage} onCheckedChange={onToggleCoverage} />
          Show coverage
        </label>

        <span className="skill-list-filter-count count-tabular">
          {resultCount} skill{resultCount !== 1 ? "s" : ""}
        </span>
      </div>

      {activeCount > 1 && (
        <div className="skill-list-filter-active-chips">
          {filter.scope !== "all" && (
            <ActiveChip
              label={
                filter.scope === "global"
                  ? "Global"
                  : filter.scope === "parked"
                    ? "Parked"
                    : projectScopeLabel(filter)
              }
              onClear={() => setScope("all")}
            />
          )}
          {filter.harness && (
            <ActiveChip
              label={filter.harness}
              onClear={() => onChange({ ...filter, harness: undefined })}
            />
          )}
          {filter.source && (
            <ActiveChip
              label={SOURCE_KIND_LABELS[filter.source]}
              onClear={() => onChange({ ...filter, source: undefined })}
            />
          )}
          {filter.issue && (
            <ActiveChip
              label={filter.issue === "any" ? "Has issues" : filter.issue}
              onClear={() => onChange({ ...filter, issue: undefined })}
            />
          )}
          {filter.invocation && (
            <ActiveChip
              label={INVOCATION_LABELS[filter.invocation]}
              onClear={() => onChange({ ...filter, invocation: undefined })}
            />
          )}
          {filter.usage && (
            <ActiveChip
              label={USAGE_LABELS[filter.usage]}
              onClear={() => onChange({ ...filter, usage: undefined })}
            />
          )}
          {filter.query.trim() && (
            <ActiveChip
              label={`“${filter.query.trim()}”`}
              onClear={() => onChange({ ...filter, query: "" })}
            />
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
