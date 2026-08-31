// ============================================================================
// PluginSkillsView - Skills shipped inside agent plugin caches, grouped by
// the plugin that ships them. A separate place from Skills: plugin skills
// update with their plugin and follow the plugin spec, so they get no
// install, edit, or bulk affordances here.
// ============================================================================

import { formatTokens, groupPluginSkills, pluginSkillsView } from "@skill-studio/lib";
import type { InstalledSkill, PluginGroup, SkillSnapshot } from "@skill-studio/lib";
import { PageShell } from "../Shell/PageShell";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";

interface PluginSkillsViewProps {
  snapshot: SkillSnapshot | undefined;
  onSelectSkill: (name: string, deploymentPath?: string) => void;
}

/** The deployment of `skill` that belongs to `group`'s plugin, for the skill page. */
function deploymentPathInGroup(skill: InstalledSkill, group: PluginGroup): string | undefined {
  return skill.deployments.find((d) => d.plugin?.name === group.pluginName)?.path;
}

export function PluginSkillsView({ snapshot, onSelectSkill }: PluginSkillsViewProps) {
  const groups = groupPluginSkills(pluginSkillsView(snapshot?.skills ?? []));
  const skillCount = new Set(groups.flatMap((g) => g.skills.map((s) => s.name))).size;

  return (
    <PageShell
      title="Plugin skills"
      subtitle="Shipped inside agent plugins and updated with them - managed by the plugin, not by Skill Studio."
    >
      {groups.length === 0 ? (
        <p className="m-0 text-small text-text-tertiary">
          No agent plugin ships skills on this machine.
        </p>
      ) : (
        <div className="flex flex-col gap-7">
          <span className="sr-only">{skillCount} plugin skills</span>
          {groups.map((group) => {
            const harnessId = harnessIdFromLabel(group.harness);
            const skills = [...group.skills].sort((a, b) => a.name.localeCompare(b.name));
            return (
              <section key={`${group.harness}-${group.pluginName}`} className="flex flex-col gap-2">
                <header className="flex items-baseline gap-2 px-3">
                  {harnessId && (
                    <span className="self-center">
                      <HarnessIcon harness={harnessId} size={14} />
                    </span>
                  )}
                  <h2 className="m-0 text-body font-semibold text-text-primary">
                    {group.pluginName}
                  </h2>
                  {group.version && (
                    <span className="text-caption text-text-tertiary">v{group.version}</span>
                  )}
                  <span className="ml-auto text-caption tabular-nums text-text-tertiary">
                    {skills.length} skill{skills.length !== 1 ? "s" : ""}
                  </span>
                </header>
                <div className="flex flex-col gap-1.5">
                  {skills.map((skill) => (
                    <button
                      key={skill.name}
                      className="grid h-11 min-w-0 cursor-pointer items-center gap-3 overflow-hidden rounded-md border border-border bg-bg-secondary px-3 text-left transition-colors [grid-template-columns:minmax(0,1fr)_minmax(0,2fr)_64px] hover:bg-bg-hover"
                      onClick={() => onSelectSkill(skill.name, deploymentPathInGroup(skill, group))}
                    >
                      <span className="truncate text-body font-semibold text-text-primary">
                        {skill.name}
                      </span>
                      <span className="truncate text-small text-text-tertiary">
                        {skill.description}
                      </span>
                      <span className="whitespace-nowrap text-right text-caption tabular-nums text-text-tertiary">
                        {formatTokens(skill.skill_md_tokens)}
                      </span>
                    </button>
                  ))}
                </div>
              </section>
            );
          })}
        </div>
      )}
    </PageShell>
  );
}
