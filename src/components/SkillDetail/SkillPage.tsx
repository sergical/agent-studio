// ============================================================================
// SkillPage - Full-page view of an installed skill: header (name, one
// primary action, overflow menu, chips, metadata line), the "where it lives"
// locations card, the SKILL.md card, and the assistant panel on the right.
// ============================================================================

import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { useSkillSnapshot } from "../../hooks/useSkillSnapshot";
import { forkSkill, readInstalledSkillMd, writeInstalledSkillMd } from "../../lib/skill-api";
import { ownDeployments, skillMdPathForDeployment } from "../../lib/skill-plugin-partition";
import type { InstalledSkill, SkillInvocationStats } from "../../lib/skill-types";
import type { ActiveView } from "../../store/appStore";
import { useAppStore } from "../../store/appStore";
import { InstalledSkillHeader } from "./InstalledSkillHeader";
import { SkillAssistantPanel } from "./SkillAssistantPanel";
import { SkillLocationsCard } from "./SkillLocationsCard";
import { SkillMarkdownCard } from "./SkillMarkdownCard";

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

/**
 * Full-page view of an installed skill: `InstalledSkillHeader` (which owns
 * the back button, name, primary action, overflow menu, chips, and metadata
 * line), then a two-column body - `SkillLocationsCard` and
 * `SkillMarkdownCard` on the left, `SkillAssistantPanel` on the right.
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
  const isPluginManaged = Boolean(deployment?.plugin);

  const [rawContent, setRawContent] = useState<string | null>(null);
  const [isLoadingContent, setIsLoadingContent] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);

  /** The SKILL.md path the page currently shows - every async read or apply checks against it so a late result for a previous skill is dropped instead of landing on this one. */
  const currentSkillMdPathRef = useRef<string | undefined>(undefined);

  /** Reads SKILL.md at `path`; `showSkeleton` swaps the card for the loading skeleton (initial load and retry), a silent reload keeps the current content up until the read lands. */
  const loadContent = useCallback((path: string, showSkeleton: boolean) => {
    setLoadError(null);
    if (showSkeleton) setIsLoadingContent(true);
    const isCurrent = () => currentSkillMdPathRef.current === path;
    readInstalledSkillMd(path)
      .then((content) => {
        if (isCurrent()) setRawContent(content);
      })
      .catch((err) => {
        if (isCurrent()) setLoadError(err instanceof Error ? err.message : "Unknown error");
      })
      .finally(() => {
        if (isCurrent()) setIsLoadingContent(false);
      });
  }, []);

  useEffect(() => {
    // A path change (including to/from `undefined`) can never carry over a
    // stale draft or edit mode from a different copy of the skill.
    currentSkillMdPathRef.current = skillMdPath;
    setRawContent(null);
    setIsEditing(false);
    setIsEditorDirty(false);
    setIsLoadingContent(false);
    setLoadError(null);
    if (skillMdPath) loadContent(skillMdPath, true);
  }, [skillMdPath, loadContent]);

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

  const handleRetryLoad = () => {
    if (skillMdPath) loadContent(skillMdPath, true);
  };

  /** Applies assistant-written content only while the page still shows the path it was written to. */
  const handleApplied = (content: string) => {
    if (skillMdPath !== undefined && currentSkillMdPathRef.current === skillMdPath) {
      setRawContent(content);
    }
  };

  if (!skill) {
    return (
      <div className="skill-page">
        <div className="skill-page-header-row-1">
          <button className="skill-page-back" onClick={onBack} aria-label="Back">
            <ArrowLeft size={16} />
            <span>{from.kind === "home" ? "Home" : "Back"}</span>
          </button>
        </div>
        <p className="skill-page-not-found">This skill is no longer installed.</p>
      </div>
    );
  }

  return (
    <div className="skill-page">
      <InstalledSkillHeader
        skill={skill}
        from={from}
        onBack={onBack}
        onRemoveComplete={onRemoveComplete}
        invocationStats={invocationStats}
        lastTest={snapshot?.last_test_by_skill[skill.name]}
        onOpenHistory={() => setIsHistoryOpen(true)}
      />

      <div className="skill-page-grid">
        <div className="skill-page-column-main">
          <SkillLocationsCard
            skill={skill}
            skillMdPath={!isPluginManaged ? skillMdPath : undefined}
            skillMdDeployment={deployment ?? undefined}
          />

          <SkillMarkdownCard
            skill={skill}
            isPluginManaged={isPluginManaged}
            deploymentUnresolved={deploymentUnresolved}
            ownDeploymentOptions={ownDeployments(skill)}
            onSelectDeployment={(path) => openSkill(skill.name, path)}
            rawContent={rawContent}
            isLoadingContent={isLoadingContent}
            loadError={loadError}
            onRetry={handleRetryLoad}
            isEditing={isEditing}
            isEditorDirty={isEditorDirty}
            onStartEdit={() => setIsEditing(true)}
            isSaving={isSaving}
            saveLabel={needsForkToSave ? "Fork and save" : "Save"}
            onSave={handleSave}
            onCancelEdit={() => {
              setIsEditing(false);
              setIsEditorDirty(false);
            }}
            onDirtyChange={setIsEditorDirty}
          />
        </div>

        <div className="skill-page-column-side">
          <SkillAssistantPanel
            skill={skill}
            rawContent={rawContent}
            skillMdPath={skillMdPath}
            isPluginManaged={isPluginManaged}
            onApplied={handleApplied}
            onDiskChanged={() => {
              if (skillMdPath) loadContent(skillMdPath, false);
            }}
            showHistory={isHistoryOpen}
            onCloseHistory={() => setIsHistoryOpen(false)}
          />
        </div>
      </div>
    </div>
  );
}
