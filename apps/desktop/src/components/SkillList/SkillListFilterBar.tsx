// ============================================================================
// SkillListFilterBar - The Skills view's one filter row: scope, harness,
// source, coverage toggle, and the result count. Filters live here, not in
// the sidebar - see the design rule in spec-ux-1.md section C.
// ============================================================================

import { ChevronDown, X } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { harnessesPresent } from "@skill-studio/lib";
import { defaultSkillListFilter, isProjectScope, type SkillListFilter } from "@skill-studio/lib";
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

/** Reproduces the former shared `.menu-control-item` look inline - InstalledSkillHeader has its own copy of the same styling. */
const MENU_ITEM_CLASS =
  "flex h-(--control-height) cursor-pointer items-center gap-2 rounded-sm px-2.5 text-body text-text-secondary transition-colors data-highlighted:bg-bg-hover data-highlighted:text-text-primary";
const MENU_RADIO_ITEM_CLASS =
  "flex h-auto flex-col items-start gap-0.5 rounded-sm px-2.5 py-1.5 text-body text-text-secondary transition-colors data-highlighted:bg-bg-hover data-highlighted:text-text-primary";
const MENU_SEPARATOR_CLASS = "mx-0.5 my-1 h-px border-none bg-border-subtle";
/** Reproduces the former shared `.segmented`/`.segmented-item` look inline - only used by the Scope group below. */
const SEGMENTED_ITEM_CLASS =
  "inline-flex h-(--control-height) items-center gap-1 border border-l-0 border-border bg-transparent px-3 text-body text-text-tertiary transition-colors duration-150 hover:bg-bg-hover hover:text-text-secondary aria-pressed:bg-bg-tertiary aria-pressed:text-text-primary";

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
    <button
      className="inline-flex h-6 cursor-pointer items-center gap-1.5 rounded-sm border-0 bg-accent-soft px-2 text-caption text-text-secondary transition-colors hover:bg-accent-softer"
      onClick={onClear}
    >
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
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-2.5">
        <div
          className="flex [&>*:first-child]:rounded-l-sm [&>*:first-child]:border-l [&>*:last-child]:rounded-r-sm"
          role="group"
          aria-label="Scope"
        >
          <button
            className={SEGMENTED_ITEM_CLASS}
            aria-pressed={filter.scope === "all"}
            onClick={() => setScope("all")}
          >
            All
          </button>
          <button
            className={SEGMENTED_ITEM_CLASS}
            aria-pressed={filter.scope === "global"}
            onClick={() => setScope("global")}
          >
            Global
          </button>
          <MenuControl
            triggerClassName={SEGMENTED_ITEM_CLASS}
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
                <MenuSeparator className={MENU_SEPARATOR_CLASS} />
                <MenuItem
                  closeOnClick
                  className={MENU_ITEM_CLASS}
                  onClick={() => handleStopTracking(selectedProject)}
                >
                  Stop tracking {basename(selectedProject)}…
                </MenuItem>
              </>
            )}
            <MenuSeparator className={MENU_SEPARATOR_CLASS} />
            <MenuItem closeOnClick className={MENU_ITEM_CLASS} onClick={onAddProject}>
              Add project…
            </MenuItem>
          </MenuControl>
        </div>

        {harnesses.length > 1 && (
          <div className="flex flex-wrap gap-1.5" role="group" aria-label="Harness">
            {harnesses.map((harness) => {
              const harnessId = harnessIdFromLabel(harness);
              const active = filter.harness === harness;
              return (
                <button
                  key={harness}
                  className={`inline-flex h-8 cursor-pointer items-center gap-1.5 rounded-sm border border-border bg-bg-tertiary px-2.5 text-caption text-text-tertiary transition-colors hover:bg-bg-hover hover:text-text-secondary ${
                    active ? "border-text-tertiary text-text-primary" : ""
                  }`}
                  onClick={() => toggleHarness(harness)}
                  aria-pressed={active}
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

        <div className="flex-1" />

        <label className="flex h-(--control-height) cursor-pointer items-center gap-2 text-small text-text-secondary">
          <SwitchControl checked={showCoverage} onCheckedChange={onToggleCoverage} />
          Show coverage
        </label>

        <span className="whitespace-nowrap text-small tabular-nums">
          {resultCount} skill{resultCount !== 1 ? "s" : ""}
        </span>
      </div>

      {activeCount > 1 && (
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
          {filter.query.trim() && (
            <ActiveChip
              label={`“${filter.query.trim()}”`}
              onClear={() => onChange({ ...filter, query: "" })}
            />
          )}
          <button
            className="h-6 cursor-pointer border-0 bg-transparent px-2 text-caption text-text-tertiary transition-colors hover:text-text-primary"
            onClick={() => onChange(defaultSkillListFilter())}
          >
            Clear all
          </button>
        </div>
      )}
    </div>
  );
}
