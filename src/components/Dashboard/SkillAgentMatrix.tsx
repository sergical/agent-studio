// ============================================================================
// SkillAgentMatrix - Rows = skills, columns = agents, cell = deployment scope
// ============================================================================

import { AGENT_MATRIX_LABELS, agentMatrix } from "../../lib/skill-stats";
import type { AgentMatrixCell } from "../../lib/skill-stats";
import type { InstalledSkill } from "../../lib/skill-types";

interface SkillAgentMatrixProps {
  skills: InstalledSkill[];
  onSelectSkill: (name: string) => void;
}

const CELL_SYMBOL = {
  global: "●",
  project: "○",
  both: "◑",
} satisfies Record<Exclude<AgentMatrixCell, null>, string>;

/**
 * Skill x agent deployment grid. A filled cell means the skill is deployed
 * for that agent: ● global, ○ project, ◐ both. Clicking a row selects the
 * skill in the detail panel.
 */
export function SkillAgentMatrix({ skills, onSelectSkill }: SkillAgentMatrixProps) {
  const sorted = [...skills].sort((a, b) => a.name.localeCompare(b.name));
  const rows = agentMatrix(sorted);

  return (
    <div className="dashboard-agent-matrix">
      <table>
        <thead>
          <tr>
            <th>Skill</th>
            {AGENT_MATRIX_LABELS.map((label) => (
              <th key={label} title={label}>
                {label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map(({ skill, cells }) => (
            <tr key={skill.name} onClick={() => onSelectSkill(skill.name)}>
              <td className="dashboard-agent-matrix-name">{skill.name}</td>
              {AGENT_MATRIX_LABELS.map((label) => (
                <td
                  key={label}
                  className={`dashboard-agent-matrix-cell ${cells[label] ?? ""}`}
                  title={cells[label] ?? "not deployed"}
                >
                  {cells[label] ? CELL_SYMBOL[cells[label]] : ""}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
