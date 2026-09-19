// ============================================================================
// Skill Studio - Main Application
// Shell: Sidebar + main view (Home, Skills, Activity, or a full-page
// installed-skill view)
// ============================================================================

import { useEffect, useRef, useState } from "react";
import { TooltipProvider } from "@skill-studio/ui";
import { Toaster } from "sonner";
import { AddSkillSheet } from "./components/AddSkill/AddSkillSheet";
import { FirstRunGate } from "./components/FirstRun/FirstRunScreen";
import { CommandPalette } from "./components/CommandPalette/CommandPalette";
import { Sidebar } from "./components/Sidebar/Sidebar";
import { SkillActivityView } from "./components/Activity/SkillActivityView";
import { HomeView } from "./components/Home/HomeView";
import { SettingsView } from "./components/Settings/SettingsView";
import { LearnView } from "./components/Learn/LearnView";
import { SkillsView } from "./components/SkillList/SkillsView";
import { PluginSkillsView } from "./components/SkillList/PluginSkillsView";
import { SkillPage } from "./components/SkillDetail/SkillPage";
import { useAppShortcuts } from "./hooks/useAppShortcuts";
import { useNativeShell } from "./hooks/useNativeShell";
import { useSkillSnapshot } from "./hooks/useSkillSnapshot";
import {
  dataFolderStatus,
  getTrackedProjects,
  importTrackedProjects,
  invokeErrorMessage,
  onTrialExpired,
  restoreTrashedSkill,
} from "./lib/skill-api";
import { clearLegacyProjectPaths, readLegacyProjectPaths } from "./lib/legacy-project-paths";
import type { ActiveView } from "./store/appStore";
import { useAppStore } from "./store/appStore";
import "./App.css";

/** The `ActiveView` kinds a skill page can be opened from that are worth keeping mounted (but
 * hidden) underneath it, so the back button is instant instead of re-scanning the whole list. */
type ListViewKind = "home" | "skills" | "plugins" | "activity";

function isListViewKind(kind: ActiveView["kind"]): kind is ListViewKind {
  return kind === "home" || kind === "skills" || kind === "plugins" || kind === "activity";
}

/** Unit 6.3's `data_folder_status` answer, before and after it arrives. A
 * discriminated union rather than a `"pending" | string | null` sentinel: a
 * real blocking message can't be confused with the not-yet-answered state
 * just because it happens to read `"pending"`. */
type DataFolderStatusState = { kind: "pending" } | { kind: "ready"; message: string | null };

function App() {
  useNativeShell();
  useAppShortcuts();
  const [firstRunDone, setFirstRunDone] = useState(false);
  // Unit 6.3: the data layer never opened if the app data folder is newer
  // than this build understands - checked once, before anything else that
  // reads from it, and shown in place of the normal chrome (a toast is not
  // enough: there is no snapshot, no settings, nothing behind it to toast
  // over). Starts in the `pending` kind so the first-run gate below can't
  // paint before the answer lands and then get replaced by the blocking
  // screen once it does; a discriminated union (rather than a `"pending"`
  // string sentinel) keeps a real blocking message that happened to read
  // "pending" from being mistaken for the not-yet-answered state.
  const [dataFolderStatusState, setDataFolderStatusState] = useState<DataFolderStatusState>({
    kind: "pending",
  });
  useEffect(() => {
    dataFolderStatus()
      .then((message) => setDataFolderStatusState({ kind: "ready", message }))
      .catch(() => setDataFolderStatusState({ kind: "ready", message: null }));
  }, []);
  const { snapshot, emittedSnapshotRevision, isLoading, requestRescan } = useSkillSnapshot();
  const resolvedTheme = useAppStore((state) => state.resolvedTheme);
  const activeView = useAppStore((state) => state.activeView);
  const openSkill = useAppStore((state) => state.openSkill);
  const closeSkill = useAppStore((state) => state.closeSkill);
  const setTrackedProjects = useAppStore((state) => state.setTrackedProjects);
  const addToast = useAppStore((state) => state.addToast);

  const onSelectSkill = (name: string, deploymentPath?: string) => openSkill(name, deploymentPath);

  // Load the tracked project list once on startup. A machine with leftover
  // localStorage entries imports them into `~/.agents/skill-studio.json`
  // first (home-directory entries are the backend's job to drop, same as any
  // other add) and clears localStorage only once that import succeeds; every
  // other machine reads the saved list straight from the backend.
  const didLoadStartupProjects = useRef(false);
  useEffect(() => {
    if (didLoadStartupProjects.current) return;
    didLoadStartupProjects.current = true;

    (async () => {
      const legacy = readLegacyProjectPaths();
      try {
        const projects = legacy
          ? await importTrackedProjects(legacy.added, legacy.excluded)
          : await getTrackedProjects();
        setTrackedProjects(projects);
        if (legacy) clearLegacyProjectPaths();
      } catch (err) {
        addToast({
          type: "error",
          title: "Couldn't load project folders",
          message: invokeErrorMessage(err),
        });
      }
    })();
  }, [setTrackedProjects, addToast]);

  // A trial expiring is driven by the backend's own timer, not a user
  // action here - surface it as a toast with a Restore action rather than
  // silently updating the snapshot.
  useEffect(() => {
    return onTrialExpired(({ name, trash_path }) => {
      addToast({
        type: "warning",
        title: `Trial ended: ${name} moved to skills-trash`,
        duration: 15000,
        action: {
          label: "Restore",
          onClick: () => {
            restoreTrashedSkill(trash_path).catch((err) => {
              addToast({
                type: "error",
                title: "Couldn't restore skill",
                message: err instanceof Error ? err.message : "Unknown error",
              });
            });
          },
        },
      });
    });
  }, [addToast]);

  /** One skill view - the page it opens, standalone (no kept-alive list underneath). */
  function renderSkillPage(view: Extract<ActiveView, { kind: "skill" }>): React.ReactNode {
    const skill = snapshot?.skills.find((s) => s.name === view.name) ?? null;
    return (
      <SkillPage
        skill={skill}
        deploymentPath={view.deploymentPath}
        onBack={closeSkill}
        onRemoveComplete={closeSkill}
        from={view.from}
      />
    );
  }

  /**
   * `kind`'s list view, plus (when `skillView` is set) the skill page open over it. Renders the
   * same wrapper shape - a `<>` holding the list's div and, conditionally, the `SkillPage` - whether
   * the list is the view on screen (`skillView` null) or hidden behind an open skill's page. Keeping
   * that shape identical in both cases is what lets React preserve the list's component instance
   * across opening and closing a skill, instead of unmounting and remounting it: reopening the list
   * is then instant instead of re-scanning and re-rendering it from scratch.
   */
  function renderListLayer(
    kind: ListViewKind,
    skillView: Extract<ActiveView, { kind: "skill" }> | null,
  ): React.ReactNode {
    const isShown = skillView === null;
    let list: React.ReactNode;
    if (kind === "home") {
      list = (
        <HomeView
          snapshot={snapshot}
          isLoading={isLoading}
          onSelectSkill={onSelectSkill}
          active={isShown}
        />
      );
    } else if (kind === "skills") {
      list = <SkillsView snapshot={snapshot} onSelectSkill={onSelectSkill} active={isShown} />;
    } else if (kind === "plugins") {
      list = <PluginSkillsView snapshot={snapshot} onSelectSkill={onSelectSkill} />;
    } else {
      list = <SkillActivityView snapshot={snapshot} onSelectSkill={onSelectSkill} />;
    }
    return (
      <>
        {/* `hidden` (not an unmounting swap) so the list's own scroll container keeps its
            `scrollTop` - display:none doesn't reset it, unlike removing the element would.
            `inert` on top so nothing inside it is focusable, clickable, or reachable by AT while
            the skill page covers it. */}
        <div hidden={!isShown} inert={!isShown} className="flex min-h-0 flex-1 flex-col">
          {list}
        </div>
        {skillView && renderSkillPage(skillView)}
      </>
    );
  }

  let main: React.ReactNode;
  switch (activeView.kind) {
    case "home":
    case "skills":
    case "plugins":
    case "activity":
      main = renderListLayer(activeView.kind, null);
      break;
    case "learn":
      main = <LearnView section={activeView.section} />;
      break;
    case "settings":
      main = <SettingsView snapshot={snapshot} />;
      break;
    case "skill": {
      // Opened from Learn or Settings: neither keeps a list worth reviving, so the
      // skill page fully replaces `main`, same as before.
      const originKind = isListViewKind(activeView.from.kind) ? activeView.from.kind : null;
      main = originKind ? renderListLayer(originKind, activeView) : renderSkillPage(activeView);
      break;
    }
  }

  if (dataFolderStatusState.kind === "pending") {
    // Neither the blocking screen nor the first-run gate is correct yet -
    // render nothing rather than guess and get replaced once the answer
    // lands.
    return null;
  }

  if (dataFolderStatusState.message != null) {
    return (
      <TooltipProvider delay={400}>
        <div className="flex h-screen w-screen items-center justify-center bg-bg-secondary p-8">
          <p className="max-w-md text-center text-sm text-text-primary">
            {dataFolderStatusState.message}
          </p>
        </div>
      </TooltipProvider>
    );
  }

  if (!firstRunDone) {
    return (
      <TooltipProvider delay={400}>
        <FirstRunGate onComplete={() => setFirstRunDone(true)} />
      </TooltipProvider>
    );
  }

  return (
    <TooltipProvider delay={400}>
      <div className="flex h-screen overflow-hidden bg-bg-secondary">
        <Sidebar
          snapshot={snapshot}
          emittedSnapshotRevision={emittedSnapshotRevision}
          requestRescan={requestRescan}
        />
        <div className="flex min-w-0 flex-1 flex-col pr-2 pb-2">
          <div data-tauri-drag-region className="h-9 shrink-0" />
          <main className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-bg-primary">
            {main}
          </main>
        </div>

        <AddSkillSheet />
        <CommandPalette snapshot={snapshot} requestRescan={requestRescan} />
        <Toaster
          position="bottom-right"
          theme={resolvedTheme}
          toastOptions={{
            style: {
              background: "var(--color-bg-elevated)",
              borderColor: "var(--color-border)",
              color: "var(--color-text-primary)",
            },
            classNames: {
              description: "select-text text-text-secondary",
              actionButton: "!bg-bg-tertiary !text-text-primary !border !border-border",
            },
          }}
        />
      </div>
    </TooltipProvider>
  );
}

export default App;
