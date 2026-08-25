// ============================================================================
// SkillCoverageMatrix - Rows = skills, columns = shared root + agents, cell =
// effective visibility (own folder, via the shared root, or none)
// ============================================================================

import { AGENT_MATRIX_LABELS, agentMatrix } from "../../lib/skill-coverage";
import type { AgentMatrixCell } from "../../lib/skill-coverage";
import type { InstalledSkill } from "../../lib/skill-types";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";

interface SkillCoverageMatrixProps {
  skills: InstalledSkill[];
  onSelectSkill: (name: string) => void;
}

/** Tooltip / cell title text, shared by the header-adjacent legend and each cell. */
function cellTitle(cell: AgentMatrixCell): string {
  if (cell.isBroken) return "Broken link";
  if (cell.state === "own") return "In the agent's folder";
  if (cell.state === "shared") return "Via the shared .agents folder";
  return "Not deployed";
}

/** The marker swatch for one cell: filled square (own), hollow dotted square (shared), ring (broken), or empty. */
function CoverageMarker({ cell }: { cell: AgentMatrixCell }) {
  const className = cell.isBroken ? "broken" : cell.state;
  return <span className={`coverage-matrix-marker ${className}`} />;
}

/**
 * Skill x column visibility grid: a "Shared" column for the shared `.agents`
 * root, then one column per first-class agent. Clicking a row selects the
 * skill in the detail panel. The header row stays pinned while scrolling.
 */
export function SkillCoverageMatrix({ skills, onSelectSkill }: SkillCoverageMatrixProps) {
  const sorted = [...skills].sort((a, b) => a.name.localeCompare(b.name));
  const rows = agentMatrix(sorted);

  return (
    <div className="coverage-matrix">
      <table>
        <thead>
          <tr>
            <th>Skill</th>
            <th title="Shared .agents folder">
              <span className="coverage-matrix-th-label">
                <HarnessIcon harness="shared" size={13} />
                Shared
              </span>
            </th>
            {AGENT_MATRIX_LABELS.map((label) => {
              const harnessId = harnessIdFromLabel(label);
              return (
                <th key={label} title={label}>
                  <span className="coverage-matrix-th-label">
                    {harnessId && <HarnessIcon harness={harnessId} size={13} />}
                    {label}
                  </span>
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {rows.map(({ skill, shared, cells }) => (
            <tr key={skill.name} onClick={() => onSelectSkill(skill.name)}>
              <td className="coverage-matrix-name">{skill.name}</td>
              <td className="coverage-matrix-cell" title={cellTitle(shared)}>
                <CoverageMarker cell={shared} />
              </td>
              {AGENT_MATRIX_LABELS.map((label) => (
                <td key={label} className="coverage-matrix-cell" title={cellTitle(cells[label])}>
                  <CoverageMarker cell={cells[label]} />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
