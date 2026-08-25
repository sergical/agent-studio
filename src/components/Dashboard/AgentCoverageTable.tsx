// ============================================================================
// AgentCoverageTable - One row per first-class agent, how many own skills
// each can see, and a link to the full Coverage matrix for what's missing
// ============================================================================

import { AGENT_MATRIX_LABELS, agentMatrix } from "../../lib/skill-stats";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import type { InstalledSkill } from "../../lib/skill-types";

interface AgentCoverageTableProps {
  /** Own skills only - plugin skills don't count toward agent coverage. */
  skills: InstalledSkill[];
  /** Opens the full Coverage matrix, from a row's "N missing" link. */
  onSelectMissing: () => void;
}

/** How many of `skills` are deployed to each agent, in `AGENT_MATRIX_LABELS` order. */
function coverageCounts(skills: InstalledSkill[]): Map<string, number> {
  const rows = agentMatrix(skills);
  const counts = new Map(AGENT_MATRIX_LABELS.map((label) => [label, 0]));
  for (const row of rows) {
    for (const label of AGENT_MATRIX_LABELS) {
      if (row.cells[label]) counts.set(label, (counts.get(label) ?? 0) + 1);
    }
  }
  return counts;
}

/**
 * A compact table, one row per first-class agent: a thin proportional bar,
 * "N of total" own skills it can see, and either "N missing →" (opens the
 * Coverage matrix) or "all" when it sees every own skill. Rows aren't
 * interactive themselves - only the missing-count link is.
 */
export function AgentCoverageTable({ skills, onSelectMissing }: AgentCoverageTableProps) {
  const counts = coverageCounts(skills);
  const total = skills.length;
  const everyAgentComplete = AGENT_MATRIX_LABELS.every(
    (label) => (counts.get(label) ?? 0) === total,
  );

  return (
    <div className="coverage-table-wrap">
      <p className="coverage-table-caption">Which of your {total} skills each agent can see.</p>
      <div className="coverage-table">
        {AGENT_MATRIX_LABELS.map((label) => {
          const covered = counts.get(label) ?? 0;
          const missing = total - covered;
          const pct = total > 0 ? (covered / total) * 100 : 0;
          return (
            <div key={label} className="coverage-table-row">
              <span className="coverage-table-agent">
                {harnessIdFromLabel(label) && (
                  <HarnessIcon harness={harnessIdFromLabel(label)!} size={13} />
                )}
                {label}
              </span>
              <span className="coverage-table-bar-track">
                <span className="coverage-table-bar-fill" style={{ width: `${pct}%` }} />
              </span>
              <span className="coverage-table-count">
                {covered} of {total}
              </span>
              {missing > 0 ? (
                <button className="coverage-table-missing" onClick={onSelectMissing}>
                  {missing} missing →
                </button>
              ) : (
                <span className="coverage-table-all">all</span>
              )}
            </div>
          );
        })}
      </div>
      {everyAgentComplete && (
        <button className="coverage-table-view-all" onClick={onSelectMissing}>
          View coverage →
        </button>
      )}
    </div>
  );
}
