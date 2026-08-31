// ============================================================================
// SkillsView - The unified, filterable skill list. Replaces SkillsScopeView,
// PluginSkillsView, SkillCoverageView, and SkillIssuesView: the sidebar holds
// places, this view holds filters (scope, harness, source, issue, query).
// ============================================================================

import { homeDir } from "@tauri-apps/api/path";
import { open } from "@tauri-apps/plugin-dialog";
import { PageShell } from "../Shell/PageShell";
import { SkillCoverageMatrix } from "../Coverage/SkillCoverageMatrix";
import { SkillListTable } from "./SkillListTable";
import { SkillListFilterBar } from "./SkillListFilterBar";
import { registerSkillProjects, unregisterSkillProject } from "../../lib/skill-api";
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
}

/**
 * The unified, filterable skill list. Holds no filter state of its own -
 * scope, harness, source, issue, query, and the coverage toggle all live in
 * the store's `skillListFilter`/`showCoverage`, so opening a skill and
 * coming back never loses them.
 */
export function SkillsView({ snapshot, onSelectSkill }: SkillsViewProps) {
  const filter = useAppStore((state) => state.skillListFilter);
  const setSkillListFilter = useAppStore((state) => state.setSkillListFilter);
  const resetSkillListFilter = useAppStore((state) => state.resetSkillListFilter);
  const showCoverage = useAppStore((state) => state.showCoverage);
  const setShowCoverage = useAppStore((state) => state.setShowCoverage);
  const selectedSkillName = useAppStore((state) =>
    state.activeView.kind === "skill" ? state.activeView.name : null,
  );
  const userAddedProjects = useAppStore((state) => state.userAddedProjects);
  const excludedProjects = useAppStore((state) => state.excludedProjects);
  const addProject = useAppStore((state) => state.addProject);
  const removeProject = useAppStore((state) => state.removeProject);
  const addToast = useAppStore((state) => state.addToast);
  const openAddSkillSheet = useAppStore((state) => state.openAddSkillSheet);

  const projects = Array.from(
    new Set([...userAddedProjects, ...(snapshot?.projects ?? [])]),
  ).filter((path) => !excludedProjects.includes(path));

  const allSkills = snapshot?.skills ?? [];
  // Plugin-shipped skills live in their own place (PluginSkillsView); this
  // list is always the user's own skills.
  const baseSkills = ownSkillsView(allSkills);
  const issues = collectDashboardIssues(baseSkills);
  const filtered = applySkillListFilter(baseSkills, filter, issues, snapshot?.invocations);

  const handleAddProject = async () => {
    const selected = await open({ directory: true, multiple: false, title: "Add Project" });
    if (!selected) return;

    const home = await homeDir().catch(() => null);
    if (home && selected === home) {
      addToast({
        type: "error",
        title: "Can't add the home directory",
        message: "It's the global scope, not a project.",
      });
      return;
    }

    try {
      await registerSkillProjects([selected]);
      addProject(selected);
      setSkillListFilter({ scope: { project: selected } });
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't add project",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  /**
   * Un-registers `path` with the backend first; the store (and the scope,
   * if it was the active project) only updates once that succeeds, so a
   * failed unregister leaves tracking state unchanged and reports an error
   * instead of silently un-tracking a project the backend still has.
   */
  const handleRemoveProject = async (path: string) => {
    try {
      await unregisterSkillProject(path);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't stop tracking project",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      return;
    }
    removeProject(path);
    if (isProjectScope(filter.scope) && filter.scope.project === path) {
      setSkillListFilter({ scope: "all" });
    }
  };

  return (
    <PageShell title="Skills">
      <SkillListFilterBar
        filter={filter}
        onChange={setSkillListFilter}
        projects={projects}
        onAddProject={handleAddProject}
        onRemoveProject={handleRemoveProject}
        showCoverage={showCoverage}
        onToggleCoverage={setShowCoverage}
        resultCount={filtered.length}
        snapshot={snapshot}
      />
      {showCoverage ? (
        <SkillCoverageMatrix skills={filtered} onSelectSkill={onSelectSkill} />
      ) : (
        <SkillListTable
          skills={filtered}
          stats={snapshot?.invocations ?? []}
          onSelectSkill={onSelectSkill}
          selectedSkillName={selectedSkillName}
          deploymentPathForSkill={(skill) => deploymentForScope(skill, filter.scope)}
          hasAnySkills={baseSkills.length > 0}
          onClearFilters={resetSkillListFilter}
          onAddSkill={() => openAddSkillSheet()}
        />
      )}
    </PageShell>
  );
}
