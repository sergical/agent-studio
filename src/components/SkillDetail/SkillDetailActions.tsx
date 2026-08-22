// ============================================================================
// SkillDetailActions - Reveal/open/copy path, remove/update, and the
// (not-yet-wired) enable/disable toggle
// ============================================================================

import { useState } from "react";
import { Copy, FolderOpen, Power, TerminalSquare } from "lucide-react";
import { InstallControls } from "../SkillStore/InstallControls";
import { openSkillPath } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import type { InstalledSkill, SkillWithStatus } from "../../lib/skill-types";

interface SkillDetailActionsProps {
  skill: InstalledSkill;
  onRemoveComplete: () => void;
}

/** Wraps an `InstalledSkill` in the shape `InstallControls` expects. */
function toSkillWithStatus(skill: InstalledSkill): SkillWithStatus {
  return {
    id: skill.name,
    name: skill.name,
    installs: 0,
    is_installed: true,
    installed_info: skill,
  };
}

/**
 * Actions row for the detail panel: reveal the skill folder in Finder, open
 * it in the default editor, copy its path, remove/update it (skills.sh
 * skills only, via the shared `InstallControls`), and an enable/disable
 * toggle that's disabled until parking lands in a later step.
 */
export function SkillDetailActions({ skill, onRemoveComplete }: SkillDetailActionsProps) {
  const [copied, setCopied] = useState(false);
  const addToast = useAppStore((state) => state.addToast);
  const path = skill.deployments[0]?.path ?? skill.skill_path;

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
    <div className="skill-detail-section skill-detail-actions-row">
      <button className="skill-action-button" onClick={handleReveal} disabled={!path}>
        <FolderOpen size={14} />
        Reveal in Finder
      </button>
      <button className="skill-action-button" onClick={handleOpenEditor} disabled={!path}>
        <TerminalSquare size={14} />
        Open in editor
      </button>
      <button className="skill-action-button" onClick={handleCopyPath} disabled={!path}>
        <Copy size={14} />
        {copied ? "Copied!" : "Copy path"}
      </button>
      <button className="skill-action-button" disabled title="Coming next">
        <Power size={14} />
        {skill.source_kind === "manual" ? "Disable" : "Enable / Disable"}
      </button>

      {skill.source_kind === "skills-sh" && (
        <InstallControls
          skill={toSkillWithStatus(skill)}
          resolvedTopSource={null}
          onInstallStart={() => {}}
          onInstallComplete={() => {}}
          onRemoveComplete={onRemoveComplete}
        />
      )}
    </div>
  );
}
