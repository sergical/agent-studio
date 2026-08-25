// ============================================================================
// PluginSkillsView - Skills shipped by one harness's plugin cache, grouped
// by plugin, read-only
// ============================================================================

import { groupPluginSkills } from "../../lib/skill-plugin-partition";
import type { SkillSnapshot } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { SkillListTable } from "../SkillList/SkillListTable";

interface PluginSkillsViewProps {
  harness: string;
  snapshot: SkillSnapshot | undefined;
  onSelectSkill: (name: string, deploymentPath?: string) => void;
}

/**
 * One section per plugin in `harness`'s cache, each with a
 * `SkillListTable` of the skills it ships. These are managed by the harness
 * (Claude Code, Codex), not the user, so there's no install/remove here.
 */
export function PluginSkillsView({ harness, snapshot, onSelectSkill }: PluginSkillsViewProps) {
  const selectedSkillName = useAppStore((state) =>
    state.activeView.kind === "skill" ? state.activeView.name : null,
  );
  const groups = groupPluginSkills(snapshot?.skills ?? []).filter((g) => g.harness === harness);

  return (
    <div className="plugin-skills-view">
      <div className="plugin-skills-view-header">
        <h2>{harness}</h2>
        <p className="plugin-skills-view-note">
          Shipped by {harness} plugins. Managed by the harness; read-only here.
        </p>
      </div>
      {groups.map((group) => (
        <div key={`${group.harness}-${group.pluginName}`} className="plugin-skills-view-group">
          <h3 className="plugin-skills-view-group-title section-label">
            {group.pluginName}
            {group.version ? ` v${group.version}` : ""} · {group.skills.length} skill
            {group.skills.length !== 1 ? "s" : ""}
          </h3>
          <SkillListTable
            skills={group.skills}
            stats={snapshot?.invocations ?? []}
            onSelectSkill={onSelectSkill}
            selectedSkillName={selectedSkillName}
            showPluginVersion
            deploymentPathForSkill={(skill) =>
              skill.deployments.find(
                (d) => d.plugin?.harness === group.harness && d.plugin?.name === group.pluginName,
              )?.path
            }
          />
        </div>
      ))}
    </div>
  );
}
