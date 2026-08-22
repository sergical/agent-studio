// ============================================================================
// SkillsScopeView - Global or single-project skill list
// ============================================================================

import { SkillList } from "./Dashboard/SkillList";
import type { SkillSnapshot } from "../lib/skill-types";

/** Which scope `SkillsScopeView` shows: every global deployment, or one project. */
export type SkillsScope = { kind: "global" } | { kind: "project"; path: string };

interface SkillsScopeViewProps {
  scope: SkillsScope;
  snapshot: SkillSnapshot | undefined;
  onSelectSkill: (name: string) => void;
}

/**
 * Header (scope name, path, skill count) plus the `SkillList` filtered to
 * that scope: global deployments (global or plugin scope) for `global`,
 * or deployments matching `project_path` for `project`.
 */
export function SkillsScopeView({ scope, snapshot, onSelectSkill }: SkillsScopeViewProps) {
  const skills =
    snapshot?.skills.filter((skill) =>
      scope.kind === "global"
        ? skill.deployments.some((d) => d.scope === "global" || d.scope === "plugin")
        : skill.deployments.some((d) => d.project_path === scope.path),
    ) ?? [];

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
      <SkillList
        skills={skills}
        stats={snapshot?.invocations ?? []}
        onSelectSkill={onSelectSkill}
      />
    </div>
  );
}
