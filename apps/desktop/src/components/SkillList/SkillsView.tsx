// ============================================================================
// SkillsView - The unified, filterable skill list. Replaces SkillsScopeView,
// PluginSkillsView, SkillCoverageView, and SkillIssuesView: the sidebar holds
// places, this view holds filters (scope, harness, source, issue, query).
// ============================================================================

import { useState } from "react";
import { PageShell } from "../Shell/PageShell";
import { SkillCoverageMatrix } from "../Coverage/SkillCoverageMatrix";
import { ScanPartialBanner } from "./ScanPartialBanner";
import { SkillListTable } from "./SkillListTable";
import type { SortMode } from "../../lib/skill-list-sort";
import { SkillListActiveFilters, SkillListFilterBar } from "./SkillListFilterBar";
import { useProjectFolderActions } from "../../hooks/useProjectFolderActions";
import { collectDashboardIssues } from "@skill-studio/lib";
import { applySkillListFilter, isProjectScope } from "@skill-studio/lib";
import type { SkillListFilter } from "@skill-studio/lib";
import { ownSkillsView } from "@skill-studio/lib";
import type { InstalledSkill, SkillSnapshot } from "@skill-studio/lib";
import { useAppStore } from "../../store/appStore";

/** The deployment the current scope shows for `skill`, so the detail drawer opens on that copy. */
function deploymentForScope(
  skill: InstalledSkill,
  scope: SkillListFilter["scope"],
): string | undefined {
  if (scope === "global") {
    return skill.deployments.find((d) => d.scope === "global" || d.scope === "plugin")?.path;
  }
  if (scope === "parked") return skill.deployments.find((d) => d.scope === "parked")?.path;
  if (isProjectScope(scope)) {
    return skill.deployments.find((d) => d.project_path === scope.project)?.path;
  }
  return undefined;
}

interface SkillsViewProps {
  snapshot: SkillSnapshot | undefined;
  onSelectSkill: (name: string, deploymentPath?: string) => void;
  /** Whether Skills is the view on screen right now - `false` while it's kept mounted but hidden
   * behind an open skill's page, so its window-level keyboard shortcuts stay off. */
  active: boolean;
}

/**
 * The unified, filterable skill list. Holds no filter state of its own -
 * scope, harness, source, issue, query, and the coverage toggle all live in
 * the store's `skillListFilter`/`showCoverage`, so opening a skill and
 * coming back never loses them.
 */
export function SkillsView({ snapshot, onSelectSkill, active }: SkillsViewProps) {
  const filter = useAppStore((state) => state.skillListFilter);
  const setSkillListFilter = useAppStore((state) => state.setSkillListFilter);
  const resetSkillListFilter = useAppStore((state) => state.resetSkillListFilter);
  const showCoverage = useAppStore((state) => state.showCoverage);
  const setShowCoverage = useAppStore((state) => state.setShowCoverage);
  const [sort, setSort] = useState<SortMode>("name");
  const selectedSkillName = useAppStore((state) =>
    state.activeView.kind === "skill" ? state.activeView.name : null,
  );
  const lastClosedSkillName = useAppStore((state) => state.lastClosedSkillName);
  const userAddedProjects = useAppStore((state) => state.userAddedProjects);
  const excludedProjects = useAppStore((state) => state.excludedProjects);
  const openAddSkillSheet = useAppStore((state) => state.openAddSkillSheet);
  const { addProject, stopTracking } = useProjectFolderActions();

  const excludedProjectSet = new Set(excludedProjects);
  const projects = Array.from(
    new Set([...userAddedProjects, ...(snapshot?.projects ?? [])]),
  ).filter((path) => !excludedProjectSet.has(path));

  const allSkills = snapshot?.skills ?? [];
  // Plugin-shipped skills live in their own place (PluginSkillsView); this
  // list is always the user's own skills.
  const baseSkills = ownSkillsView(allSkills);
  const issues = collectDashboardIssues(baseSkills);
  const rows = applySkillListFilter(baseSkills, filter, issues, snapshot?.invocations);

  /** Adds a project via the shared hook, then switches the scope to it. */
  const handleAddProject = async () => {
    const added = await addProject();
    if (added) setSkillListFilter({ scope: { project: added } });
  };

  return (
    <PageShell
      title="Skills"
      toolbar={
        <SkillListFilterBar
          filter={filter}
          onChange={setSkillListFilter}
          projects={projects}
          onAddProject={handleAddProject}
          onRemoveProject={stopTracking}
          showCoverage={showCoverage}
          onToggleCoverage={setShowCoverage}
          resultCount={rows.length}
          sort={sort}
          onSortChange={setSort}
          snapshot={snapshot}
        />
      }
    >
      <SkillListActiveFilters
        filter={filter}
        onChange={setSkillListFilter}
        onReset={resetSkillListFilter}
      />
      {snapshot?.scan_partial && <ScanPartialBanner observations={snapshot.scan_observations} />}
      {showCoverage ? (
        <SkillCoverageMatrix skills={rows} onSelectSkill={onSelectSkill} />
      ) : (
        <SkillListTable
          skills={rows}
          stats={snapshot?.invocations ?? []}
          sort={sort}
          onSelectSkill={onSelectSkill}
          selectedSkillName={selectedSkillName}
          initialCursorSkillName={lastClosedSkillName}
          active={active}
          deploymentPathForSkill={(skill) => deploymentForScope(skill, filter.scope)}
          hasAnySkills={baseSkills.length > 0}
          onClearFilters={resetSkillListFilter}
          onAddSkill={() => openAddSkillSheet()}
        />
      )}
    </PageShell>
  );
}
