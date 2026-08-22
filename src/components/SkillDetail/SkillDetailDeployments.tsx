// ============================================================================
// SkillDetailDeployments - Deployment list, provenance, and invocation stats
// ============================================================================

import { AlertTriangle, Link2 } from "lucide-react";
import type { InstalledSkill, SkillInvocationStats } from "../../lib/skill-types";

interface SkillDetailDeploymentsProps {
  skill: InstalledSkill;
  stats: SkillInvocationStats | undefined;
}

function formatDate(dateStr: string | undefined): string {
  if (!dateStr) return "unknown";
  try {
    return new Date(dateStr).toLocaleString();
  } catch {
    return dateStr;
  }
}

/**
 * Every place the skill is deployed on disk, its provenance (lock file
 * source or manual/plugin origin), and its invocation history broken down
 * by project.
 */
export function SkillDetailDeployments({ skill, stats }: SkillDetailDeploymentsProps) {
  return (
    <>
      <div className="skill-detail-section">
        <h4>Deployments</h4>
        <div className="skill-detail-deployment-list">
          {skill.deployments.length === 0 ? (
            <p className="skill-detail-content-empty">Known only from the lock file</p>
          ) : (
            skill.deployments.map((d, i) => (
              <div key={`${d.agent}-${d.scope}-${i}`} className="skill-detail-deployment-row">
                <div className="skill-detail-deployment-row-main">
                  <span className={`scope-badge ${d.scope}`}>{d.scope}</span>
                  <span>{d.agent}</span>
                  {d.project_path && (
                    <span className="skill-detail-fact-label">{d.project_path}</span>
                  )}
                </div>
                <div className="skill-detail-deployment-row-path">{d.path}</div>
                {d.is_symlink && (
                  <div className="skill-detail-deployment-row-symlink">
                    <Link2 size={11} />
                    {d.symlink_is_broken ? (
                      <span className="skill-detail-fact-warning">
                        <AlertTriangle size={11} />
                        broken link{d.symlink_error ? `: ${d.symlink_error}` : ""}
                      </span>
                    ) : (
                      <span>→ {d.symlink_target}</span>
                    )}
                  </div>
                )}
                {d.plugin && (
                  <div className="skill-detail-fact-label">
                    plugin: {d.plugin.name}
                    {d.plugin.version ? ` v${d.plugin.version}` : ""}
                  </div>
                )}
              </div>
            ))
          )}
        </div>
      </div>

      <div className="skill-detail-section">
        <h4>Provenance</h4>
        <div className="skill-detail-facts-grid">
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Source</span>
            <span>{skill.source || "manual/plugin"}</span>
          </div>
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Installed</span>
            <span>{formatDate(skill.installed_at)}</span>
          </div>
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Updated</span>
            <span>{skill.updated_at ? formatDate(skill.updated_at) : "—"}</span>
          </div>
        </div>
      </div>

      <div className="skill-detail-section">
        <h4>Invocations</h4>
        <div className="skill-detail-facts-grid">
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Total</span>
            <span>{stats?.total ?? 0}</span>
          </div>
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Last 30 days</span>
            <span>{stats?.last_30_days ?? 0}</span>
          </div>
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Last used</span>
            <span>{stats?.last_used ? formatDate(stats.last_used) : "never"}</span>
          </div>
        </div>
        {stats && Object.keys(stats.by_project).length > 0 && (
          <div className="skill-detail-frontmatter">
            {Object.entries(stats.by_project).map(([project, count]) => (
              <div key={project} className="skill-detail-frontmatter-row">
                <span className="skill-detail-frontmatter-key">{project}</span>
                <span className="skill-detail-frontmatter-value">{count}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}
