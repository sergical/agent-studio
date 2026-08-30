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
import type { SkillWithStatus } from "@skill-studio/lib";
import { SOURCE_KIND_LABELS } from "@skill-studio/lib";

interface SkillDetailHeaderProps {
  skill: SkillWithStatus;
  resolvedTopSource: string | null;
  onClose: () => void;
}

const BADGE_CLASS = "inline-flex items-center gap-1 rounded-xs px-2 py-1 text-caption font-medium";

/** skills.sh: accent; dotagents: info; plugin: warning; in-repo/manual: tertiary; fork: success. */
function sourceKindBadgeClass(kind: string): string {
  switch (kind) {
    case "skills-sh":
      return "bg-accent-softer text-accent";
    case "dotagents":
      return "bg-info-soft text-info";
    case "plugin":
      return "bg-warning-soft text-warning";
    case "fork":
      return "bg-success-soft text-success";
    default:
      return "bg-bg-tertiary text-text-tertiary";
  }
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
      <div className="flex items-start justify-between gap-3 border-b border-border p-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <h2 className="m-0 text-pretty text-balance text-heading font-semibold break-words text-text-primary">
            {skill.name}
          </h2>
          {skill.is_installed && (
            <span className={`${BADGE_CLASS} bg-success-soft text-success`}>
              <Check size={12} />
              Installed
            </span>
          )}
          {skill.installed_info?.has_update && (
            <span className={`${BADGE_CLASS} bg-warning-soft text-warning`}>Update available</span>
          )}
          {skill.installed_info && (
            <span
              className={`${BADGE_CLASS} ${sourceKindBadgeClass(skill.installed_info.source_kind)}`}
            >
              {SOURCE_KIND_LABELS[skill.installed_info.source_kind]}
            </span>
          )}
          {skill.installed_info?.has_spec && (
            <span
              className={`${BADGE_CLASS} bg-success-soft text-success`}
              title="Ships behavior specs/evals"
            >
              <FileCheck2 size={12} />
              spec
            </span>
          )}
          {skill.installed_info && skill.installed_info.spec_violations.length > 0 && (
            <span
              className={`${BADGE_CLASS} bg-warning-soft text-warning`}
              title="SKILL.md doesn't fully match the agentskills.io spec"
            >
              <AlertTriangle size={12} />
              spec issues
            </span>
          )}
        </div>
        <button
          className="flex size-8 shrink-0 items-center justify-center rounded-sm border-0 bg-transparent text-text-tertiary transition-colors hover:bg-bg-hover hover:text-text-primary"
          onClick={onClose}
          aria-label="Close"
        >
          <X size={18} />
        </button>
      </div>

      {skill.installed_info && skill.installed_info.deployments.length > 0 && (
        <div className="flex flex-wrap gap-1.5 px-5 pb-3">
          {skill.installed_info.deployments.map((deployment, i) => (
            <span
              key={`${deployment.agent}-${deployment.scope}-${i}`}
              className="inline-flex items-center gap-1 rounded-sm bg-bg-tertiary px-2 py-[3px] text-caption text-text-tertiary"
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
        <div className="p-5 pt-0">
          <h4 className="m-0 mb-3 text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
            Spec violations
          </h4>
          <ul className="m-0 flex flex-col gap-1.5 pl-4.5">
            {skill.installed_info.spec_violations.map((violation) => (
              <li key={violation} className="text-small text-warning">
                {violation}
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="mb-3 flex flex-wrap items-center gap-2.5 border-b border-border px-5 py-3">
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
              <div className="flex items-center gap-1.5 font-mono text-small text-text-tertiary">
                <GitBranch size={14} />
                <a
                  href={wellKnownUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex items-center gap-1 text-text-tertiary no-underline transition-colors hover:text-accent hover:underline"
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
              <div className="flex items-center gap-1.5 font-mono text-small text-text-tertiary">
                <GitBranch size={14} />
                <a
                  href={`https://github.com/${source}`}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex items-center gap-1 text-text-tertiary no-underline transition-colors hover:text-accent hover:underline"
                >
                  {source}
                  <ExternalLink size={12} />
                </a>
              </div>
            );
          }

          return null;
        })()}
        <div className="ml-auto flex items-center gap-2">
          <div className="flex items-center gap-1 rounded-sm bg-bg-tertiary px-2 py-1 text-small text-text-tertiary">
            <Download size={14} />
            <span>{skill.installs.toLocaleString()}</span>
          </div>
          {skill.installed_info && (
            <div className="flex items-center gap-1 rounded-sm bg-bg-tertiary px-2 py-1 text-small text-text-tertiary">
              <Clock size={14} />
              <span>{formatDate(skill.installed_info.installed_at)}</span>
            </div>
          )}
        </div>
      </div>

      {skill.tags && skill.tags.length > 0 && (
        <div className="flex flex-wrap gap-1.5 px-5 pb-4">
          {skill.tags.map((tag) => (
            <span
              key={tag}
              className="rounded-xs bg-bg-tertiary px-2.5 py-1 text-caption text-text-secondary"
            >
              {tag}
            </span>
          ))}
        </div>
      )}
    </>
  );
}
