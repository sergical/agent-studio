// ============================================================================
// SkillList - Searchable, filterable skill rows, shared by the dashboard,
// Global view, and Project view
// ============================================================================

import { useMemo, useState } from "react";
import { AlertTriangle, ChevronDown, ChevronRight, Copy, Link2Off, Search } from "lucide-react";
import { SOURCE_KIND_LABELS } from "../../lib/skill-types";
import type { InstalledSkill, SkillInvocationStats } from "../../lib/skill-types";

interface SkillListProps {
  skills: InstalledSkill[];
  stats: SkillInvocationStats[];
  onSelectSkill: (name: string) => void;
  /** Section title; when set, the list renders as a collapsible group. */
  title?: string;
  defaultCollapsed?: boolean;
  /** Restrict the list to these skill names (e.g. a HealthCard filter). */
  filterNames?: string[] | null;
}

/**
 * Searchable table of installed skills: name, description, source-kind
 * badge, agent chips, tokens, invocations, and health icons (broken
 * symlink, spec violation, duplicate content). Clicking a row selects the
 * skill in the detail panel.
 */
export function SkillList({
  skills,
  stats,
  onSelectSkill,
  title,
  defaultCollapsed = false,
  filterNames = null,
}: SkillListProps) {
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState(defaultCollapsed);
  const statsBySkill = useMemo(() => new Map(stats.map((s) => [s.skill, s])), [stats]);

  const filtered = useMemo(() => {
    let result = skills;
    if (filterNames) {
      const allowed = new Set(filterNames);
      result = result.filter((s) => allowed.has(s.name));
    }
    const q = query.trim().toLowerCase();
    if (q) {
      result = result.filter(
        (s) => s.name.toLowerCase().includes(q) || (s.description ?? "").toLowerCase().includes(q),
      );
    }
    return [...result].sort((a, b) => a.name.localeCompare(b.name));
  }, [skills, query, filterNames]);

  const header = title && (
    <button className="dashboard-skill-list-title" onClick={() => setCollapsed(!collapsed)}>
      {collapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
      <span>{title}</span>
      <span className="dashboard-skill-list-count">{skills.length}</span>
    </button>
  );

  if (title && collapsed) {
    return <div className="dashboard-skill-list">{header}</div>;
  }

  return (
    <div className="dashboard-skill-list">
      {header}
      <div className="dashboard-skill-list-search">
        <Search size={13} />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Filter by name or description…"
        />
      </div>
      {filtered.length === 0 ? (
        <p className="dashboard-skill-list-empty">No skills found</p>
      ) : (
        <div className="dashboard-skill-list-rows">
          {filtered.map((skill) => {
            const stat = statsBySkill.get(skill.name);
            const hasBrokenLink = skill.deployments.some((d) => d.symlink_is_broken);
            const hasDuplicate = skill.content_hashes.length > 1;
            const hasSpecViolation = skill.spec_violations.length > 0;

            return (
              <button
                key={skill.name}
                className="dashboard-skill-list-row"
                onClick={() => onSelectSkill(skill.name)}
              >
                <div className="dashboard-skill-list-row-main">
                  <span className="dashboard-skill-list-row-name">{skill.name}</span>
                  <span className={`skill-card-source-kind ${skill.source_kind}`}>
                    {SOURCE_KIND_LABELS[skill.source_kind]}
                  </span>
                  {hasBrokenLink && (
                    <Link2Off size={12} className="dashboard-skill-list-health-icon error" />
                  )}
                  {hasSpecViolation && (
                    <AlertTriangle size={12} className="dashboard-skill-list-health-icon warning" />
                  )}
                  {hasDuplicate && (
                    <Copy size={12} className="dashboard-skill-list-health-icon warning" />
                  )}
                </div>
                {skill.description && (
                  <p className="dashboard-skill-list-row-description">{skill.description}</p>
                )}
                <div className="dashboard-skill-list-row-meta">
                  {skill.deployments.map((d, i) => (
                    <span key={`${d.agent}-${d.scope}-${i}`} className="skill-card-deployment">
                      {d.agent} · {d.scope}
                    </span>
                  ))}
                  <span className="dashboard-skill-list-row-stat">{skill.skill_md_tokens} tok</span>
                  <span className="dashboard-skill-list-row-stat">
                    {stat?.last_30_days ?? 0} / {stat?.total ?? 0} invocations
                  </span>
                </div>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
