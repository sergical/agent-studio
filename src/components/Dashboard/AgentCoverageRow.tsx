// ============================================================================
// AgentCoverageRow - One coverage line plus a link to the full Coverage view
// ============================================================================

import { AGENT_MATRIX_LABELS, agentMatrix } from "../../lib/skill-stats";
import type { InstalledSkill } from "../../lib/skill-types";

interface AgentCoverageRowProps {
  /** Own skills only - plugin skills don't count toward agent coverage. */
  skills: InstalledSkill[];
  onSelect: () => void;
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
 * One line summarizing deployment coverage across the first-class agents
 * plus the shared root ("Claude Code 41 · Codex 38 · ..."), followed by a
 * text link that opens the full Coverage matrix.
 */
export function AgentCoverageRow({ skills, onSelect }: AgentCoverageRowProps) {
  const counts = coverageCounts(skills);

  return (
    <div>
      <p className="dashboard-coverage-line">
        {AGENT_MATRIX_LABELS.map((label, i) => (
          <span key={label}>
            {i > 0 && <span className="dashboard-coverage-sep">·</span>}
            <span className="dashboard-coverage-agent">{label}</span>
            <span className="dashboard-coverage-count">{counts.get(label)}</span>
          </span>
        ))}
      </p>
      <button className="dashboard-coverage-link" onClick={onSelect}>
        View coverage →
      </button>
    </div>
  );
}
