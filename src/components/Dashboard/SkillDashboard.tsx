// ============================================================================
// SkillDashboard - The default view: stat cards, top skills, health, activity,
// and agent coverage for the user's own skills. Plugin-shipped skills are
// counted separately and have their own section (see Sidebar/Plugins).
// ============================================================================

import { useMemo } from "react";
import {
  findBrokenSymlinks,
  findDuplicateSkills,
  findLockOnlySkills,
  findMissingFromAgents,
  findSpecViolations,
} from "../../lib/skill-health";
import { ownSkillsView, pluginSkillsView } from "../../lib/skill-plugin-partition";
import type { SkillSnapshot } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { AgentCoverageRow } from "./AgentCoverageRow";
import { DashboardStatStrip } from "./DashboardStatStrip";
import { InvocationHeatmap } from "./InvocationHeatmap";
import { NeedsAttentionCard } from "./NeedsAttentionCard";
import { TopSkillsList } from "./TopSkillsList";

interface SkillDashboardProps {
  snapshot: SkillSnapshot | undefined;
  isLoading: boolean;
  onSelectSkill: (name: string) => void;
}

/**
 * The dashboard: stat cards, top skills, a health summary ("needs
 * attention"), an invocation heatmap, and an agent coverage row - all one
 * screen, no skill table. "Never invoked" is deliberately not surfaced here;
 * it's noise, not something worth fixing for every skill.
 */
export function SkillDashboard({ snapshot, isLoading, onSelectSkill }: SkillDashboardProps) {
  const setActiveView = useAppStore((state) => state.setActiveView);

  const own = useMemo(() => ownSkillsView(snapshot?.skills ?? []), [snapshot]);
  const plugin = useMemo(() => pluginSkillsView(snapshot?.skills ?? []), [snapshot]);

  const issues = useMemo(
    () => [
      ...findDuplicateSkills(own),
      ...findBrokenSymlinks(own),
      ...findSpecViolations(own),
      ...findLockOnlySkills(own),
      ...findMissingFromAgents(own),
    ],
    [own],
  );

  if (!snapshot) {
    return (
      <div className="dashboard-empty">
        <p>{isLoading ? "Scanning installed skills…" : "No skill snapshot yet."}</p>
      </div>
    );
  }

  const invocations30d = snapshot.invocations.reduce((sum, s) => sum + s.last_30_days, 0);
  const invocationsLastYear = Object.values(snapshot.heatmap.days).reduce(
    (sum, count) => sum + count,
    0,
  );
  const goToGlobal = () => setActiveView({ kind: "global" });

  return (
    <div className="dashboard">
      <DashboardStatStrip
        ownCount={own.length}
        pluginCount={plugin.length}
        invocations30d={invocations30d}
        issuesCount={issues.length}
      />

      <div className="dashboard-section-row">
        <div className="dashboard-section">
          <span className="section-label">Top skills, 30 days</span>
          <TopSkillsList
            skills={snapshot.skills}
            stats={snapshot.invocations}
            onSelectSkill={onSelectSkill}
          />
        </div>
        <div className="dashboard-section">
          <span className="section-label">Needs attention</span>
          <NeedsAttentionCard
            issues={issues}
            onSelectSkill={onSelectSkill}
            onSeeAll={goToGlobal}
            scannedAt={snapshot.scanned_at}
          />
        </div>
      </div>

      <div className="dashboard-section">
        <div className="dashboard-section-header">
          <span className="section-label">Activity</span>
          <span className="dashboard-section-total">
            {invocationsLastYear.toLocaleString()} invocations in the last year
          </span>
        </div>
        <InvocationHeatmap heatmap={snapshot.heatmap} />
      </div>

      <div className="dashboard-section">
        <span className="section-label">Coverage</span>
        <AgentCoverageRow skills={own} onSelect={() => setActiveView({ kind: "coverage" })} />
      </div>
    </div>
  );
}
