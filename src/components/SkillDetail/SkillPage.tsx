// ============================================================================
// SkillPage - Full-page view of an installed skill: header, stats line,
// rendered/editable SKILL.md body, actions, and the collapsed details
// section on the left; the `SkillAssistantPanel` on the right
// ============================================================================

import { useEffect, useState } from "react";
import { ArrowLeft } from "lucide-react";
import ReactMarkdown from "react-markdown";
import { useSkillSnapshot } from "../../hooks/useSkillSnapshot";
import { forkSkill, readInstalledSkillMd, writeInstalledSkillMd } from "../../lib/skill-api";
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
 * section on the left, and the `SkillAssistantPanel` on the right.
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
  const openSkill = useAppStore((state) => state.openSkill);
  const { snapshot } = useSkillSnapshot();

  const [isEditing, setIsEditing] = useState(false);
  const [isEditorDirty, setIsEditorDirty] = useState(false);
  const [isHistoryOpen, setIsHistoryOpen] = useState(false);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      const target = event.target;
      // Escape typed into an input, textarea, contenteditable region, or an
      // open dialog belongs to that widget - the local handler (if any) deals
      // with it, not page navigation.
      if (
        target instanceof HTMLElement &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable ||
          target.closest("dialog") !== null)
      ) {
        return;
      }
      if (isEditing && isEditorDirty && !window.confirm("Discard unsaved changes?")) return;
      onBack();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onBack, isEditing, isEditorDirty]);

  useEffect(() => {
    setIsHistoryOpen(false);
  }, [skill?.name]);

  // The deployment this page edits: only the one the caller clicked, when
  // given - a stale `deploymentPath` (the copy was removed by a rescan) must
  // not silently fall back to a different copy of the skill. With no
  // `deploymentPath` at all, fall back to the skill's first own deployment,
  // then its first deployment (a plugin-only skill has no own deployment).
  const requestedDeployment =
    skill && deploymentPath ? skill.deployments.find((d) => d.path === deploymentPath) : undefined;
  const deploymentUnresolved = Boolean(skill && deploymentPath && !requestedDeployment);
  const deployment =
    skill &&
    (deploymentPath ? requestedDeployment : ownDeployments(skill)[0] || skill.deployments[0]);
  const skillMdPath = deployment ? skillMdPathForDeployment(deployment) : undefined;
  const isPluginManaged = deployment?.plugin !== undefined;

  const [rawContent, setRawContent] = useState<string | null>(null);
  const [isLoadingContent, setIsLoadingContent] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);

  useEffect(() => {
    // A path change (including to/from `undefined`) can never carry over a
    // stale draft or edit mode from a different copy of the skill.
    setRawContent(null);
    setIsEditing(false);
    setIsEditorDirty(false);
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

  // A dotagents/skills.sh-managed skill would have its edits overwritten by
  // the next sync/update - saving forks it first so the edit sticks.
  const needsForkToSave = skill?.source_kind === "dotagents" || skill?.source_kind === "skills-sh";

  const handleSave = async (content: string) => {
    // Ignore a duplicate save request (e.g. Cmd+S fired while the Save
    // button's own click is already in flight).
    if (!skill || !skillMdPath || isSaving) return;
    setIsSaving(true);
    try {
      if (needsForkToSave) {
        try {
          // `skillMdPath` is only set once `deployment` resolves (see
          // above), so it's non-null here.
          await forkSkill(skill.name, deployment!.path);
        } catch (err) {
          addToast({
            type: "error",
            title: "Couldn't fork before saving",
            message: err instanceof Error ? err.message : "Unknown error",
          });
          return;
        }
      }
      await writeInstalledSkillMd(skillMdPath, content);
      setRawContent(content);
      setIsEditing(false);
      setIsEditorDirty(false);
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
        <InstalledSkillHeader
          skill={skill}
          lastTest={snapshot?.last_test_by_skill[skill.name]}
          onOpenHistory={() => setIsHistoryOpen(true)}
        />
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

            {deploymentUnresolved ? (
              <div className="skill-detail-content-fallback">
                <p>The copy you opened is no longer installed.</p>
                {ownDeployments(skill).length > 0 && (
                  <div className="skill-detail-actions-row">
                    {ownDeployments(skill).map((d) => (
                      <button
                        key={d.path}
                        className="skill-action-button"
                        onClick={() => openSkill(skill.name, d.path)}
                      >
                        {d.agent} · {d.scope}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            ) : isPluginManaged ? (
              <p className="skill-detail-content-fallback">
                Managed by the {pluginLabelForSkill(skill) ?? "harness"} plugin.
              </p>
            ) : isEditing && rawContent !== null ? (
              <SkillMarkdownEditor
                key={skillMdPath}
                initialContent={rawContent}
                isSaving={isSaving}
                saveLabel={needsForkToSave ? "Fork and save" : "Save"}
                onSave={handleSave}
                onCancel={() => {
                  setIsEditing(false);
                  setIsEditorDirty(false);
                }}
                onDirtyChange={setIsEditorDirty}
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
          <SkillAssistantPanel
            skill={skill}
            rawContent={rawContent}
            skillMdPath={skillMdPath}
            isPluginManaged={isPluginManaged}
            onApplied={setRawContent}
            onDiskChanged={() => {
              if (!skillMdPath) return;
              readInstalledSkillMd(skillMdPath)
                .then(setRawContent)
                .catch((err) => {
                  setLoadError(err instanceof Error ? err.message : "Unknown error");
                });
            }}
            showHistory={isHistoryOpen}
            onCloseHistory={() => setIsHistoryOpen(false)}
          />
        </div>
      </div>
    </div>
  );
}
