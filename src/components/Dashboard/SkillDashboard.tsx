// ============================================================================
// SkillDashboard - The default view: stats, agent matrix, activity, health,
// and the full skill list grouped by scope
// ============================================================================

import { useMemo, useState } from "react";
import {
  findBrokenSymlinks,
  findDuplicateSkills,
  findLockOnlySkills,
  findMissingFromAgents,
  findNeverInvoked,
  findSpecViolations,
} from "../../lib/skill-health";
import type { HealthIssueKind } from "../../lib/skill-health";
import { computeTotals } from "../../lib/skill-stats";
import type { SkillSnapshot } from "../../lib/skill-types";
import { HealthCard } from "./HealthCard";
import { InvocationHeatmap } from "./InvocationHeatmap";
import { SkillAgentMatrix } from "./SkillAgentMatrix";
import { SkillList } from "./SkillList";
import { StatCards } from "./StatCards";
import { TopSkillsList } from "./TopSkillsList";

interface SkillDashboardProps {
  snapshot: SkillSnapshot | undefined;
  isLoading: boolean;
  onSelectSkill: (name: string) => void;
}

/**
 * The dashboard: stat cards, the skill x agent matrix, an invocation
 * heatmap, top skills, a health summary, and the full skill list split by
 * scope (Global, then one collapsible section per project).
 */
export function SkillDashboard({ snapshot, isLoading, onSelectSkill }: SkillDashboardProps) {
  const [healthFilter, setHealthFilter] = useState<HealthIssueKind | null>(null);

  const issues = useMemo(() => {
    if (!snapshot) return [];
    return [
      ...findDuplicateSkills(snapshot.skills),
      ...findBrokenSymlinks(snapshot.skills),
      ...findSpecViolations(snapshot.skills),
      ...findLockOnlySkills(snapshot.skills),
      ...findNeverInvoked(snapshot.skills, snapshot.invocations),
      ...findMissingFromAgents(snapshot.skills),
    ];
  }, [snapshot]);

  if (!snapshot) {
    return (
      <div className="dashboard-empty">
        <p>{isLoading ? "Scanning installed skills…" : "No skill snapshot yet."}</p>
      </div>
    );
  }

  const totals = computeTotals(snapshot);
  const filterNames = healthFilter
    ? issues.filter((i) => i.kind === healthFilter).map((i) => i.skill.name)
    : null;

  const globalSkills = snapshot.skills.filter((s) =>
    s.deployments.some((d) => d.scope === "global" || d.scope === "plugin"),
  );

  return (
    <div className="dashboard">
      <StatCards totals={totals} />

      <div className="dashboard-section">
        <h3>Deployment matrix</h3>
        <SkillAgentMatrix skills={snapshot.skills} onSelectSkill={onSelectSkill} />
      </div>

      <div className="dashboard-section-row">
        <div className="dashboard-section">
          <h3>Activity</h3>
          <InvocationHeatmap heatmap={snapshot.heatmap} />
        </div>
        <div className="dashboard-section">
          <h3>Top skills (30d)</h3>
          <TopSkillsList stats={snapshot.invocations} onSelectSkill={onSelectSkill} />
        </div>
      </div>

      <div className="dashboard-section">
        <h3>Health</h3>
        <HealthCard issues={issues} activeFilter={healthFilter} onFilter={setHealthFilter} />
      </div>

      <div className="dashboard-section">
        <SkillList
          skills={globalSkills}
          stats={snapshot.invocations}
          onSelectSkill={onSelectSkill}
          title="Global"
          filterNames={filterNames}
        />
        {snapshot.projects.map((path) => (
          <SkillList
            key={path}
            skills={snapshot.skills.filter((s) =>
              s.deployments.some((d) => d.project_path === path),
            )}
            stats={snapshot.invocations}
            onSelectSkill={onSelectSkill}
            title={path.split("/").filter(Boolean).pop() ?? path}
            defaultCollapsed
            filterNames={filterNames}
          />
        ))}
      </div>
    </div>
  );
}
