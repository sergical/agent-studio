// ============================================================================
// SkillPage - Full-page view of an installed skill: header (name, one
// primary action, an assistant trigger, overflow menu, chips, metadata
// line), the "where it lives" locations card, the SKILL.md card, and the
// assistant panel in a right-hand overlay drawer.
// ============================================================================

import { useCallback, useEffect, useEffectEvent, useRef, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { useSkillSnapshot } from "../../hooks/useSkillSnapshot";
import { forkSkill, readInstalledSkillMd, writeInstalledSkillMd } from "../../lib/skill-api";
import { isFeatureEnabled } from "../../lib/feature-flags";
import {
  editableDeployments,
  isUnresolvedDeployment,
  ownDeployments,
  skillMdPathForDeployment,
} from "@skill-studio/lib";
import type { InstalledSkill, SkillInvocationStats } from "@skill-studio/lib";
import type { ActiveView } from "../../store/appStore";
import { useAppStore } from "../../store/appStore";
import { DiscardChangesDialog } from "./DiscardChangesDialog";
import { InstalledSkillHeader } from "./InstalledSkillHeader";
import { SkillAssistantDrawer } from "./SkillAssistantDrawer";
import { SkillAssistantPanel } from "./SkillAssistantPanel";
import { SkillCompareDialog } from "./SkillCompareDialog";
import { SkillLocationsCard } from "./SkillLocationsCard";
import { SkillMarkdownCard } from "./SkillMarkdownCard";
import { SkillRepairCard } from "./SkillRepairCard";

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

interface UseSkillMdContentParams {
  skill: InstalledSkill | null;
  skillMdPath: string | undefined;
  deployment: { path: string } | undefined;
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
  const handleSave = (content: string, onSaved: () => void) => {
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
      ? forkSkill(skill.name, deployment!.path).catch((err) => {
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
        return writeInstalledSkillMd(skillMdPath, content).then(() => {
          setRawContent(content);
          onSaved();
        });
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't save SKILL.md",
          message: err instanceof Error ? err.message : "Unknown error",
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
 * chips, and metadata line), then a single-column body - `SkillLocationsCard`
 * and `SkillMarkdownCard` - with `SkillAssistantPanel` rendered inside a
 * `SkillAssistantDrawer` overlay.
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
  const activeView = useAppStore((state) => state.activeView);
  const clearSkillIntent = useAppStore((state) => state.clearSkillIntent);
  const isAssistantOpen = useAppStore((state) => state.isAssistantOpen);
  const setIsAssistantOpen = useAppStore((state) => state.setIsAssistantOpen);
  const { snapshot } = useSkillSnapshot();

  const [isEditing, setIsEditing] = useState(false);
  const [isEditorDirty, setIsEditorDirty] = useState(false);
  const [isHistoryOpen, setIsHistoryOpen] = useState(false);
  const [isCompareOpen, setIsCompareOpen] = useState(false);
  const assistantTriggerRef = useRef<HTMLButtonElement>(null);

  // Set while the discard-changes guard is waiting on the user - runs on
  // confirm, cleared on cancel. The old native confirm() prompt made this a
  // synchronous check; the dialog makes it async instead.
  const [pendingDiscard, setPendingDiscard] = useState<(() => void) | null>(null);

  /** The skill the compare dialog was last shown for, so a plain skill switch (no fresh compare request) closes it instead of carrying it over. */
  const compareSkillNameRef = useRef<string | undefined>(skill?.name);

  // A plain skill switch (no fresh compare request) closes a dialog carried
  // over from the previous skill - adjusted during render, per React's
  // "storing information from previous renders" pattern, since it's a reset
  // keyed off an identity change rather than something to synchronize.
  if (compareSkillNameRef.current !== skill?.name) {
    // react-doctor-disable-next-line react-doctor/no-ref-current-in-render -- adjust-during-render, per React docs "storing information from previous renders"
    compareSkillNameRef.current = skill?.name;
    if (isCompareOpen) setIsCompareOpen(false);
  }

  // Opening with `intent: "compare"` shows the dialog exactly once - the
  // intent is cleared as soon as it opens, so navigating away and back to
  // this page (without a fresh compare request) never reopens it.
  useEffect(() => {
    if (activeView.kind === "skill" && activeView.intent === "compare") {
      // react-doctor-disable-next-line react-hooks-js/set-state-in-effect -- syncs from an external source (the app's navigation intent), not a value derivable at render
      setIsCompareOpen(true);
      clearSkillIntent();
    }
  }, [activeView, clearSkillIntent]);

  // Reads the latest isEditing/isEditorDirty/onBack without making the
  // listener effect below re-subscribe every time one of them changes.
  const onEscapeBack = useEffectEvent(() => {
    if (isEditing && isEditorDirty) {
      setPendingDiscard(() => onBack);
      return;
    }
    onBack();
  });

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
          target.closest("dialog") !== null ||
          target.closest('[role="dialog"]') !== null)
      ) {
        return;
      }
      onEscapeBack();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  /** The skill the history drawer was last shown for, so a skill switch closes it instead of carrying it over. */
  const historySkillNameRef = useRef<string | undefined>(skill?.name);
  if (historySkillNameRef.current !== skill?.name) {
    // react-doctor-disable-next-line react-doctor/no-ref-current-in-render -- adjust-during-render, per React docs "storing information from previous renders"
    historySkillNameRef.current = skill?.name;
    if (isHistoryOpen) setIsHistoryOpen(false);
  }

  // The deployment this page edits: only the one the caller clicked, when
  // given - a stale `deploymentPath` (the copy was removed by a rescan) must
  // not silently fall back to a different copy of the skill. With no
  // `deploymentPath` at all, fall back to the skill's first physical file
  // (a symlink only points at another copy), then its first own deployment,
  // then its first deployment (a plugin-only skill has no own deployment).
  const requestedDeployment =
    skill && deploymentPath ? skill.deployments.find((d) => d.path === deploymentPath) : undefined;
  const deploymentUnresolved = Boolean(skill && deploymentPath && !requestedDeployment);
  const deployment =
    skill &&
    (deploymentPath
      ? requestedDeployment
      : editableDeployments(skill)[0] || ownDeployments(skill)[0] || skill.deployments[0]);
  // A broken deployment symlink can't be read at all - SkillRepairCard takes
  // over the SKILL.md card's spot instead of firing the doomed
  // `readInstalledSkillMd` for it (see SkillMarkdownCard's old "Unknown
  // error" + Retry state for a broken link).
  const isDeploymentBroken = Boolean(deployment && isUnresolvedDeployment(deployment));
  const skillMdPath =
    deployment && !isDeploymentBroken ? skillMdPathForDeployment(deployment) : undefined;
  const isPluginManaged = Boolean(deployment?.plugin);

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
  } = useSkillMdContent({ skill, skillMdPath, deployment: deployment || undefined, addToast });

  // A skill switch can't carry over a draft or edit mode from a different
  // skill - adjusted during render (same single-ref pattern as
  // `compareSkillNameRef`/`historySkillNameRef` above), keyed off the same
  // identity `useSkillMdContent`'s own reset uses.
  const editSkillNameRef = useRef<string | undefined>(skill?.name);
  if (editSkillNameRef.current !== skill?.name) {
    // react-doctor-disable-next-line react-doctor/no-ref-current-in-render -- adjust-during-render, per React docs "storing information from previous renders"
    editSkillNameRef.current = skill?.name;
    if (isEditing) setIsEditing(false);
    if (isEditorDirty) setIsEditorDirty(false);
  }

  // A deployment/path change (same skill, different copy) can't carry over a
  // draft or edit mode either - same adjust-during-render idiom, keyed off
  // the path instead of the skill name.
  const editSkillMdPathRef = useRef<string | undefined>(skillMdPath);
  if (editSkillMdPathRef.current !== skillMdPath) {
    // react-doctor-disable-next-line react-doctor/no-ref-current-in-render -- adjust-during-render, per React docs "storing information from previous renders"
    editSkillMdPathRef.current = skillMdPath;
    if (isEditing) setIsEditing(false);
    if (isEditorDirty) setIsEditorDirty(false);
  }

  const handleSave = (content: string) => {
    saveContent(content, () => {
      setIsEditing(false);
      setIsEditorDirty(false);
    });
  };

  if (!skill) {
    return (
      <div className="mx-auto flex max-w-[1200px] flex-col gap-6 pt-7 pb-7 px-8">
        <div className="flex items-center gap-4">
          <button
            className="flex shrink-0 items-center gap-1.5 border-0 bg-transparent p-1 text-small text-text-tertiary transition-colors hover:text-text-primary"
            onClick={onBack}
            aria-label="Back"
          >
            <ArrowLeft size={16} />
            <span>{from.kind === "home" ? "Home" : "Back"}</span>
          </button>
        </div>
        <p className="text-body text-text-tertiary">This skill is no longer installed.</p>
      </div>
    );
  }

  return (
    <div className="mx-auto flex max-w-[1200px] flex-col gap-6 pt-7 pb-7 px-8">
      <InstalledSkillHeader
        skill={skill}
        deployment={deployment ?? undefined}
        from={from}
        onBack={onBack}
        onRemoveComplete={onRemoveComplete}
        invocationStats={invocationStats}
        lastTest={snapshot?.last_test_by_skill[skill.name]}
        onOpenHistory={() => {
          setIsHistoryOpen(true);
          setIsAssistantOpen(true);
        }}
        isAssistantOpen={isAssistantOpen}
        onOpenAssistant={() => setIsAssistantOpen(true)}
        assistantTriggerRef={assistantTriggerRef}
      />

      <div className="flex min-w-0 flex-col gap-6">
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
              isEditing
                ? { kind: "editing", isDirty: isEditorDirty, isSaving }
                : { kind: "viewing" }
            }
            onStartEdit={() => setIsEditing(true)}
            saveLabel={needsForkToSave ? "Fork and save" : "Save"}
            onSave={handleSave}
            onCancelEdit={() => {
              setIsEditing(false);
              setIsEditorDirty(false);
            }}
            onDirtyChange={setIsEditorDirty}
          />
        )}
      </div>

      <SkillAssistantDrawer
        isOpen={isAssistantOpen && isFeatureEnabled("skill-assistant")}
        onClose={() => setIsAssistantOpen(false)}
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
          showHistory={isHistoryOpen}
          onCloseHistory={() => setIsHistoryOpen(false)}
        />
      </SkillAssistantDrawer>

      {isCompareOpen && (
        <SkillCompareDialog skill={skill} onClose={() => setIsCompareOpen(false)} />
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
    </div>
  );
}
