// ============================================================================
// SkillPage - Full-page view of an installed skill: header (name, one
// primary action, an assistant trigger, overflow menu, chips, source ledger),
// the "where it lives" locations card, the SKILL.md card, and the
// assistant panel in a right-hand overlay drawer.
// ============================================================================

import { useCallback, useEffect, useRef, useState } from "react";
import {
  forkSkill,
  readInstalledSkillMd,
  writeInstalledSkillMdIfUnchanged,
} from "../../lib/skill-api";
import { lifecycleTargetForDeployment } from "../../lib/skill-lifecycle-target";
import { isFeatureEnabled } from "../../lib/feature-flags";
import { editableDeployments } from "@skill-studio/lib";
import type { Deployment, InstalledSkill } from "@skill-studio/lib";
import type { ActiveView } from "../../store/appStore";
import { useAppStore } from "../../store/appStore";
import { PageShell } from "../Shell/PageShell";
import { DiscardChangesDialog } from "./DiscardChangesDialog";
import { InstalledSkillHeader } from "./InstalledSkillHeader";
import { backLabel } from "./skill-page-nav";
import { SkillAssistantDrawer } from "./SkillAssistantDrawer";
import { SkillAssistantPanel } from "./SkillAssistantPanel";
import { useSkillAssistantNavigation } from "./skill-assistant-view-policy";
import { SkillCompareDialog } from "./SkillCompareDialog";
import { SkillFrontmatterRepairDialog } from "./SkillFrontmatterRepairDialog";
import { SkillLocationsCard } from "./SkillLocationsCard";
import { SkillMarkdownCard } from "./SkillMarkdownCard";
import { SkillPageHeaderActions } from "./SkillPageHeaderActions";
import { SkillPropertiesRail } from "./SkillPropertiesRail";
import { SkillRepairCard } from "./SkillRepairCard";
import { saveSkillEditorDraft } from "./skill-editor-save";
import { useSkillPageActions } from "./skill-page-actions";
import { resolveSkillPageDeployment } from "./skill-page-deployment";
import { useSkillCompareDialog } from "./useSkillCompareDialog";
import { useSkillEscapeGuard } from "./useSkillEscapeGuard";
import { useSkillFrontmatterRepair } from "./useSkillFrontmatterRepair";
import { useSkillMdEditorState } from "./useSkillMdEditorState";

interface SkillPageProps {
  /** `null` when the skill named by the route was removed since the page opened. */
  skill: InstalledSkill | null;
  /** The specific deployment the caller clicked, when known - see `ActiveView`'s "skill" kind. */
  deploymentPath?: string;
  onBack: () => void;
  onRemoveComplete: () => void;
  /** The view the page was opened from, for the back button's label. */
  from: ActiveView;
}

interface UseSkillMdContentParams {
  skill: InstalledSkill | null;
  skillMdPath: string | undefined;
  deployment: Deployment | undefined;
  addToast: ReturnType<typeof useAppStore.getState>["addToast"];
}

/**
 * Owns SKILL.md's raw content, its loading/error/saving state, and the fork-
 * before-save flow - all keyed on `skillMdPath`. Pulled out of `SkillPage`
 * since every piece here (state, the load effect, save, retry, apply) closes
 * over the same `currentSkillMdPathRef` staleness check.
 */
function useSkillMdContent({ skill, skillMdPath, deployment, addToast }: UseSkillMdContentParams) {
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

  /** Set when the path change is a copy switch on the same skill - the old copy's content stays up (no skeleton), so the card keeps its height and the page doesn't jump. */
  const lastLoadedSkillRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    // A path change (including to/from `undefined`) can never carry over a
    // stale draft or edit mode from a different copy of the skill.
    const isCopySwitch = skill?.name !== undefined && lastLoadedSkillRef.current === skill.name;
    lastLoadedSkillRef.current = skill?.name;
    currentSkillMdPathRef.current = skillMdPath;
    // react-doctor-disable-next-line react-hooks-js/set-state-in-effect -- syncs an async load: skillMdPath changing kicks off loadContent below, an external Tauri read
    if (!isCopySwitch) setRawContent(null);
    // react-doctor-disable-next-line react-hooks-js/set-state-in-effect -- resets load state before the same external load below fires for the new path
    setIsLoadingContent(false);
    // react-doctor-disable-next-line react-hooks-js/set-state-in-effect -- resets load state before the same external load below fires for the new path
    setLoadError(null);
    if (skillMdPath) loadContent(skillMdPath, !isCopySwitch);
  }, [skill?.name, skillMdPath, loadContent]);

  // A dotagents/skills.sh-managed skill would have its edits overwritten by
  // the next sync/update - saving forks it first so the edit sticks.
  const needsForkToSave = skill?.source_kind === "dotagents" || skill?.source_kind === "skills-sh";

  // A Promise chain, not a try/finally statement, so the compiler can still
  // optimize this component (it doesn't support `finally` clauses yet).
  const handleSave = (content: string, openedContent: string, onSaved: () => void) => {
    // Ignore a duplicate save request (e.g. Cmd+S fired while the Save
    // button's own click is already in flight).
    if (!skill || !skillMdPath || isSaving) return;
    setIsSaving(true);

    // A fork failure already shows its own toast and must skip the write -
    // this flag lets the generic catch below tell that case apart from a
    // write failure without a second, redundant toast.
    let forkFailed = false;
    // `skillMdPath` is only set once `deployment` resolves (see above), so
    // it's non-null here.
    const forkIfNeeded = needsForkToSave
      ? forkSkill(lifecycleTargetForDeployment(deployment!)).catch((err) => {
          forkFailed = true;
          addToast({
            type: "error",
            title: "Couldn't fork before saving",
            message: err instanceof Error ? err.message : "Unknown error",
          });
        })
      : Promise.resolve();

    return forkIfNeeded
      .then(() => {
        if (forkFailed) return;
        return saveSkillEditorDraft(
          { path: skillMdPath, openedContent, draftContent: content },
          writeInstalledSkillMdIfUnchanged,
        ).then((savedDraft) => {
          setRawContent(savedDraft.openedContent);
          onSaved();
        });
      })
      .catch((err) => {
        const message = err instanceof Error ? err.message : String(err);
        const isConflict = message.includes("SKILL.md changed on disk since it was loaded");
        if (isConflict) loadContent(skillMdPath, false);
        addToast({
          type: "error",
          title: isConflict ? "SKILL.md changed on disk" : "Couldn't save SKILL.md",
          message: isConflict
            ? "Your draft is still open. Cancel editing to view the latest file; you will be asked before the draft is discarded."
            : message,
        });
      })
      .finally(() => {
        setIsSaving(false);
      });
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

  return {
    rawContent,
    isLoadingContent,
    loadError,
    isSaving,
    needsForkToSave,
    loadContent,
    handleSave,
    handleRetryLoad,
    handleApplied,
  };
}

/**
 * Full-page view of an installed skill: `InstalledSkillHeader` (which owns
 * the back button, name, primary action, assistant trigger, overflow menu,
 * chips, and source ledger), then a single-column body - `SkillLocationsCard`
 * and `SkillMarkdownCard` - with `SkillAssistantPanel` rendered inside a
 * `SkillAssistantDrawer` overlay.
 */
export function SkillPage({
  skill,
  deploymentPath,
  onBack,
  onRemoveComplete,
  from,
}: SkillPageProps) {
  const addToast = useAppStore((state) => state.addToast);
  const openSkill = useAppStore((state) => state.openSkill);
  const activeView = useAppStore((state) => state.activeView);
  const clearSkillIntent = useAppStore((state) => state.clearSkillIntent);
  const isAssistantOpen = useAppStore((state) => state.isAssistantOpen);
  const setIsAssistantOpen = useAppStore((state) => state.setIsAssistantOpen);
  const { isRunsOpen, openAssistant, closeAssistant, openRuns, closeRuns } =
    useSkillAssistantNavigation(skill?.name, setIsAssistantOpen);
  const pageActions = useSkillPageActions(skill, onRemoveComplete);
  const [isFrontmatterRepairOpen, setIsFrontmatterRepairOpen] = useState(false);
  const assistantTriggerRef = useRef<HTMLButtonElement>(null);

  const { isCompareOpen, setIsCompareOpen } = useSkillCompareDialog(skill?.name);

  // Opening with `intent: "compare"` shows the dialog exactly once - the
  // intent is cleared as soon as it opens, so navigating away and back to
  // this page (without a fresh compare request) never reopens it.
  useEffect(() => {
    if (activeView.kind === "skill" && activeView.intent === "compare") {
      // react-doctor-disable-next-line react-hooks-js/set-state-in-effect -- syncs from an external source (the app's navigation intent), not a value derivable at render
      setIsCompareOpen(true);
      clearSkillIntent();
    }
  }, [activeView, clearSkillIntent, setIsCompareOpen]);

  const { deployment, deploymentUnresolved, isDeploymentBroken, skillMdPath, isPluginManaged } =
    resolveSkillPageDeployment(skill, deploymentPath);

  const { selectedFrontmatterRepair, setFrontmatterRepair } = useSkillFrontmatterRepair(deployment);

  const {
    rawContent,
    isLoadingContent,
    loadError,
    isSaving,
    needsForkToSave,
    loadContent,
    handleSave: saveContent,
    handleRetryLoad,
    handleApplied,
  } = useSkillMdContent({ skill, skillMdPath, deployment, addToast });

  const {
    editorOpenedContent,
    setEditorOpenedContent,
    isEditing,
    isEditorDirty,
    setIsEditorDirty,
  } = useSkillMdEditorState(skill?.name, skillMdPath);

  const { pendingDiscard, setPendingDiscard } = useSkillEscapeGuard(
    isEditing,
    isEditorDirty,
    onBack,
  );

  const handleSave = (content: string) => {
    if (editorOpenedContent === null) return;
    saveContent(content, editorOpenedContent, () => {
      setEditorOpenedContent(null);
      setIsEditorDirty(false);
    });
  };

  const startEditing = () => {
    if (rawContent === null) return;
    setEditorOpenedContent(rawContent);
    setIsEditorDirty(false);
  };

  if (!skill) {
    const name = activeView.kind === "skill" ? activeView.name : "";
    return (
      <PageShell title={name} parent={{ label: backLabel(from), onClick: onBack }}>
        <p className="text-body text-text-tertiary">This skill is no longer installed.</p>
      </PageShell>
    );
  }

  return (
    <PageShell
      title={skill.name}
      parent={{ label: backLabel(from), onClick: onBack }}
      actions={
        <SkillPageHeaderActions
          actions={pageActions}
          assistantEnabled={isFeatureEnabled("skill-assistant")}
          isAssistantOpen={isAssistantOpen}
          onOpenAssistant={openAssistant}
          assistantTriggerRef={assistantTriggerRef}
        />
      }
    >
      <div className="grid grid-cols-1 gap-8 min-[900px]:grid-cols-[minmax(0,1fr)_260px]">
        <div className="flex min-w-0 flex-col gap-6">
          <InstalledSkillHeader
            skill={skill}
            deployment={deployment ?? undefined}
            frontmatterRepair={selectedFrontmatterRepair}
            onFixYaml={() => setIsFrontmatterRepairOpen(true)}
            onEditManually={startEditing}
          />

          <SkillLocationsCard skill={skill} onCompareCopies={() => setIsCompareOpen(true)} />

          {deployment && isDeploymentBroken ? (
            <SkillRepairCard skill={skill} deployment={deployment} />
          ) : (
            <SkillMarkdownCard
              skill={skill}
              isPluginManaged={isPluginManaged}
              deploymentUnresolved={deploymentUnresolved}
              ownDeploymentOptions={editableDeployments(skill)}
              deployment={deployment ?? undefined}
              onSelectDeployment={(path) => openSkill(skill.name, path)}
              rawContent={rawContent}
              isLoadingContent={isLoadingContent}
              loadError={loadError}
              onRetry={handleRetryLoad}
              editState={
                editorOpenedContent !== null
                  ? {
                      kind: "editing",
                      openedContent: editorOpenedContent,
                      isDirty: isEditorDirty,
                      isSaving,
                    }
                  : { kind: "viewing" }
              }
              onStartEdit={startEditing}
              saveLabel={needsForkToSave ? "Fork and save" : "Save"}
              onSave={handleSave}
              onCancelEdit={() => {
                setEditorOpenedContent(null);
                setIsEditorDirty(false);
              }}
              onDirtyChange={setIsEditorDirty}
            />
          )}
        </div>

        <SkillPropertiesRail skill={skill} updateAction={pageActions.primaryAction} />
      </div>

      <SkillAssistantDrawer
        isOpen={isAssistantOpen && isFeatureEnabled("skill-assistant")}
        onClose={closeAssistant}
        triggerRef={assistantTriggerRef}
      >
        <SkillAssistantPanel
          skill={skill}
          rawContent={rawContent}
          skillMdPath={skillMdPath}
          isPluginManaged={isPluginManaged}
          onApplied={handleApplied}
          onDiskChanged={() => {
            if (skillMdPath) loadContent(skillMdPath, false);
          }}
          showHistory={isRunsOpen}
          onOpenHistory={openRuns}
          onCloseHistory={closeRuns}
        />
      </SkillAssistantDrawer>

      {isCompareOpen && (
        <SkillCompareDialog skill={skill} onClose={() => setIsCompareOpen(false)} />
      )}

      {isFrontmatterRepairOpen && selectedFrontmatterRepair && deployment && (
        <SkillFrontmatterRepairDialog
          target={lifecycleTargetForDeployment(deployment)}
          preview={selectedFrontmatterRepair}
          onClose={() => setIsFrontmatterRepairOpen(false)}
          onEditManually={startEditing}
          onApplied={() => {
            setFrontmatterRepair(null);
            if (skillMdPath) loadContent(skillMdPath, false);
          }}
        />
      )}

      <DiscardChangesDialog
        open={pendingDiscard !== null}
        onOpenChange={(open) => {
          if (!open) setPendingDiscard(null);
        }}
        onDiscard={() => {
          pendingDiscard?.();
          setPendingDiscard(null);
        }}
      />
    </PageShell>
  );
}
