// ============================================================================
// SkillDetailHeader - Name, badges, provenance, and deployments for a skill
// ============================================================================

import {
  X,
  Download,
  ExternalLink,
  Check,
  Clock,
  GitBranch,
  FileCheck2,
  AlertTriangle,
  Link2,
} from "lucide-react";
import type { SkillWithStatus } from "../../lib/skill-types";
import { SOURCE_KIND_LABELS } from "../../lib/skill-types";

interface SkillDetailHeaderProps {
  skill: SkillWithStatus;
  resolvedTopSource: string | null;
  onClose: () => void;
}

function formatDate(dateStr: string): string {
  try {
    return new Date(dateStr).toLocaleDateString();
  } catch {
    return dateStr;
  }
}

function safeHostname(url: string): string | null {
  try {
    return new URL(url).hostname;
  } catch {
    return null;
  }
}

export function SkillDetailHeader({ skill, resolvedTopSource, onClose }: SkillDetailHeaderProps) {
  return (
    <>
      <div className="skill-detail-header">
        <div className="skill-detail-title">
          <h3>{skill.name}</h3>
          {skill.is_installed && (
            <span className="skill-detail-badge installed">
              <Check size={12} />
              Installed
            </span>
          )}
          {skill.installed_info?.has_update && (
            <span className="skill-detail-badge update">Update available</span>
          )}
          {skill.installed_info && (
            <span className={`skill-detail-badge source-kind ${skill.installed_info.source_kind}`}>
              {SOURCE_KIND_LABELS[skill.installed_info.source_kind]}
            </span>
          )}
          {skill.installed_info?.has_spec && (
            <span className="skill-detail-badge spec" title="Ships behavior specs/evals">
              <FileCheck2 size={12} />
              spec
            </span>
          )}
          {skill.installed_info && skill.installed_info.spec_violations.length > 0 && (
            <span
              className="skill-detail-badge spec-violation"
              title="SKILL.md doesn't fully match the agentskills.io spec"
            >
              <AlertTriangle size={12} />
              spec issues
            </span>
          )}
        </div>
        <button className="skill-detail-close" onClick={onClose}>
          <X size={18} />
        </button>
      </div>

      {skill.installed_info && skill.installed_info.deployments.length > 0 && (
        <div className="skill-detail-deployments">
          {skill.installed_info.deployments.map((deployment, i) => (
            <span
              key={`${deployment.agent}-${deployment.scope}-${i}`}
              className="skill-detail-deployment"
            >
              {deployment.is_symlink && <Link2 size={11} />}
              {deployment.plugin
                ? `${deployment.agent} · via ${deployment.plugin.name}`
                : `${deployment.agent} · ${deployment.scope}`}
            </span>
          ))}
        </div>
      )}

      {skill.installed_info && skill.installed_info.spec_violations.length > 0 && (
        <div className="skill-detail-section skill-detail-spec-violations">
          <h4>Spec violations</h4>
          <ul>
            {skill.installed_info.spec_violations.map((violation, i) => (
              <li key={i}>{violation}</li>
            ))}
          </ul>
        </div>
      )}

      <div className="skill-detail-meta">
        {/* Source display - handle both GitHub (owner/repo) and well-known (URL) sources */}
        {(() => {
          // For installed well-known skills, show the domain from source_url
          const wellKnownUrl =
            skill.installed_info?.source_url && skill.installed_info.source_type === "well-known"
              ? skill.installed_info.source_url
              : null;
          const wellKnownHostname = wellKnownUrl ? safeHostname(wellKnownUrl) : null;

          if (wellKnownUrl && wellKnownHostname) {
            return (
              <div className="skill-detail-source">
                <GitBranch size={14} />
                <a
                  href={wellKnownUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="skill-detail-link"
                >
                  {wellKnownHostname}
                  <ExternalLink size={12} />
                </a>
              </div>
            );
          }

          // GitHub owner/repo format
          const source = skill.top_source || resolvedTopSource;
          if (source) {
            return (
              <div className="skill-detail-source">
                <GitBranch size={14} />
                <a
                  href={`https://github.com/${source}`}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="skill-detail-link"
                >
                  {source}
                  <ExternalLink size={12} />
                </a>
              </div>
            );
          }

          return null;
        })()}
        <div className="skill-detail-stats">
          <div className="skill-detail-stat">
            <Download size={14} />
            <span>{skill.installs.toLocaleString()}</span>
          </div>
          {skill.installed_info && (
            <div className="skill-detail-stat">
              <Clock size={14} />
              <span>{formatDate(skill.installed_info.installed_at)}</span>
            </div>
          )}
        </div>
      </div>

      {skill.tags && skill.tags.length > 0 && (
        <div className="skill-detail-tags">
          {skill.tags.map((tag) => (
            <span key={tag} className="skill-detail-tag">
              {tag}
            </span>
          ))}
        </div>
      )}
    </>
  );
}
