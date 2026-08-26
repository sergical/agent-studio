// ============================================================================
// SkillsScopeView - Global or single-project skill list, own skills only
// ============================================================================

import { ownSkillsView } from "../lib/skill-plugin-partition";
import type { InstalledSkill, SkillSnapshot } from "../lib/skill-types";
import { useAppStore } from "../store/appStore";
import { SkillListTable } from "./SkillList/SkillListTable";

/**
 * Which scope `SkillsScopeView` shows: every global deployment, one project,
 * or every parked (disabled globally - see `skill_park.rs`) skill.
 */
export type SkillsScope =
  | { kind: "global" }
  | { kind: "project"; path: string }
  | { kind: "parked" };

interface SkillsScopeViewProps {
  scope: SkillsScope;
  snapshot: SkillSnapshot | undefined;
  onSelectSkill: (name: string, deploymentPath?: string) => void;
}

/** The deployment `scope` shows for `skill`, so the detail drawer opens on that copy. */
function deploymentForScope(skill: InstalledSkill, scope: SkillsScope): string | undefined {
  return skill.deployments.find((d) => {
    if (scope.kind === "global") return d.scope === "global" || d.scope === "plugin";
    if (scope.kind === "parked") return d.scope === "parked";
    return d.project_path === scope.path;
  })?.path;
}

/**
 * Header (scope name, path, skill count) plus a `SkillListTable` filtered to
 * that scope: global deployments (global or plugin scope) for `global`,
 * deployments matching `project_path` for `project`, or parked skills for
 * `parked`. Excludes plugin-only skills - those live under Plugins.
 */
export function SkillsScopeView({ scope, snapshot, onSelectSkill }: SkillsScopeViewProps) {
  const selectedSkillName = useAppStore((state) =>
    state.activeView.kind === "skill" ? state.activeView.name : null,
  );
  const own = ownSkillsView(snapshot?.skills ?? []);

  const skills = own.filter((skill) => {
    if (scope.kind === "global")
      return skill.deployments.some((d) => d.scope === "global" || d.scope === "plugin");
    if (scope.kind === "parked") return skill.parked;
    return skill.deployments.some((d) => d.project_path === scope.path);
  });

  const title =
    scope.kind === "global"
      ? "Global"
      : scope.kind === "parked"
        ? "Parked"
        : (scope.path.split("/").filter(Boolean).pop() ?? scope.path);

  return (
    <div className="skills-scope-view">
      <div className="skills-scope-view-header">
        <h2>{title}</h2>
        {scope.kind === "project" && <p className="skills-scope-view-path">{scope.path}</p>}
        <span className="skill-store-count">
          {skills.length} skill{skills.length !== 1 ? "s" : ""}
        </span>
      </div>
      <SkillListTable
        skills={skills}
        stats={snapshot?.invocations ?? []}
        onSelectSkill={onSelectSkill}
        selectedSkillName={selectedSkillName}
        deploymentPathForSkill={(skill) => deploymentForScope(skill, scope)}
        lastTestBySkill={snapshot?.last_test_by_skill}
      />
    </div>
  );
}
