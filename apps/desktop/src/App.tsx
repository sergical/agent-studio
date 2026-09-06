// ============================================================================
// Skill Studio - Main Application
// Shell: Sidebar + main view (Home, Skills, Activity, Packs, or a full-page
// installed-skill view)
// ============================================================================

import { useEffect, useRef } from "react";
import { homeDir } from "@tauri-apps/api/path";
import { TooltipProvider } from "@skill-studio/ui";
import { Toaster } from "sonner";
import { AddSkillSheet } from "./components/AddSkill/AddSkillSheet";
import { Sidebar } from "./components/Sidebar/Sidebar";
import { SkillActivityView } from "./components/Activity/SkillActivityView";
import { HomeView } from "./components/Home/HomeView";
import { SettingsView } from "./components/Settings/SettingsView";
import { LearnView } from "./components/Learn/LearnView";
import { SkillsView } from "./components/SkillList/SkillsView";
import { PluginSkillsView } from "./components/SkillList/PluginSkillsView";
import { PacksView } from "./components/Packs/PacksView";
import { SkillPage } from "./components/SkillDetail/SkillPage";
import { useSkillSnapshot } from "./hooks/useSkillSnapshot";
import {
  onTrialExpired,
  registerSkillProjects,
  restoreTrashedSkill,
  unregisterSkillProject,
} from "./lib/skill-api";
import { useAppStore } from "./store/appStore";
import "./App.css";

function App() {
  const { snapshot, isLoading, requestRescan } = useSkillSnapshot();
  const resolvedTheme = useAppStore((state) => state.resolvedTheme);
  const activeView = useAppStore((state) => state.activeView);
  const openSkill = useAppStore((state) => state.openSkill);
  const closeSkill = useAppStore((state) => state.closeSkill);
  const userAddedProjects = useAppStore((state) => state.userAddedProjects);
  const excludedProjects = useAppStore((state) => state.excludedProjects);
  const removeProject = useAppStore((state) => state.removeProject);
  const addToast = useAppStore((state) => state.addToast);

  const onSelectSkill = (name: string, deploymentPath?: string) => openSkill(name, deploymentPath);

  // Re-register the user's remembered projects, and re-apply their remembered
  // exclusions, with the backend once on startup, so future background
  // rebuilds always reflect both. A legacy persisted entry equal to the home
  // directory (the global scope, never a project) is dropped rather than
  // sent - the backend would refuse it anyway.
  const didRegisterStartupProjects = useRef(false);
  useEffect(() => {
    if (didRegisterStartupProjects.current) return;
    didRegisterStartupProjects.current = true;

    (async () => {
      const home = await homeDir().catch(() => null);
      const projectsToRegister = home
        ? userAddedProjects.filter((path) => path !== home)
        : userAddedProjects;
      if (home) {
        for (const path of userAddedProjects) {
          if (path === home) removeProject(path);
        }
      }

      if (projectsToRegister.length > 0) {
        try {
          await registerSkillProjects(projectsToRegister);
        } catch (err) {
          addToast({
            type: "error",
            title: "Couldn't restore tracked projects",
            message: err instanceof Error ? err.message : "Unknown error",
          });
        }
      }
      for (const path of excludedProjects) {
        unregisterSkillProject(path);
      }
    })();
  }, [userAddedProjects, excludedProjects, removeProject, addToast]);

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

  let main: React.ReactNode;
  if (activeView.kind === "home") {
    main = <HomeView snapshot={snapshot} isLoading={isLoading} onSelectSkill={onSelectSkill} />;
  } else if (activeView.kind === "skills") {
    main = <SkillsView snapshot={snapshot} onSelectSkill={onSelectSkill} />;
  } else if (activeView.kind === "plugins") {
    main = <PluginSkillsView snapshot={snapshot} onSelectSkill={onSelectSkill} />;
  } else if (activeView.kind === "activity") {
    main = <SkillActivityView snapshot={snapshot} onSelectSkill={onSelectSkill} />;
  } else if (activeView.kind === "packs") {
    main = <PacksView />;
  } else if (activeView.kind === "learn") {
    main = <LearnView section={activeView.section} />;
  } else if (activeView.kind === "settings") {
    main = <SettingsView />;
  } else {
    const skill = snapshot?.skills.find((s) => s.name === activeView.name) ?? null;
    main = (
      <SkillPage
        skill={skill}
        deploymentPath={activeView.deploymentPath}
        onBack={closeSkill}
        onRemoveComplete={closeSkill}
        from={activeView.from}
      />
    );
  }

  return (
    <TooltipProvider delay={400}>
      <div className="flex h-screen overflow-hidden bg-[var(--color-bg-primary)]">
        <Sidebar snapshot={snapshot} isLoading={isLoading} requestRescan={requestRescan} />
        <main className="flex-1 overflow-y-auto">{main}</main>

        <AddSkillSheet />
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
              description: "text-text-secondary",
              actionButton: "!bg-bg-tertiary !text-text-primary !border !border-border",
            },
          }}
        />
      </div>
    </TooltipProvider>
  );
}

export default App;
