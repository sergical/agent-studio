// ============================================================================
// SkillActivityView - Full-page invocation history: a year heatmap, a
// per-skill table for the shared usage window, and a 30-day per-project
// breakdown
// ============================================================================

import { useMemo } from "react";
import {
  formatRelativeTime,
  heatmapDateRangeUtc,
  invocationsInWindow,
  topSkills,
  USAGE_WINDOWS,
} from "../../lib/skill-stats";
import type { SkillSnapshot } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { PageShell } from "../Shell/PageShell";
import { WindowSegmentedControl } from "../ui/WindowSegmentedControl";
import { InvocationHeatmap } from "./InvocationHeatmap";

interface SkillActivityViewProps {
  snapshot: SkillSnapshot | undefined;
  onSelectSkill: (name: string) => void;
}

/**
 * Every project's 30-day invocation total, keyed and sorted by full path so
 * two projects with the same basename (e.g. two checkouts of the same repo)
 * stay separate rows; `label` is the basename shown in the row.
 */
function projectTotals(
  snapshot: SkillSnapshot,
): { project: string; label: string; count: number }[] {
  const totals = new Map<string, number>();
  for (const stat of snapshot.invocations) {
    for (const [project, count] of Object.entries(stat.by_project_30_days)) {
      totals.set(project, (totals.get(project) ?? 0) + count);
    }
  }
  return [...totals.entries()]
    .map(([project, count]) => ({
      project,
      label: project.split("/").filter(Boolean).pop() ?? project,
      count,
    }))
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

  // Anchored to the snapshot's own scan time (not just the render time), and
  // re-derived whenever its UTC date changes, so the grid still advances past
  // UTC midnight instead of freezing at whatever date the component first
  // mounted.
  const scannedAtUtcDateKey = (snapshot?.scanned_at ? new Date(snapshot.scanned_at) : new Date())
    .toISOString()
    .slice(0, 10);
  // Computed once here and passed down to InvocationHeatmap, so the header
  // total, the grid, and its aria-label all sum over the exact same 364-day
  // UTC key list rather than three independently-computed ranges. Rebuilt
  // from `scannedAtUtcDateKey` alone (midnight UTC of that date) rather than
  // depending on a fresh `Date` instance, which changes identity every
  // render even when the UTC date hasn't.
  const heatmapDates = useMemo(
    () => heatmapDateRangeUtc(new Date(`${scannedAtUtcDateKey}T00:00:00Z`)).dates,
    [scannedAtUtcDateKey],
  );
  const invocationsLastYear = useMemo(
    () => heatmapDates.reduce((sum, key) => sum + (snapshot?.heatmap.days[key] ?? 0), 0),
    [heatmapDates, snapshot],
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
    <PageShell
      title="Activity"
      subtitle="From Claude Code transcripts. Codex, OpenCode and pi are not tracked yet."
    >
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
            <InvocationHeatmap heatmap={snapshot.heatmap} dates={heatmapDates} />
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
                    <span className="activity-skill-table-name" title={stat.skill}>
                      {stat.skill}
                    </span>
                    <span className="activity-skill-table-last-used">
                      {stat.last_used ? formatRelativeTime(stat.last_used) : "never"}
                    </span>
                    <span className="activity-skill-table-count">
                      {invocationsInWindow(stat, usageWindow)}
                    </span>
                    <span className="activity-skill-table-projects">
                      {Object.keys(stat.by_project_30_days).length}
                    </span>
                  </button>
                ))}
              </div>
            )}
          </div>

          <div className="activity-section">
            <span className="section-label">By project, 30 days</span>
            <div className="activity-project-table">
              {byProject.map(({ project, label, count }) => (
                <div key={project} className="activity-project-table-row" title={project}>
                  <span className="activity-project-table-name">{label}</span>
                  <span className="activity-project-table-count">{count}</span>
                </div>
              ))}
            </div>
          </div>
        </>
      )}
    </PageShell>
  );
}
