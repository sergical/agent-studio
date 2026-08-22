// ============================================================================
// SkillDetailFacts - Facts grid, spec violations, and frontmatter table
// ============================================================================

import { AlertTriangle } from "lucide-react";
import { formatBytes } from "../../lib/skill-stats";
import type { InstalledSkill } from "../../lib/skill-types";

interface SkillDetailFactsProps {
  skill: InstalledSkill;
}

/** Frontmatter keys that gate model auto-invocation; see docs/agent-skill-conventions.md. */
const INVOCATION_CONTROL_KEYS = new Set([
  "disable-model-invocation",
  "user-invocable",
  "allowed-tools",
  "paths",
]);

function formatModified(modifiedAt: string | undefined): string {
  if (!modifiedAt) return "unknown";
  try {
    return new Date(modifiedAt).toLocaleString();
  } catch {
    return modifiedAt;
  }
}

/**
 * Facts grid (tokens, bytes, file count, modified time, short content hash,
 * spec compliance), the spec violation list, and the raw frontmatter table
 * with invocation-control keys highlighted.
 */
export function SkillDetailFacts({ skill }: SkillDetailFactsProps) {
  const frontmatterEntries = Object.entries(skill.frontmatter_fields);

  return (
    <>
      <div className="skill-detail-section">
        <h4>Facts</h4>
        <div className="skill-detail-facts-grid">
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Tokens</span>
            <span>{skill.skill_md_tokens.toLocaleString()}</span>
          </div>
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Folder size</span>
            <span>{formatBytes(skill.folder_bytes)}</span>
          </div>
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Files</span>
            <span>{skill.file_count.toLocaleString()}</span>
          </div>
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Modified</span>
            <span>{formatModified(skill.modified_at)}</span>
          </div>
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Content hash</span>
            <span title={skill.content_hash}>{skill.content_hash.slice(0, 10)}</span>
          </div>
          <div className="skill-detail-fact">
            <span className="skill-detail-fact-label">Has spec</span>
            <span>{skill.has_spec ? "yes" : "no"}</span>
          </div>
        </div>
        {skill.folder_truncated && (
          <p className="skill-detail-fact-warning">
            <AlertTriangle size={12} />
            Folder walk was truncated (2,000-file / 64 MiB cap)
          </p>
        )}
      </div>

      {skill.spec_violations.length > 0 && (
        <div className="skill-detail-section skill-detail-spec-violations">
          <h4>Spec violations</h4>
          <ul>
            {skill.spec_violations.map((violation, i) => (
              <li key={i}>{violation}</li>
            ))}
          </ul>
        </div>
      )}

      {frontmatterEntries.length > 0 && (
        <div className="skill-detail-section">
          <h4>Frontmatter</h4>
          <div className="skill-detail-frontmatter">
            {frontmatterEntries.map(([key, value]) => (
              <div
                key={key}
                className={`skill-detail-frontmatter-row ${
                  INVOCATION_CONTROL_KEYS.has(key) ? "invocation-control" : ""
                }`}
              >
                <span className="skill-detail-frontmatter-key">{key}</span>
                <span className="skill-detail-frontmatter-value">{value}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </>
  );
}
