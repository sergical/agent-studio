// ============================================================================
// AgentCoverageRow - One chip per agent, deployed-skill count, links to Coverage
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
 * One row of five chips ("Claude Code 41", "Codex 38", ...) summarizing
 * deployment coverage across the first-class agents plus the shared root.
 * Clicking any chip opens the full Coverage matrix.
 */
export function AgentCoverageRow({ skills, onSelect }: AgentCoverageRowProps) {
  const counts = coverageCounts(skills);

  return (
    <div className="dashboard-agent-coverage-row">
      {AGENT_MATRIX_LABELS.map((label) => (
        <button key={label} className="dashboard-agent-coverage-chip" onClick={onSelect}>
          <span className="dashboard-agent-coverage-chip-label">{label}</span>
          <span className="dashboard-agent-coverage-chip-count">{counts.get(label)}</span>
        </button>
      ))}
    </div>
  );
}
