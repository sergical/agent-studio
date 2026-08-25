// ============================================================================
// SkillDetail - Detail drawer for an installed skill: header, stats line,
// rendered/editable SKILL.md body, actions, and the collapsed details section
// ============================================================================

import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import { readInstalledSkillMd, writeInstalledSkillMd } from "../../lib/skill-api";
import {
  ownDeployments,
  pluginLabelForSkill,
  skillMdPathForDeployment,
} from "../../lib/skill-plugin-partition";
import { formatBytes, formatRelativeTime, formatTokens } from "../../lib/skill-stats";
import type { InstalledSkill, SkillInvocationStats } from "../../lib/skill-types";
import { useAppStore } from "../../store/appStore";
import { InstalledSkillHeader } from "./InstalledSkillHeader";
import { SkillDetailActions } from "./SkillDetailActions";
import { SkillDetailDetails } from "./SkillDetailDetails";
import { SkillMarkdownEditor } from "./SkillMarkdownEditor";

interface SkillDetailProps {
  skill: InstalledSkill;
  /** The specific deployment the caller clicked, when known - see `SelectedSkill`. */
  deploymentPath?: string;
  invocationStats: SkillInvocationStats | undefined;
  onClose: () => void;
  onRemoveComplete: () => void;
}

/** Strips a leading `---\n...\n---\n` YAML frontmatter block, if present. */
function stripFrontmatter(content: string): string {
  return content.replace(/^---\n[\s\S]*?\n---\n/, "");
}

/**
 * Detail drawer for an installed skill: `InstalledSkillHeader`, a stats line
 * (tokens, size, invocations, relative install/modified time - unknown
 * segments omitted), the rendered SKILL.md body with an inline raw-text
 * editor, the legacy actions row (remove/update for skills.sh skills), and
 * the collapsed `SkillDetailDetails` section.
 */
export function SkillDetail({
  skill,
  deploymentPath,
  invocationStats,
  onClose,
  onRemoveComplete,
}: SkillDetailProps) {
  const addToast = useAppStore((state) => state.addToast);

  // The deployment this drawer edits: the one the caller clicked, falling
  // back to the skill's first own deployment, then its first deployment at
  // all (a plugin-only skill has no own deployment to fall back to).
  const deployment =
    (deploymentPath && skill.deployments.find((d) => d.path === deploymentPath)) ||
    ownDeployments(skill)[0] ||
    skill.deployments[0];
  const skillMdPath = deployment ? skillMdPathForDeployment(deployment) : undefined;
  const isPluginManaged = deployment?.plugin !== undefined;

  const [rawContent, setRawContent] = useState<string | null>(null);
  const [isLoadingContent, setIsLoadingContent] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [isEditing, setIsEditing] = useState(false);
  const [isSaving, setIsSaving] = useState(false);

  useEffect(() => {
    if (!skillMdPath) return;
    let cancelled = false;
    setIsLoadingContent(true);
    setLoadError(null);
    readInstalledSkillMd(skillMdPath)
      .then((content) => {
        if (!cancelled) setRawContent(content);
      })
      .catch((err) => {
        if (!cancelled) setLoadError(err instanceof Error ? err.message : "Unknown error");
      })
      .finally(() => {
        if (!cancelled) setIsLoadingContent(false);
      });
    return () => {
      cancelled = true;
    };
  }, [skillMdPath]);

  const handleSave = async (content: string) => {
    // Ignore a duplicate save request (e.g. Cmd+S fired while the Save
    // button's own click is already in flight).
    if (!skillMdPath || isSaving) return;
    setIsSaving(true);
    try {
      await writeInstalledSkillMd(skillMdPath, content);
      setRawContent(content);
      setIsEditing(false);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't save SKILL.md",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    } finally {
      setIsSaving(false);
    }
  };

  const modifiedRelative =
    skill.modified_at !== undefined ? formatRelativeTime(skill.modified_at) : "unknown";
  const statsSegments = [
    `${formatTokens(skill.skill_md_tokens)} tokens`,
    formatBytes(skill.folder_bytes),
    `${invocationStats?.last_30_days ?? 0} invocations (30d)`,
    modifiedRelative !== "unknown" && `modified ${modifiedRelative}`,
  ].filter((segment): segment is string => Boolean(segment));

  return (
    <div className="skill-detail-panel">
      <InstalledSkillHeader skill={skill} onClose={onClose} />

      <div className="skill-detail-stats-line">{statsSegments.join(" · ")}</div>

      <div className="skill-detail-section skill-detail-markdown-section">
        <div className="skill-detail-content-header">
          <span>SKILL.md</span>
          {!isEditing && !isPluginManaged && rawContent !== null && (
            <button className="skill-action-button" onClick={() => setIsEditing(true)}>
              Edit
            </button>
          )}
        </div>

        {isPluginManaged ? (
          <p className="skill-detail-content-fallback">
            Managed by the {pluginLabelForSkill(skill) ?? "harness"} plugin.
          </p>
        ) : isEditing && rawContent !== null ? (
          <SkillMarkdownEditor
            initialContent={rawContent}
            isSaving={isSaving}
            onSave={handleSave}
            onCancel={() => setIsEditing(false)}
          />
        ) : isLoadingContent ? (
          <div className="skill-detail-content-loading">
            <span className="spinner" />
            Loading content...
          </div>
        ) : loadError ? (
          <p className="skill-detail-content-fallback">{loadError}</p>
        ) : rawContent !== null ? (
          <div className="skill-markdown">
            <ReactMarkdown>{stripFrontmatter(rawContent)}</ReactMarkdown>
          </div>
        ) : (
          <p className="skill-detail-content-empty">No content available</p>
        )}
      </div>

      <div className="skill-detail-divider" />
      <SkillDetailActions skill={skill} onRemoveComplete={onRemoveComplete} />
      <div className="skill-detail-divider" />
      <SkillDetailDetails skill={skill} />
    </div>
  );
}
