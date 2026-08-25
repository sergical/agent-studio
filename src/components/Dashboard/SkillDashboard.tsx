// ============================================================================
// SkillDashboard - The default view: stat cards, top skills, health, and
// agent coverage for the user's own skills. Plugin-shipped skills are
// counted separately and have their own section (see Sidebar/Plugins).
// ============================================================================

import { useMemo } from "react";
import { collectDashboardIssues } from "../../lib/skill-health";
import type { HealthIssueKind } from "../../lib/skill-health";
import { ownSkillsView } from "../../lib/skill-plugin-partition";
import { invocationsInWindow, USAGE_WINDOWS } from "../../lib/skill-stats";
import type { SkillSnapshot } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { WindowSegmentedControl } from "../ui/WindowSegmentedControl";
import { AgentCoverageTable } from "./AgentCoverageTable";
import { DashboardStatStrip } from "./DashboardStatStrip";
import { NeedsAttentionCard } from "./NeedsAttentionCard";
import { TopSkillsList } from "./TopSkillsList";

const DAYS_IN_WINDOW = 30;

interface SkillDashboardProps {
  snapshot: SkillSnapshot | undefined;
  isLoading: boolean;
  onSelectSkill: (name: string) => void;
}

/** Invocation counts for the last 30 days, oldest first, from the heatmap's day map. */
function last30DailyCounts(days: Record<string, number>): number[] {
  const today = new Date();
  today.setHours(0, 0, 0, 0);

  const counts: number[] = [];
  for (let i = DAYS_IN_WINDOW - 1; i >= 0; i--) {
    const date = new Date(today);
    date.setDate(date.getDate() - i);
    const key = date.toISOString().slice(0, 10);
    counts.push(days[key] ?? 0);
  }
  return counts;
}

/**
 * The dashboard: a stat strip (skill count + invocation trend), top skills
 * and a grouped health summary side by side, and agent coverage - all one
 * screen, no skill table. The year-long activity heatmap lives on the
 * Activity page instead (see components/Activity). "Never invoked" is
 * deliberately not surfaced here; it's noise, not something worth fixing for
 * every skill.
 */
export function SkillDashboard({ snapshot, isLoading, onSelectSkill }: SkillDashboardProps) {
  const setActiveView = useAppStore((state) => state.setActiveView);
  const usageWindow = useAppStore((state) => state.usageWindow);
  const setUsageWindow = useAppStore((state) => state.setUsageWindow);

  const own = useMemo(() => ownSkillsView(snapshot?.skills ?? []), [snapshot]);
  const issues = useMemo(() => collectDashboardIssues(own), [own]);

  if (!snapshot) {
    return (
      <div className="dashboard-empty">
        <p>{isLoading ? "Scanning installed skills…" : "No skill snapshot yet."}</p>
      </div>
    );
  }

  const invocationsForWindow = snapshot.invocations.reduce(
    (sum, s) => sum + invocationsInWindow(s, usageWindow),
    0,
  );
  const windowLabel = USAGE_WINDOWS.find((w) => w.id === usageWindow)?.label ?? "30 days";
  const dailyCounts = last30DailyCounts(snapshot.heatmap.days);
  const goToIssues = (issueKind?: HealthIssueKind) => setActiveView({ kind: "issues", issueKind });

  return (
    <div className="dashboard">
      <DashboardStatStrip
        ownCount={own.length}
        invocationsInWindow={invocationsForWindow}
        windowLabel={windowLabel}
        dailyCounts={dailyCounts}
      />

      <div className="dashboard-section-row">
        <div className="dashboard-section">
          <div className="dashboard-section-header">
            <span className="section-label">Top skills</span>
            <WindowSegmentedControl value={usageWindow} onChange={setUsageWindow} />
          </div>
          <TopSkillsList
            skills={snapshot.skills}
            stats={snapshot.invocations}
            window={usageWindow}
            onSelectSkill={onSelectSkill}
          />
        </div>
        <div className="dashboard-section">
          <div className="dashboard-section-header">
            <span className="section-label">Needs attention</span>
            {issues.length > 0 && <span className="dashboard-section-total">{issues.length}</span>}
          </div>
          <NeedsAttentionCard
            issues={issues}
            onSelectKind={goToIssues}
            onSeeAll={() => goToIssues()}
            scannedAt={snapshot.scanned_at}
          />
        </div>
      </div>

      <div className="dashboard-section">
        <span className="section-label">Coverage</span>
        <AgentCoverageTable
          skills={own}
          onSelectMissing={() => setActiveView({ kind: "coverage" })}
        />
      </div>
    </div>
  );
}
