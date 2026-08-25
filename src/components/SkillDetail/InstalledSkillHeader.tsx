// ============================================================================
// InstalledSkillHeader - Name, description, badges, and the icon-button row
// (reveal, open in editor, copy path, enable/disable)
// ============================================================================

import { useState } from "react";
import { AlertTriangle, Copy, FolderOpen, Power, TerminalSquare, X } from "lucide-react";
import { openSkillPath } from "../../lib/skill-api";
import { pluginLabelForSkill } from "../../lib/skill-plugin-partition";
import { SOURCE_KIND_LABELS } from "../../lib/skill-types";
import type { InstalledSkill } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";

interface InstalledSkillHeaderProps {
  skill: InstalledSkill;
  onClose: () => void;
}

/** "skills.sh" / "plugin: openai-templates" / ".agents" / "manual". */
function sourceKindBadgeLabel(skill: InstalledSkill): string {
  if (skill.source_kind === "plugin") {
    const pluginName = pluginLabelForSkill(skill);
    return pluginName ? `plugin: ${pluginName}` : SOURCE_KIND_LABELS.plugin;
  }
  return SOURCE_KIND_LABELS[skill.source_kind];
}

/**
 * Header: name, description, a badge row (source kind, agent chips, and a
 * spec-violation warning when any), and an icon-button row for the file
 * actions every installed skill supports. Enable/disable is wired up in a
 * later step; it stays disabled here.
 */
export function InstalledSkillHeader({ skill, onClose }: InstalledSkillHeaderProps) {
  const [copied, setCopied] = useState(false);
  const addToast = useAppStore((state) => state.addToast);
  const path = skill.deployments[0]?.path ?? skill.skill_path;
  const agents = [...new Set(skill.deployments.map((d) => d.agent))];

  const handleReveal = async () => {
    if (!path) return;
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

  const handleOpenEditor = async () => {
    if (!path) return;
    try {
      await openSkillPath(path, "editor");
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't open in editor",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  const handleCopyPath = async () => {
    if (!path) return;
    await navigator.clipboard.writeText(path);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="skill-detail-header">
      <div className="skill-detail-header-body">
        <div className="skill-detail-title">
          <h2>{skill.name}</h2>
          <button className="skill-detail-close" onClick={onClose}>
            <X size={18} />
          </button>
        </div>

        {skill.description && <p className="skill-detail-description">{skill.description}</p>}

        <div className="skill-detail-badge-row">
          <span className={`skill-detail-badge source-kind ${skill.source_kind}`}>
            {sourceKindBadgeLabel(skill)}
          </span>
          {agents.map((agent) => (
            <span key={agent} className="skill-detail-agent-chip">
              {agent}
            </span>
          ))}
          {skill.spec_violations.length > 0 && (
            <span
              className="skill-detail-badge spec-violation"
              title={skill.spec_violations.join("; ")}
            >
              <AlertTriangle size={12} />
              {skill.spec_violations.length} spec issue
              {skill.spec_violations.length !== 1 ? "s" : ""}
            </span>
          )}
        </div>

        <div className="skill-detail-icon-row">
          <button className="skill-detail-icon-button" onClick={handleReveal} disabled={!path}>
            <FolderOpen size={14} />
            Reveal in Finder
          </button>
          <button className="skill-detail-icon-button" onClick={handleOpenEditor} disabled={!path}>
            <TerminalSquare size={14} />
            Open in editor
          </button>
          <button className="skill-detail-icon-button" onClick={handleCopyPath} disabled={!path}>
            <Copy size={14} />
            {copied ? "Copied!" : "Copy path"}
          </button>
          <button className="skill-detail-icon-button" disabled title="Coming next">
            <Power size={14} />
            Enable / Disable
          </button>
        </div>
      </div>
    </div>
  );
}
