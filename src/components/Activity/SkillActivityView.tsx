// ============================================================================
// SkillActivityView - Full-page invocation history: a year heatmap, a
// per-skill table for the shared usage window, and a 30-day per-project
// breakdown
// ============================================================================

import { useMemo } from "react";
import {
  formatRelativeTime,
  invocationsInWindow,
  topSkills,
  USAGE_WINDOWS,
} from "../../lib/skill-stats";
import type { SkillSnapshot } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { WindowSegmentedControl } from "../ui/WindowSegmentedControl";
import { InvocationHeatmap } from "./InvocationHeatmap";

interface SkillActivityViewProps {
  snapshot: SkillSnapshot | undefined;
  onSelectSkill: (name: string) => void;
}

/** Every project path's basename summed over its own `by_project` entries, 30-day totals only. */
function projectTotals(snapshot: SkillSnapshot): { project: string; count: number }[] {
  const totals = new Map<string, number>();
  for (const stat of snapshot.invocations) {
    for (const [project, count] of Object.entries(stat.by_project)) {
      const basename = project.split("/").filter(Boolean).pop() ?? project;
      totals.set(basename, (totals.get(basename) ?? 0) + count);
    }
  }
  return [...totals.entries()]
    .map(([project, count]) => ({ project, count }))
    .sort((a, b) => b.count - a.count || a.project.localeCompare(b.project));
}

/**
 * Full activity history for own and plugin skills, from Claude Code
 * transcripts only. A year-long heatmap, a per-skill table filtered to the
 * shared usage window, and a 30-day per-project breakdown.
 */
export function SkillActivityView({ snapshot, onSelectSkill }: SkillActivityViewProps) {
  const usageWindow = useAppStore((state) => state.usageWindow);
  const setUsageWindow = useAppStore((state) => state.setUsageWindow);

  const invocationsLastYear = useMemo(
    () => Object.values(snapshot?.heatmap.days ?? {}).reduce((sum, count) => sum + count, 0),
    [snapshot],
  );

  const bySkill = useMemo(() => {
    if (!snapshot) return [];
    const used = snapshot.invocations.filter((s) => invocationsInWindow(s, usageWindow) > 0);
    return topSkills(used, used.length, usageWindow);
  }, [snapshot, usageWindow]);

  const byProject = useMemo(() => (snapshot ? projectTotals(snapshot) : []), [snapshot]);

  const windowLabel = USAGE_WINDOWS.find((w) => w.id === usageWindow)?.label ?? "30 days";
  const hasAnyInvocations = snapshot ? snapshot.invocations.some((s) => s.total > 0) : false;

  return (
    <div className="activity-view">
      <div className="activity-view-header">
        <h1>Activity</h1>
        <span className="activity-view-subline">
          From Claude Code transcripts. Codex, OpenCode and pi are not tracked yet.
        </span>
      </div>

      {!snapshot || !hasAnyInvocations ? (
        <p className="activity-view-empty">No invocations recorded yet.</p>
      ) : (
        <>
          <div className="activity-section">
            <div className="activity-section-header">
              <span className="section-label">Activity</span>
              <span className="activity-section-total">
                {invocationsLastYear.toLocaleString()} invocations in the last year
              </span>
            </div>
            <InvocationHeatmap heatmap={snapshot.heatmap} />
          </div>

          <div className="activity-section">
            <div className="activity-section-header">
              <span className="section-label">By skill</span>
              <WindowSegmentedControl value={usageWindow} onChange={setUsageWindow} />
            </div>
            {bySkill.length === 0 ? (
              <p className="activity-view-empty">No invocations in the last {windowLabel}.</p>
            ) : (
              <div className="activity-skill-table">
                {bySkill.map((stat) => (
                  <button
                    key={stat.skill}
                    className="activity-skill-table-row"
                    onClick={() => onSelectSkill(stat.skill)}
                  >
                    <span className="activity-skill-table-name">{stat.skill}</span>
                    <span className="activity-skill-table-last-used">
                      {stat.last_used ? formatRelativeTime(stat.last_used) : "never"}
                    </span>
                    <span className="activity-skill-table-count">
                      {invocationsInWindow(stat, usageWindow)}
                    </span>
                    <span className="activity-skill-table-projects">
                      {Object.keys(stat.by_project).length}
                    </span>
                  </button>
                ))}
              </div>
            )}
          </div>

          <div className="activity-section">
            <span className="section-label">By project, 30 days</span>
            <div className="activity-project-table">
              {byProject.map(({ project, count }) => (
                <div key={project} className="activity-project-table-row">
                  <span className="activity-project-table-name">{project}</span>
                  <span className="activity-project-table-count">{count}</span>
                </div>
              ))}
            </div>
          </div>
        </>
      )}
    </div>
  );
}
