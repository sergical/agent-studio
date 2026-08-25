// ============================================================================
// SkillIssuesView - Every health issue across the user's own skills, grouped
// by kind with filter chips and a flat table
// ============================================================================

import { useMemo } from "react";
import {
  collectDashboardIssues,
  deploymentLabel,
  HEALTH_ISSUE_KIND_LABEL,
  HEALTH_ISSUE_SEVERITY,
  groupIssuesByKind,
  type HealthIssue,
  type HealthIssueKind,
} from "../../lib/skill-health";
import { ownSkillsView } from "../../lib/skill-plugin-partition";
import { formatRelativeTime } from "../../lib/skill-stats";
import type { SkillSnapshot } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";

interface SkillIssuesViewProps {
  snapshot: SkillSnapshot | undefined;
  issueKind: HealthIssueKind | undefined;
  onSelectSkill: (name: string) => void;
}

/** One line describing what's wrong, independent of where it happened. */
function whatIsWrong(issue: HealthIssue): string {
  switch (issue.kind) {
    case "duplicate":
      return issue.detail;
    case "broken-symlink":
      return "Broken symlink";
    case "spec-violation":
      return issue.detail;
    case "lock-only":
      return "In the lock file but not deployed anywhere";
    case "missing-from-agents":
      return issue.detail.includes(": missing from ")
        ? `Missing from ${issue.detail.split(": missing from ")[1]}`
        : issue.detail;
    case "never-invoked":
      return issue.detail;
  }
}

/** Scope/agent label pulled out of the issue's detail, when it names one. */
function issueLocation(issue: HealthIssue): string {
  if (issue.kind === "broken-symlink") {
    return issue.detail.split(" · ")[0] ?? "—";
  }
  if (issue.kind === "missing-from-agents") {
    return issue.detail.split(": missing from")[0] ?? "—";
  }
  const deployment = issue.skill.deployments[0];
  return deployment ? deploymentLabel(deployment) : "—";
}

/**
 * Full-page issues list: a subline count, filter chips per issue kind (deep
 * linkable from the dashboard's grouped summary via `issueKind`), and a flat
 * table of every issue sorted the same way `collectDashboardIssues` returns
 * them. Rows open the skill in the detail drawer.
 */
export function SkillIssuesView({ snapshot, issueKind, onSelectSkill }: SkillIssuesViewProps) {
  const setActiveView = useAppStore((state) => state.setActiveView);

  const own = useMemo(() => ownSkillsView(snapshot?.skills ?? []), [snapshot]);
  const allIssues = useMemo(() => collectDashboardIssues(own), [own]);
  const groups = useMemo(() => groupIssuesByKind(allIssues), [allIssues]);
  const shownIssues = issueKind ? allIssues.filter((i) => i.kind === issueKind) : allIssues;
  const affectedSkills = new Set(allIssues.map((i) => i.skill.name)).size;

  const setKind = (kind: HealthIssueKind | undefined) =>
    setActiveView({ kind: "issues", issueKind: kind });

  return (
    <div className="issues-view">
      <div className="issues-view-header">
        <h1>Issues</h1>
        <span className="issues-view-subline">
          {allIssues.length} issues across {affectedSkills} of your skills
        </span>
      </div>

      <div className="issues-view-chips">
        <button
          className={`issues-view-chip ${issueKind === undefined ? "active" : ""}`}
          aria-pressed={issueKind === undefined}
          onClick={() => setKind(undefined)}
        >
          All {allIssues.length}
        </button>
        {groups.map(({ kind, count }) => (
          <button
            key={kind}
            className={`issues-view-chip ${issueKind === kind ? "active" : ""}`}
            aria-pressed={issueKind === kind}
            onClick={() => setKind(kind)}
          >
            {HEALTH_ISSUE_KIND_LABEL[kind].plural} {count}
          </button>
        ))}
      </div>

      {shownIssues.length === 0 ? (
        <p className="issues-view-empty">
          Nothing needs attention · scanned{" "}
          {snapshot?.scanned_at ? formatRelativeTime(snapshot.scanned_at) : "never"}
        </p>
      ) : (
        <div className="issue-table">
          {shownIssues.map((issue, i) => (
            <button
              key={`${issue.kind}-${issue.skill.name}-${i}`}
              className="issue-table-row"
              onClick={() => onSelectSkill(issue.skill.name)}
            >
              <span className={`issue-table-severity ${HEALTH_ISSUE_SEVERITY[issue.kind]}`} />
              <span className="issue-table-skill">{issue.skill.name}</span>
              <span className="issue-table-message" title={whatIsWrong(issue)}>
                {whatIsWrong(issue)}
              </span>
              <span className="issue-table-where">{issueLocation(issue)}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
