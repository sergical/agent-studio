// ============================================================================
// SkillCoverageMatrix - Rows = skills, columns = shared root + agents, cell =
// effective visibility (own folder, via the shared root, or none)
// ============================================================================

import { AGENT_MATRIX_LABELS, agentMatrix } from "@skill-studio/lib";
import type { AgentMatrixCell } from "@skill-studio/lib";
import type { InstalledSkill } from "@skill-studio/lib";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import { TooltipControl } from "../ui/TooltipControl";

interface SkillCoverageMatrixProps {
  skills: InstalledSkill[];
  onSelectSkill: (name: string) => void;
}

/** Tooltip / cell title text, shared by the header-adjacent legend and each cell. */
function cellTitle(cell: AgentMatrixCell): string {
  const base =
    cell.state === "own"
      ? "In the agent's folder"
      : cell.state === "shared"
        ? "Via the shared .agents folder"
        : "Not deployed";
  // A broken own-directory link doesn't hide effective visibility (e.g. a
  // healthy shared fallback), so the tooltip states both facts rather than
  // just "Broken link".
  return cell.isBroken ? `${base} (broken link)` : base;
}

/**
 * The marker swatch for one cell: filled square (own), hollow dotted square
 * (shared), or empty (none), with a red ring outline layered on top when
 * `isBroken` - the ring is the broken marker, the fill/outline underneath
 * still shows the effective visibility state.
 */
const MARKER_BY_STATE = {
  own: "bg-accent",
  shared:
    "border-[1.5px] border-accent/60 after:absolute after:top-1/2 after:left-1/2 after:size-[3px] after:-translate-x-1/2 after:-translate-y-1/2 after:rounded-full after:bg-accent/60 after:content-['']",
  none: "border border-border-subtle",
} satisfies Record<AgentMatrixCell["state"], string>;

function CoverageMarker({ cell }: { cell: AgentMatrixCell }) {
  return (
    <span
      className={`relative inline-block size-[9px] rounded-[2px] align-middle ${MARKER_BY_STATE[cell.state]} ${
        cell.isBroken ? "outline outline-[1.5px] outline-offset-[1.5px] outline-error" : ""
      }`}
    />
  );
}

/**
 * Skill x column visibility grid: a "Shared" column for the shared `.agents`
 * root, then one column per first-class agent. Clicking a row selects the
 * skill in the detail panel. The header row stays pinned while scrolling.
 */
const HEADER_CELL_CLASS =
  "sticky top-0 z-10 h-9 border-b border-border-subtle bg-bg-secondary px-2.5 text-center text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase";
const CELL_CLASS = "h-9 border-b border-border-subtle px-2.5 group-hover:bg-bg-hover";

export function SkillCoverageMatrix({ skills, onSelectSkill }: SkillCoverageMatrixProps) {
  const sorted = [...skills].sort((a, b) => a.name.localeCompare(b.name));
  const rows = agentMatrix(sorted);

  return (
    <div className="max-h-[calc(100vh-160px)] overflow-auto rounded-md border border-border">
      <table className="w-full border-collapse text-small">
        <thead>
          <tr>
            <th className={HEADER_CELL_CLASS}>Skill</th>
            <th className={HEADER_CELL_CLASS}>
              <TooltipControl content="Shared .agents folder">
                <span
                  aria-label="Shared .agents folder"
                  className="inline-flex items-center justify-center"
                >
                  <HarnessIcon harness="shared" size={15} />
                </span>
              </TooltipControl>
            </th>
            {AGENT_MATRIX_LABELS.map((label) => {
              const harnessId = harnessIdFromLabel(label);
              return (
                <th key={label} className={HEADER_CELL_CLASS}>
                  <TooltipControl content={label}>
                    <span aria-label={label} className="inline-flex items-center justify-center">
                      {harnessId ? <HarnessIcon harness={harnessId} size={15} /> : label}
                    </span>
                  </TooltipControl>
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {rows.map(({ skill, shared, cells }) => (
            <tr
              key={skill.name}
              className="group cursor-pointer"
              tabIndex={0}
              role="button"
              onClick={() => onSelectSkill(skill.name)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onSelectSkill(skill.name);
                }
              }}
            >
              <td className={`${CELL_CLASS} text-left text-text-primary`}>{skill.name}</td>
              <td className={`${CELL_CLASS} text-center`} title={cellTitle(shared)}>
                <CoverageMarker cell={shared} />
              </td>
              {AGENT_MATRIX_LABELS.map((label) => (
                <td
                  key={label}
                  className={`${CELL_CLASS} text-center`}
                  title={cellTitle(cells[label])}
                >
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
