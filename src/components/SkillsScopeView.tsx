// ============================================================================
// SkillsScopeView - Global or single-project skill list, own skills only
// ============================================================================

import { ownSkillsView } from "../lib/skill-plugin-partition";
import type { InstalledSkill, SkillSnapshot } from "../lib/skill-types";
import { useAppStore } from "../store/appStore";
import { SkillListTable } from "./SkillList/SkillListTable";

/** Which scope `SkillsScopeView` shows: every global deployment, or one project. */
export type SkillsScope = { kind: "global" } | { kind: "project"; path: string };

interface SkillsScopeViewProps {
  scope: SkillsScope;
  snapshot: SkillSnapshot | undefined;
  onSelectSkill: (name: string, deploymentPath?: string) => void;
}

/** The deployment `scope` shows for `skill`, so the detail drawer opens on that copy. */
function deploymentForScope(skill: InstalledSkill, scope: SkillsScope): string | undefined {
  return skill.deployments.find((d) =>
    scope.kind === "global"
      ? d.scope === "global" || d.scope === "plugin"
      : d.project_path === scope.path,
  )?.path;
}

/**
 * Header (scope name, path, skill count) plus a `SkillListTable` filtered to
 * that scope: global deployments (global or plugin scope) for `global`, or
 * deployments matching `project_path` for `project`. Excludes plugin-only
 * skills - those live under Plugins.
 */
export function SkillsScopeView({ scope, snapshot, onSelectSkill }: SkillsScopeViewProps) {
  const selectedSkillName = useAppStore((state) => state.selectedSkill?.name ?? null);
  const own = ownSkillsView(snapshot?.skills ?? []);

  const skills = own.filter((skill) =>
    scope.kind === "global"
      ? skill.deployments.some((d) => d.scope === "global" || d.scope === "plugin")
      : skill.deployments.some((d) => d.project_path === scope.path),
  );

  const title =
    scope.kind === "global"
      ? "Global"
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
      />
    </div>
  );
}
