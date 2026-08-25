// ============================================================================
// NeedsAttentionCard - Up to 8 health issues, each linking to its skill
// ============================================================================

import type { HealthIssue, HealthIssueKind } from "../../lib/skill-health";
import { formatRelativeTime } from "../../lib/skill-stats";

const MAX_SHOWN = 8;

/** Dot color per issue kind. "never-invoked" is excluded from this card entirely (see caller). */
const SEVERITY = {
  duplicate: "warning",
  "broken-symlink": "error",
  "spec-violation": "warning",
  "lock-only": "info",
  "never-invoked": "info",
  "missing-from-agents": "warning",
} as const satisfies Record<HealthIssueKind, "error" | "warning" | "info">;

function issueMessage(issue: HealthIssue): string {
  switch (issue.kind) {
    case "duplicate":
      return "Duplicate content across deployments";
    case "broken-symlink":
      return `Broken symlink (${issue.detail})`;
    case "spec-violation":
      return `Spec issue: ${issue.detail}`;
    case "lock-only":
      return "In the lock file but not deployed anywhere";
    case "missing-from-agents":
      return issue.detail;
    case "never-invoked":
      return issue.detail;
  }
}

interface NeedsAttentionCardProps {
  issues: HealthIssue[];
  onSelectSkill: (name: string) => void;
  /** Opens the Global view, for "{n} more" and the empty state's scan time. */
  onSeeAll: () => void;
  scannedAt: string | undefined;
}

/**
 * Up to 8 flagged skills, one line each: a severity dot, a one-line message,
 * and a button with the skill's name that opens the detail drawer. Excludes
 * "never invoked" (the caller shouldn't pass those in - it's noise, not
 * something worth fixing). With zero issues, collapses to a single line so
 * the section doesn't take up a full column.
 */
export function NeedsAttentionCard({
  issues,
  onSelectSkill,
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

  const shown = issues.slice(0, MAX_SHOWN);
  const remaining = issues.length - shown.length;

  return (
    <div className="dashboard-needs-attention">
      {shown.map((issue, i) => (
        <div
          key={`${issue.kind}-${issue.skill.name}-${i}`}
          className="dashboard-needs-attention-row"
        >
          <span className={`dashboard-needs-attention-dot ${SEVERITY[issue.kind]}`} />
          <span className="dashboard-needs-attention-message">{issueMessage(issue)}</span>
          <button
            className="dashboard-needs-attention-skill"
            onClick={() => onSelectSkill(issue.skill.name)}
          >
            {issue.skill.name}
          </button>
        </div>
      ))}
      {remaining > 0 && (
        <button className="dashboard-needs-attention-more" onClick={onSeeAll}>
          {remaining} more →
        </button>
      )}
    </div>
  );
}
