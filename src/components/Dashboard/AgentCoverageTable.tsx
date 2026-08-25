// ============================================================================
// AgentCoverageTable - Two rows: what Claude Code sees in its own folder,
// and what the shared .agents folder makes visible to Codex, OpenCode and pi
// ============================================================================

import { summarizeCoverage } from "../../lib/skill-coverage";
import { HarnessIcon } from "../ui/HarnessIcon";
import type { AgentId, InstalledSkill } from "../../lib/skill-types";

interface AgentCoverageTableProps {
  /** Own skills only - plugin skills don't count toward agent coverage. */
  skills: InstalledSkill[];
  /** Opens the full Coverage matrix, from a row's "N missing" link. */
  onSelectMissing: () => void;
}

/** Display label for each agent that reads the shared `.agents` root, for the "only in own dir" line. */
const SHARED_READER_LABELS = {
  codex: "Codex",
  "open-code": "OpenCode",
  pi: "pi",
} satisfies Partial<Record<AgentId, string>>;

/** The display label for one of the three shared-reading agents, falling back to its id. */
function sharedReaderLabel(agent: AgentId): string {
  if (agent in SHARED_READER_LABELS) {
    // SAFETY: just checked `agent` is one of SHARED_READER_LABELS' own keys.
    return SHARED_READER_LABELS[agent as keyof typeof SHARED_READER_LABELS];
  }
  return agent;
}

/** "+1 only in OpenCode" / "+3 only in Codex, pi", or null when every skill in an own dir is also shared. */
function onlyInOwnDirLine(onlyInOwnDir: Partial<Record<AgentId, number>>): string | null {
  // SAFETY: `onlyInOwnDir` is only ever built by keying on `AgentId` values
  // (see summarizeCoverage), so its own keys are AgentId, not plain string.
  const entries = Object.entries(onlyInOwnDir) as [AgentId, number][];
  if (entries.length === 0) return null;
  const total = entries.reduce((sum, [, count]) => sum + count, 0);
  const names = entries.map(([agent]) => sharedReaderLabel(agent));
  return `+${total} only in ${names.join(", ")}`;
}

interface CoverageRowProps {
  harness: "claude-code" | "shared";
  label: string;
  sublines: string[];
  visible: number;
  total: number;
  onSelectMissing: () => void;
}

/** One coverage row: icon, label (with optional tertiary sublines), a thin bar, "n of total", and a missing link. */
function CoverageRow({
  harness,
  label,
  sublines,
  visible,
  total,
  onSelectMissing,
}: CoverageRowProps) {
  const missing = total - visible;
  const pct = total > 0 ? (visible / total) * 100 : 0;

  return (
    <div className="coverage-table-row">
      <span className="coverage-table-agent">
        <HarnessIcon harness={harness} size={13} />
        <span className="coverage-table-agent-text">
          <span className="coverage-table-agent-label">{label}</span>
          {sublines.map((line) => (
            <span key={line} className="coverage-table-agent-subline">
              {line}
            </span>
          ))}
        </span>
      </span>
      <span className="coverage-table-bar-track">
        <span className="coverage-table-bar-fill" style={{ width: `${pct}%` }} />
      </span>
      <span className="coverage-table-count">
        {visible} of {total}
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
}

/**
 * A compact table, one row for Claude Code's own folder and one for the
 * shared `.agents` folder that Codex, OpenCode and pi read natively: a thin
 * proportional bar, "N of total" own skills visible, and either "N missing →"
 * (opens the Coverage matrix) or "all". Rows aren't interactive themselves -
 * only the missing-count link is.
 */
export function AgentCoverageTable({ skills, onSelectMissing }: AgentCoverageTableProps) {
  const summary = summarizeCoverage(skills);
  const { total } = summary;
  const onlyInOwnDir = onlyInOwnDirLine(summary.shared.onlyInOwnDir);
  const everyRowComplete = summary.claudeCode.missing === 0 && summary.shared.missing === 0;

  return (
    <div className="coverage-table-wrap">
      <p className="coverage-table-caption">
        Which of your {total} skills each harness can see. Claude Code reads its own folder; Codex,
        OpenCode and pi read the shared .agents folder.
      </p>
      <div className="coverage-table">
        <CoverageRow
          harness="claude-code"
          label="Claude Code"
          sublines={[]}
          visible={summary.claudeCode.visible}
          total={total}
          onSelectMissing={onSelectMissing}
        />
        <CoverageRow
          harness="shared"
          label="Shared folder"
          sublines={
            onlyInOwnDir
              ? ["Read by Codex, OpenCode and pi", onlyInOwnDir]
              : ["Read by Codex, OpenCode and pi"]
          }
          visible={summary.shared.visible}
          total={total}
          onSelectMissing={onSelectMissing}
        />
      </div>
      {everyRowComplete && (
        <button className="coverage-table-view-all" onClick={onSelectMissing}>
          View coverage →
        </button>
      )}
    </div>
  );
}
