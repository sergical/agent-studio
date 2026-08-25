// ============================================================================
// SkillListTable - Searchable, sortable skill rows, shared by the Global,
// project, and Plugins views
// ============================================================================

import { useMemo, useState } from "react";
import { Link2, Search, Unlink } from "lucide-react";
import { deploymentLinkKind } from "../../lib/skill-coverage";
import { formatBytes } from "../../lib/skill-stats";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import type { InstalledSkill, SkillInvocationStats } from "../../lib/skill-types";

type SortMode = "name" | "used" | "size";

interface SkillListTableProps {
  skills: InstalledSkill[];
  stats: SkillInvocationStats[];
  onSelectSkill: (name: string, deploymentPath?: string) => void;
  selectedSkillName?: string | null;
  /** Plugins view: show each deployment's plugin version instead of the agent label. */
  showPluginVersion?: boolean;
  /** Resolves which deployment a row's click should open in the detail drawer, when the caller knows it. */
  deploymentPathForSkill?: (skill: InstalledSkill) => string | undefined;
}

/**
 * Toolbar (filter + sort) above a list of skill rows: name, one-line
 * description, agent chips (or plugin version chips), 30-day use count, and
 * folder size. Clicking a row selects the skill in the detail drawer.
 */
export function SkillListTable({
  skills,
  stats,
  onSelectSkill,
  selectedSkillName = null,
  showPluginVersion = false,
  deploymentPathForSkill,
}: SkillListTableProps) {
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortMode>("name");
  const statsBySkill = useMemo(() => new Map(stats.map((s) => [s.skill, s])), [stats]);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = q
      ? skills.filter(
          (s) =>
            s.name.toLowerCase().includes(q) || (s.description ?? "").toLowerCase().includes(q),
        )
      : skills;

    const sorted = [...filtered];
    if (sort === "name") {
      sorted.sort((a, b) => a.name.localeCompare(b.name));
    } else if (sort === "used") {
      sorted.sort(
        (a, b) =>
          (statsBySkill.get(b.name)?.last_30_days ?? 0) -
          (statsBySkill.get(a.name)?.last_30_days ?? 0),
      );
    } else {
      sorted.sort((a, b) => b.folder_bytes - a.folder_bytes);
    }
    return sorted;
  }, [skills, query, sort, statsBySkill]);

  return (
    <div className="skill-list-table">
      <div className="skill-list-table-toolbar">
        <div className="skill-list-table-search">
          <Search size={13} />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter by name or description…"
          />
        </div>
        <select
          className="skill-list-table-sort"
          value={sort}
          // SAFETY: the <select>'s options are the three SortMode literals below.
          onChange={(e) => setSort(e.target.value as SortMode)}
        >
          <option value="name">Name</option>
          <option value="used">Most used</option>
          <option value="size">Largest</option>
        </select>
      </div>

      {rows.length === 0 ? (
        <p className="skill-list-table-empty">No skills found</p>
      ) : (
        <div className="skill-list-table-rows">
          {rows.map((skill) => {
            const stat = statsBySkill.get(skill.name);
            return (
              <button
                key={skill.name}
                className={`skill-list-table-row ${skill.name === selectedSkillName ? "selected" : ""}`}
                onClick={() => onSelectSkill(skill.name, deploymentPathForSkill?.(skill))}
              >
                <span className="skill-list-table-name">{skill.name}</span>
                <span className="skill-list-table-description">{skill.description ?? ""}</span>
                <span className="skill-list-table-chips">
                  {skill.deployments.map((d, i) => {
                    const harnessId = harnessIdFromLabel(d.agent);
                    const linkKind = deploymentLinkKind(d);
                    return (
                      <span key={`${d.agent}-${d.scope}-${i}`} className="skill-list-table-chip">
                        {harnessId && <HarnessIcon harness={harnessId} size={12} />}
                        {showPluginVersion && d.plugin?.version ? `v${d.plugin.version}` : d.agent}
                        {linkKind === "linked-to-shared" && (
                          <span
                            className="skill-list-table-chip-marker"
                            title="Symlink to the shared folder"
                          >
                            <Link2 size={10} />
                          </span>
                        )}
                        {linkKind === "broken" && (
                          <span className="skill-list-table-chip-marker broken" title="Broken link">
                            <Unlink size={10} />
                          </span>
                        )}
                      </span>
                    );
                  })}
                </span>
                <span className="skill-list-table-stat">{stat?.last_30_days ?? 0}</span>
                <span className="skill-list-table-stat">{formatBytes(skill.folder_bytes)}</span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
