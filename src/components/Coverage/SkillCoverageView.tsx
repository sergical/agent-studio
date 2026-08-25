// ============================================================================
// SkillCoverageView - The skill x agent matrix, filterable, with a legend
// ============================================================================

import { useMemo, useState } from "react";
import { Search } from "lucide-react";
import { ownSkillsView } from "../../lib/skill-plugin-partition";
import type { SkillSnapshot } from "../../lib/skill-types";
import { SkillCoverageMatrix } from "./SkillCoverageMatrix";

interface SkillCoverageViewProps {
  snapshot: SkillSnapshot | undefined;
  onSelectSkill: (name: string) => void;
}

/**
 * Full-page coverage matrix: a name filter, a legend for the three markers,
 * and the sticky-header `SkillCoverageMatrix` below. Own skills only -
 * plugin-shipped skills are managed by the harness, not tracked for coverage.
 */
export function SkillCoverageView({ snapshot, onSelectSkill }: SkillCoverageViewProps) {
  const [query, setQuery] = useState("");
  const own = useMemo(() => ownSkillsView(snapshot?.skills ?? []), [snapshot]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q ? own.filter((s) => s.name.toLowerCase().includes(q)) : own;
  }, [own, query]);

  return (
    <div className="coverage-view">
      <div className="coverage-view-header">
        <h2>Coverage</h2>
        <div className="coverage-view-filter">
          <Search size={13} />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter by name…"
          />
        </div>
        <div className="coverage-view-legend">
          <span>
            <span className="coverage-legend-marker global">●</span> global
          </span>
          <span>
            <span className="coverage-legend-marker project">○</span> project
          </span>
          <span>
            <span className="coverage-legend-marker both">◑</span> both
          </span>
        </div>
      </div>
      <SkillCoverageMatrix skills={filtered} onSelectSkill={onSelectSkill} />
    </div>
  );
}
