// ============================================================================
// NeedsAttentionCard - Grouped issue-kind summary, linking to the Issues view
// ============================================================================

import {
  HEALTH_ISSUE_KIND_LABEL,
  HEALTH_ISSUE_SEVERITY,
  groupIssuesByKind,
} from "../../lib/skill-health";
import type { HealthIssue, HealthIssueKind } from "../../lib/skill-health";
import { formatRelativeTime } from "../../lib/skill-stats";

const MAX_GROUPS = 5;

interface NeedsAttentionCardProps {
  issues: HealthIssue[];
  /** Opens the Issues view, pre-filtered to one kind. */
  onSelectKind: (kind: HealthIssueKind) => void;
  /** Opens the Issues view, unfiltered ("View all issues"). */
  onSeeAll: () => void;
  scannedAt: string | undefined;
}

/**
 * One row per issue kind present (max 5), each a count and a label rather
 * than a per-issue line - the detail lives in the Issues view now. Rows open
 * the Issues view pre-filtered to that kind; the footer opens it unfiltered.
 * With zero issues, collapses to a single line so the section doesn't take
 * up a full column.
 */
export function NeedsAttentionCard({
  issues,
  onSelectKind,
  onSeeAll,
  scannedAt,
}: NeedsAttentionCardProps) {
  if (issues.length === 0) {
    return (
      <p className="dashboard-needs-attention-empty">
        Nothing needs attention · scanned {scannedAt ? formatRelativeTime(scannedAt) : "never"}
      </p>
    );
  }

  const groups = groupIssuesByKind(issues).slice(0, MAX_GROUPS);

  return (
    <div className="dashboard-needs-attention">
      {groups.map(({ kind, count }) => {
        const label = HEALTH_ISSUE_KIND_LABEL[kind];
        return (
          <button
            key={kind}
            className="dashboard-needs-attention-row"
            onClick={() => onSelectKind(kind)}
          >
            <span className={`dashboard-needs-attention-dot ${HEALTH_ISSUE_SEVERITY[kind]}`} />
            <span className="dashboard-needs-attention-count">{count}</span>
            <span className="dashboard-needs-attention-label">
              {count === 1 ? label.singular : label.plural}
            </span>
          </button>
        );
      })}
      <button className="dashboard-needs-attention-more" onClick={onSeeAll}>
        View all issues →
      </button>
    </div>
  );
}
