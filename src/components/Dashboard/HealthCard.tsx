// ============================================================================
// HealthCard - Counts of each health issue kind, clickable to filter the list
// ============================================================================

import { AlertTriangle, Copy, Link2Off, PackageX, ShieldAlert, UserX } from "lucide-react";
import type { HealthIssue, HealthIssueKind } from "../../lib/skill-health";

interface HealthCardProps {
  issues: HealthIssue[];
  activeFilter: HealthIssueKind | null;
  onFilter: (kind: HealthIssueKind | null) => void;
}

const KIND_META = {
  duplicate: { label: "Duplicates", icon: Copy },
  "broken-symlink": { label: "Broken symlinks", icon: Link2Off },
  "spec-violation": { label: "Spec issues", icon: ShieldAlert },
  "lock-only": { label: "Lock-only", icon: PackageX },
  "never-invoked": { label: "Never invoked", icon: UserX },
  "missing-from-agents": { label: "Missing from agents", icon: AlertTriangle },
} satisfies Record<HealthIssueKind, { label: string; icon: typeof AlertTriangle }>;

/** Display order for the health badges; keeps `KIND_META`'s keys typed. */
const HEALTH_KIND_ORDER: HealthIssueKind[] = [
  "duplicate",
  "broken-symlink",
  "spec-violation",
  "lock-only",
  "never-invoked",
  "missing-from-agents",
];

/**
 * One badge per health issue kind, showing its count. Clicking a badge
 * filters the skill list below to just the affected skills; clicking the
 * active one again clears the filter.
 */
export function HealthCard({ issues, activeFilter, onFilter }: HealthCardProps) {
  const counts = new Map<HealthIssueKind, number>();
  for (const issue of issues) {
    counts.set(issue.kind, (counts.get(issue.kind) ?? 0) + 1);
  }

  return (
    <div className="dashboard-health-card">
      {HEALTH_KIND_ORDER.map((kind) => {
        const count = counts.get(kind) ?? 0;
        const { label, icon: Icon } = KIND_META[kind];
        return (
          <button
            key={kind}
            className={`dashboard-health-badge ${count > 0 ? "has-issues" : ""} ${
              activeFilter === kind ? "active" : ""
            }`}
            disabled={count === 0}
            onClick={() => onFilter(activeFilter === kind ? null : kind)}
          >
            <Icon size={13} />
            <span>{label}</span>
            <span className="dashboard-health-badge-count">{count}</span>
          </button>
        );
      })}
    </div>
  );
}
