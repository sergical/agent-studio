// ============================================================================
// SkillListFilterBar - The Skills view's one filter row: scope, harness,
// source, coverage toggle, and the result count. Filters live here, not in
// the sidebar - see the design rule in spec-ux-1.md section C.
// ============================================================================

import { ChevronDown, LayoutGrid, List, ListFilter, Search, X } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { ToggleGroup, ToggleGroupItem } from "@skill-studio/ui";
import { harnessesPresent } from "@skill-studio/lib";
import { isProjectScope, type SkillListFilter } from "@skill-studio/lib";
import { shortProjectPath } from "@skill-studio/lib";
import type { InstalledSkill, SkillSnapshot, SkillSourceKind } from "@skill-studio/lib";
import { SOURCE_KIND_LABELS } from "@skill-studio/lib";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import {
  MenuControl,
  MenuItem,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
} from "../ui/MenuControl";
import { SelectControl } from "../ui/SelectControl";
import { TooltipControl } from "../ui/TooltipControl";
import { SORT_ITEMS, isSortMode } from "../../lib/skill-list-sort";
import type { SortMode } from "../../lib/skill-list-sort";
import { singleSelectToggleValue } from "../../lib/single-select-toggle-group";

// No "plugin" here: plugin-shipped skills have their own place (PluginSkillsView).
const SOURCE_KINDS: SkillSourceKind[] = ["dotagents", "skills-sh", "in-repo", "manual", "fork"];

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

/** The project menu's two-line rows (name + full path) - `MenuItem`'s baked-in default is a single line. */
const MENU_RADIO_ITEM_CLASS =
  "flex h-auto flex-col items-start gap-0.5 rounded-sm px-2.5 py-1.5 text-body text-text-secondary transition-colors data-highlighted:bg-bg-hover data-highlighted:text-text-primary";
/** The Scope group's project trigger sits after the ToggleGroup, styled the same as its items. */
const SEGMENTED_TRIGGER_CLASS =
  "inline-flex h-(--control-height) items-center gap-1 rounded-r-sm border-y border-r border-border bg-transparent px-3 text-body text-text-tertiary transition-colors duration-150 hover:bg-bg-hover hover:text-text-secondary aria-expanded:bg-bg-tertiary aria-expanded:text-text-primary";

interface SkillListFilterBarProps {
  filter: SkillListFilter;
  onChange: (filter: SkillListFilter) => void;
  /** "Clear all" on the active-chips row - must replace the whole filter, not merge a patch (a merge would keep the very fields being cleared). */
  onReset: () => void;
  projects: string[];
  onAddProject: () => void;
  /** Stops tracking `path` - "Stop tracking" on the project menu's row for the selected project. */
  onRemoveProject: (path: string) => void | Promise<void>;
  showCoverage: boolean;
  onToggleCoverage: (show: boolean) => void;
  resultCount: number;
  /** Free-text name/description filter, applied by `SkillListTable`. */
  query: string;
  onQueryChange: (query: string) => void;
  /** Sort order, applied by `SkillListTable`. */
  sort: SortMode;
  onSortChange: (sort: SortMode) => void;
  /** For the harness chips - harnesses at least one deployment gives coverage for (see `harnessesPresent`). */
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
    <button
      className="inline-flex h-6 cursor-pointer items-center gap-1.5 rounded-sm border-0 bg-accent-soft px-2 text-caption text-text-secondary transition-colors hover:bg-accent-softer"
      onClick={onClear}
    >
      {label}
      <X size={11} />
    </button>
  );
}

/**
 * How many of `filter`'s optional fields are set, not counting the query -
 * the search box already shows the query, but every other active filter needs
 * a chip: invocation and usage are only ever set from Home's stat tiles and
 * have no control of their own in this bar.
 */
function activeFilterCount(filter: SkillListFilter): number {
  let count = filter.scope !== "all" ? 1 : 0;
  if (filter.harness) count += 1;
  if (filter.source) count += 1;
  if (filter.issue) count += 1;
  if (filter.invocation) count += 1;
  if (filter.usage) count += 1;
  return count;
}

/** How many of the Filter menu's own fields are set, for its trigger badge. */
function filterMenuCount(filter: SkillListFilter): number {
  return (filter.harness ? 1 : 0) + (filter.source ? 1 : 0);
}

export function SkillListFilterBar({
  filter,
  onChange,
  onReset,
  projects,
  onAddProject,
  onRemoveProject,
  showCoverage,
  onToggleCoverage,
  resultCount,
  query,
  onQueryChange,
  sort,
  onSortChange,
  snapshot,
}: SkillListFilterBarProps) {
  const harnesses = snapshot ? harnessesPresent(snapshot) : [];

  function setScope(scope: SkillListFilter["scope"]) {
    onChange({ ...filter, scope });
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
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-2.5">
        <div className="relative flex w-60 items-center text-text-tertiary">
          <Search size={13} className="pointer-events-none absolute left-3" />
          <input
            className="h-(--control-height) w-full rounded-sm border border-border bg-bg-primary py-0 pr-3 pl-8 text-body text-text-primary transition-colors placeholder:text-text-quaternary focus-visible:border-border-focus"
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
            placeholder="Filter skills…"
            aria-label="Filter skills"
          />
        </div>

        <div className="flex" role="group" aria-label="Scope">
          <ToggleGroup
            variant="segmented"
            className="rounded-r-none"
            value={filter.scope === "all" || filter.scope === "global" ? [filter.scope] : []}
            onValueChange={(next) => singleSelectToggleValue<"all" | "global">(next, setScope)}
          >
            <ToggleGroupItem value="all" className="px-3">
              All
            </ToggleGroupItem>
            <ToggleGroupItem value="global" className="px-3">
              Global
            </ToggleGroupItem>
          </ToggleGroup>
          <MenuControl
            triggerClassName={SEGMENTED_TRIGGER_CLASS}
            triggerAriaLabel="Project"
            trigger={
              <>
                {projectScopeLabel(filter)}
                <ChevronDown size={12} />
              </>
            }
          >
            {projects.length === 0 && (
              <p className="px-2.5 py-1.5 text-pretty text-small text-text-tertiary">
                No projects tracked yet
              </p>
            )}
            <MenuRadioGroup
              value={selectedProject ?? null}
              onValueChange={(value) => {
                // SAFETY: every MenuRadioItem in this group carries a string project path.
                setScope({ project: value as string });
              }}
            >
              {projects.map((path) => (
                <MenuRadioItem
                  key={path}
                  value={path}
                  closeOnClick
                  className={MENU_RADIO_ITEM_CLASS}
                >
                  <span className="truncate" title={basename(path)}>
                    {basename(path)}
                  </span>
                  <span className="block text-caption text-text-tertiary">
                    {shortProjectPath(path)}
                  </span>
                </MenuRadioItem>
              ))}
            </MenuRadioGroup>
            {selectedProject && (
              <>
                <MenuSeparator />
                <MenuItem closeOnClick onClick={() => handleStopTracking(selectedProject)}>
                  Stop tracking {basename(selectedProject)}…
                </MenuItem>
              </>
            )}
            <MenuSeparator />
            <MenuItem closeOnClick onClick={onAddProject}>
              Add project…
            </MenuItem>
          </MenuControl>
        </div>

        <MenuControl
          triggerClassName="inline-flex h-(--control-height) cursor-pointer items-center gap-1.5 rounded-sm border border-border bg-transparent px-2.5 text-body text-text-tertiary transition-colors hover:bg-bg-hover hover:text-text-secondary"
          triggerAriaLabel="Filter"
          trigger={
            <>
              <ListFilter size={13} />
              Filter
              {filterMenuCount(filter) > 0 && (
                <span className="rounded-full bg-accent-soft px-1.5 text-caption tabular-nums text-accent">
                  {filterMenuCount(filter)}
                </span>
              )}
            </>
          }
        >
          {harnesses.length > 1 && (
            <>
              <p className="m-0 px-2.5 pt-1.5 pb-0.5 text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
                Harness
              </p>
              <MenuRadioGroup
                value={filter.harness ?? ""}
                onValueChange={(value) => {
                  // SAFETY: every MenuRadioItem in this group carries a string - "" (Any) or a harness label.
                  onChange({ ...filter, harness: (value as string) || undefined });
                }}
              >
                <MenuRadioItem value="" closeOnClick>
                  Any harness
                </MenuRadioItem>
                {harnesses.map((harness) => {
                  const harnessId = harnessIdFromLabel(harness);
                  return (
                    <MenuRadioItem key={harness} value={harness} closeOnClick>
                      {harnessId && <HarnessIcon harness={harnessId} size={13} />}
                      {harness}
                    </MenuRadioItem>
                  );
                })}
              </MenuRadioGroup>
              <MenuSeparator />
            </>
          )}
          <p className="m-0 px-2.5 pt-1.5 pb-0.5 text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
            Source
          </p>
          <MenuRadioGroup
            value={filter.source ?? ""}
            onValueChange={(value) => {
              // SAFETY: every value here comes from SOURCE_OPTIONS - "" or a SkillSourceKind literal.
              onChange({
                ...filter,
                source: ((value as string) || undefined) as SkillSourceKind | undefined,
              });
            }}
          >
            {SOURCE_OPTIONS.map((option) => (
              <MenuRadioItem key={option.value} value={option.value} closeOnClick>
                {option.value === "" ? "Any source" : option.label}
              </MenuRadioItem>
            ))}
          </MenuRadioGroup>
        </MenuControl>

        <div className="flex-1" />

        <span className="whitespace-nowrap text-small tabular-nums text-text-tertiary">
          {resultCount} skill{resultCount !== 1 ? "s" : ""}
        </span>

        <SelectControl
          value={sort}
          onValueChange={(v) => {
            if (isSortMode(v)) onSortChange(v);
          }}
          items={SORT_ITEMS}
          ariaLabel="Sort"
          triggerPrefix="Sort:"
        />

        <ToggleGroup
          variant="segmented"
          aria-label="View"
          value={[showCoverage ? "coverage" : "list"]}
          onValueChange={(next) =>
            singleSelectToggleValue<"list" | "coverage">(next, (selected) =>
              onToggleCoverage(selected === "coverage"),
            )
          }
        >
          <TooltipControl content="List">
            <ToggleGroupItem value="list" className="px-2.5" aria-label="List view">
              <List size={14} />
            </ToggleGroupItem>
          </TooltipControl>
          <TooltipControl content="Coverage matrix">
            <ToggleGroupItem value="coverage" className="px-2.5" aria-label="Coverage matrix view">
              <LayoutGrid size={14} />
            </ToggleGroupItem>
          </TooltipControl>
        </ToggleGroup>
      </div>

      {activeCount > 0 && (
        <div className="flex flex-wrap items-center gap-1.5">
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
          <button
            className="h-6 cursor-pointer border-0 bg-transparent px-2 text-caption text-text-tertiary transition-colors hover:text-text-primary"
            onClick={onReset}
          >
            Clear all
          </button>
        </div>
      )}
    </div>
  );
}
