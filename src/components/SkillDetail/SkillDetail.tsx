// ============================================================================
// SkillDetail - Detail drawer for an installed skill
// ============================================================================

import { AlertTriangle, FileCheck2, X } from "lucide-react";
import { SkillContent } from "../SkillStore/SkillContent";
import { SOURCE_KIND_LABELS } from "../../lib/skill-types";
import type { InstalledSkill, SkillInvocationStats, SkillWithStatus } from "../../lib/skill-types";
import { SkillDetailActions } from "./SkillDetailActions";
import { SkillDetailDeployments } from "./SkillDetailDeployments";
import { SkillDetailFacts } from "./SkillDetailFacts";

interface SkillDetailProps {
  skill: InstalledSkill;
  invocationStats: SkillInvocationStats | undefined;
  onClose: () => void;
  onRemoveComplete: () => void;
}

/** Wraps an `InstalledSkill` in the shape `SkillContent` expects. */
function toSkillWithStatus(skill: InstalledSkill): SkillWithStatus {
  return {
    id: skill.name,
    name: skill.name,
    installs: 0,
    is_installed: true,
    installed_info: skill,
    description: skill.description,
  };
}

/**
 * Detail drawer for an installed skill: header, facts, deployments,
 * provenance, invocations, frontmatter, spec violations, the rendered
 * SKILL.md, and the actions row (reveal/open/copy, remove/update, and the
 * not-yet-wired enable/disable toggle). Keeps the Discover tab's
 * `SkillStore/SkillDetailPanel` (for search results) working unmodified.
 */
export function SkillDetail({
  skill,
  invocationStats,
  onClose,
  onRemoveComplete,
}: SkillDetailProps) {
  return (
    <div className="skill-detail-panel">
      <div className="skill-detail-header">
        <div className="skill-detail-title">
          <h3>{skill.name}</h3>
          <span className={`skill-detail-badge source-kind ${skill.source_kind}`}>
            {SOURCE_KIND_LABELS[skill.source_kind]}
          </span>
          {skill.has_spec && (
            <span className="skill-detail-badge spec" title="Ships behavior specs/evals">
              <FileCheck2 size={12} />
              spec
            </span>
          )}
          {skill.spec_violations.length > 0 && (
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

      {skill.description && (
        <div className="skill-detail-section skill-detail-description">
          <p>{skill.description}</p>
        </div>
      )}

      <SkillDetailFacts skill={skill} />
      <div className="skill-detail-divider" />
      <SkillDetailDeployments skill={skill} stats={invocationStats} />
      <div className="skill-detail-divider" />
      <SkillContent skill={toSkillWithStatus(skill)} resolvedTopSource={null} />
      <div className="skill-detail-divider" />
      <SkillDetailActions skill={skill} onRemoveComplete={onRemoveComplete} />
    </div>
  );
}
