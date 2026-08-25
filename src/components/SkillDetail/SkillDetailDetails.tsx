// ============================================================================
// SkillDetailDetails - Collapsed-by-default <details>: locations, invocation
// control frontmatter, and source/install metadata
// ============================================================================

import { AlertTriangle, FolderOpen, Link2 } from "lucide-react";
import { openSkillPath } from "../../lib/skill-api";
import { formatRelativeTime } from "../../lib/skill-stats";
import type { InstalledSkill } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";

interface SkillDetailDetailsProps {
  skill: InstalledSkill;
}

/** Frontmatter keys that gate model auto-invocation; see docs/agent-skill-conventions.md. */
const INVOCATION_CONTROL_KEYS = ["disable-model-invocation", "user-invocable", "allowed-tools"];

/**
 * A native `<details>`, collapsed by default: every deployment's location
 * (with a per-row Reveal button), the invocation-control frontmatter keys,
 * and where/when the skill was installed.
 */
export function SkillDetailDetails({ skill }: SkillDetailDetailsProps) {
  const addToast = useAppStore((state) => state.addToast);
  const invocationEntries = INVOCATION_CONTROL_KEYS.filter(
    (key) => skill.frontmatter_fields[key] !== undefined,
  );
  const installed = formatRelativeTime(skill.installed_at);
  const updated = skill.updated_at ? formatRelativeTime(skill.updated_at) : "unknown";

  const handleReveal = async (path: string) => {
    try {
      await openSkillPath(path, "reveal");
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't reveal in Finder",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  return (
    <details className="skill-detail-details">
      <summary>Details</summary>

      <div className="skill-detail-details-section">
        <h4>Locations</h4>
        {skill.deployments.length === 0 ? (
          <p className="skill-detail-content-empty">Known only from the lock file</p>
        ) : (
          <div className="skill-detail-deployment-list">
            {skill.deployments.map((d, i) => (
              <div key={`${d.agent}-${d.scope}-${i}`} className="skill-detail-deployment-row">
                <div className="skill-detail-deployment-row-main">
                  <span>{d.agent}</span>
                  <span className={`scope-badge ${d.scope}`}>{d.scope}</span>
                  {d.plugin && (
                    <span className="skill-detail-fact-label">
                      {d.plugin.name}
                      {d.plugin.version ? ` v${d.plugin.version}` : ""}
                    </span>
                  )}
                  <button
                    className="skill-detail-deployment-row-reveal"
                    onClick={() => handleReveal(d.path)}
                    title="Reveal in Finder"
                  >
                    <FolderOpen size={12} />
                  </button>
                </div>
                <div className="skill-detail-deployment-row-path">{d.path}</div>
                {d.is_symlink &&
                  (d.symlink_is_broken ? (
                    <div className="skill-detail-fact-warning">
                      <AlertTriangle size={11} />
                      broken link{d.symlink_error ? `: ${d.symlink_error}` : ""}
                    </div>
                  ) : (
                    <div className="skill-detail-deployment-row-symlink">
                      <Link2 size={11} />
                      <span>→ {d.symlink_target}</span>
                    </div>
                  ))}
              </div>
            ))}
          </div>
        )}
      </div>

      {invocationEntries.length > 0 && (
        <div className="skill-detail-details-section">
          <h4>Invocation</h4>
          <div className="skill-detail-frontmatter">
            {invocationEntries.map((key) => (
              <div key={key} className="skill-detail-frontmatter-row">
                <span className="skill-detail-frontmatter-key">{key}</span>
                <span className="skill-detail-frontmatter-value">
                  {skill.frontmatter_fields[key]}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="skill-detail-details-section">
        <h4>Source</h4>
        <div className="skill-detail-facts-grid">
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Source</span>
            {skill.source_url ? (
              <a
                href={skill.source_url}
                target="_blank"
                rel="noreferrer"
                className="skill-detail-link"
              >
                {skill.source || skill.source_url}
              </a>
            ) : (
              <span>{skill.source || "manual/plugin"}</span>
            )}
          </div>
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Installed</span>
            <span>{installed}</span>
          </div>
          {updated !== "unknown" && (
            <div className="skill-detail-fact">
              <span className="skill-detail-fact-label">Updated</span>
              <span>{updated}</span>
            </div>
          )}
        </div>
      </div>
    </details>
  );
}
