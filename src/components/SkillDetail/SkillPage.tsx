// ============================================================================
// SkillPage - Full-page view of an installed skill: header, stats line,
// rendered/editable SKILL.md body, actions, and the collapsed details
// section on the left; the assistant placeholder on the right
// ============================================================================

import { useEffect, useState } from "react";
import { ArrowLeft } from "lucide-react";
import ReactMarkdown from "react-markdown";
import { readInstalledSkillMd, writeInstalledSkillMd } from "../../lib/skill-api";
import {
  ownDeployments,
  pluginLabelForSkill,
  skillMdPathForDeployment,
} from "../../lib/skill-plugin-partition";
import { formatBytes, formatRelativeTime, formatTokens } from "../../lib/skill-stats";
import type { InstalledSkill, SkillInvocationStats } from "../../lib/skill-types";
import type { ActiveView } from "../../store/appStore";
import { useAppStore } from "../../store/appStore";
import { InstalledSkillHeader } from "./InstalledSkillHeader";
import { SkillAssistantPanel } from "./SkillAssistantPanel";
import { SkillDetailActions } from "./SkillDetailActions";
import { SkillDetailDetails } from "./SkillDetailDetails";
import { SkillMarkdownEditor } from "./SkillMarkdownEditor";

interface SkillPageProps {
  /** `null` when the skill named by the route was removed since the page opened. */
  skill: InstalledSkill | null;
  /** The specific deployment the caller clicked, when known - see `ActiveView`'s "skill" kind. */
  deploymentPath?: string;
  invocationStats: SkillInvocationStats | undefined;
  onBack: () => void;
  onRemoveComplete: () => void;
  /** The view the page was opened from, for the back button's label. */
  from: ActiveView;
}

/** Strips a leading `---\n...\n---\n` YAML frontmatter block, if present. */
function stripFrontmatter(content: string): string {
  return content.replace(/^---\n[\s\S]*?\n---\n/, "");
}

/** The back button's label: the name of the view the page was opened from. */
function backLabel(from: ActiveView): string {
  switch (from.kind) {
    case "dashboard":
      return "Dashboard";
    case "global":
      return "Global";
    case "project":
      return from.path.split("/").filter(Boolean).pop() ?? from.path;
    case "plugins":
      return "Plugins";
    case "coverage":
      return "Coverage";
    case "issues":
      return "Issues";
    case "activity":
      return "Activity";
    case "discover":
      return "Discover";
    default:
      // `ActiveView`'s "skill" kind never nests as its own `from` (see `openSkill`).
      return "Back";
  }
}

/**
 * Full-page view of an installed skill: a back button plus `InstalledSkillHeader`
 * in the page header, then a two-column body - the stats line, the rendered
 * SKILL.md body with an inline raw-text editor, the legacy actions row
 * (remove/update for skills.sh skills), and the collapsed `SkillDetailDetails`
 * section on the left, and the `SkillAssistantPanel` placeholder on the right.
 */
export function SkillPage({
  skill,
  deploymentPath,
  invocationStats,
  onBack,
  onRemoveComplete,
  from,
}: SkillPageProps) {
  const addToast = useAppStore((state) => state.addToast);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onBack();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onBack]);

  // The deployment this page edits: the one the caller clicked, falling back
  // to the skill's first own deployment, then its first deployment at all (a
  // plugin-only skill has no own deployment to fall back to).
  const deployment =
    skill &&
    ((deploymentPath && skill.deployments.find((d) => d.path === deploymentPath)) ||
      ownDeployments(skill)[0] ||
      skill.deployments[0]);
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

  if (!skill) {
    return (
      <div className="skill-page">
        <div className="skill-page-header-row">
          <button className="skill-page-back" onClick={onBack} aria-label="Back">
            <ArrowLeft size={28} />
            <span>{backLabel(from)}</span>
          </button>
        </div>
        <p className="skill-page-not-found">This skill is no longer installed.</p>
      </div>
    );
  }

  const modifiedRelative =
    skill.modified_at !== undefined ? formatRelativeTime(skill.modified_at) : "unknown";
  const statsSegments = [
    `${formatTokens(skill.skill_md_tokens)} tokens`,
    formatBytes(skill.folder_bytes),
    `${invocationStats?.last_30_days ?? 0} invocations (30d)`,
    modifiedRelative !== "unknown" && `modified ${modifiedRelative}`,
  ].filter((segment): segment is string => Boolean(segment));

  return (
    <div className="skill-page">
      <div className="skill-page-header-row">
        <button className="skill-page-back" onClick={onBack} aria-label="Back">
          <ArrowLeft size={28} />
          <span>{backLabel(from)}</span>
        </button>
        <InstalledSkillHeader skill={skill} />
      </div>

      <div className="skill-page-grid">
        <div className="skill-page-column-main">
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

        <div className="skill-page-column-side">
          <SkillAssistantPanel skill={skill} />
        </div>
      </div>
    </div>
  );
}
