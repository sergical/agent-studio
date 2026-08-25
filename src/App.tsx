// ============================================================================
// Skill Studio - Main Application
// Shell: Sidebar + main view (Dashboard, Global, Project, Plugins, Coverage,
// Issues, Activity, or Discover) + an optional installed-skill detail drawer
// ============================================================================

import { useEffect, useRef } from "react";
import { homeDir } from "@tauri-apps/api/path";
import { Sidebar } from "./components/Sidebar/Sidebar";
import { SkillActivityView } from "./components/Activity/SkillActivityView";
import { SkillDashboard } from "./components/Dashboard/SkillDashboard";
import { SkillsScopeView } from "./components/SkillsScopeView";
import { SkillCoverageView } from "./components/Coverage/SkillCoverageView";
import { SkillIssuesView } from "./components/Issues/SkillIssuesView";
import { PluginSkillsView } from "./components/Plugins/PluginSkillsView";
import { SkillDetail } from "./components/SkillDetail/SkillDetail";
import { SkillStore } from "./components/SkillStore";
import { ToastContainer } from "./components/ui/ToastContainer";
import { useSkillSnapshot } from "./hooks/useSkillSnapshot";
import { registerSkillProjects, unregisterSkillProject } from "./lib/skill-api";
import { useAppStore } from "./store/appStore";
import "./App.css";

function App() {
  const { snapshot, isLoading, requestRescan } = useSkillSnapshot();
  const activeView = useAppStore((state) => state.activeView);
  const selectedSkill = useAppStore((state) => state.selectedSkill);
  const setSelectedSkill = useAppStore((state) => state.setSelectedSkill);
  const userAddedProjects = useAppStore((state) => state.userAddedProjects);
  const excludedProjects = useAppStore((state) => state.excludedProjects);
  const removeProject = useAppStore((state) => state.removeProject);
  const addToast = useAppStore((state) => state.addToast);

  const onSelectSkill = (name: string, deploymentPath?: string) =>
    setSelectedSkill({ name, deploymentPath });

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

  const selectedSkillRecord = snapshot?.skills.find((s) => s.name === selectedSkill?.name) ?? null;
  const invocationStats = snapshot?.invocations.find((s) => s.skill === selectedSkill?.name);

  let main: React.ReactNode;
  if (activeView.kind === "dashboard") {
    main = (
      <SkillDashboard snapshot={snapshot} isLoading={isLoading} onSelectSkill={onSelectSkill} />
    );
  } else if (activeView.kind === "global") {
    main = (
      <SkillsScopeView
        scope={{ kind: "global" }}
        snapshot={snapshot}
        onSelectSkill={onSelectSkill}
      />
    );
  } else if (activeView.kind === "project") {
    main = (
      <SkillsScopeView
        scope={{ kind: "project", path: activeView.path }}
        snapshot={snapshot}
        onSelectSkill={onSelectSkill}
      />
    );
  } else if (activeView.kind === "plugins") {
    main = (
      <PluginSkillsView
        harness={activeView.harness}
        snapshot={snapshot}
        onSelectSkill={onSelectSkill}
      />
    );
  } else if (activeView.kind === "coverage") {
    main = <SkillCoverageView snapshot={snapshot} onSelectSkill={onSelectSkill} />;
  } else if (activeView.kind === "issues") {
    main = (
      <SkillIssuesView
        snapshot={snapshot}
        issueKind={activeView.issueKind}
        onSelectSkill={onSelectSkill}
      />
    );
  } else if (activeView.kind === "activity") {
    main = <SkillActivityView snapshot={snapshot} onSelectSkill={onSelectSkill} />;
  } else {
    main = <SkillStore />;
  }

  return (
    <div className="flex h-screen overflow-hidden bg-[var(--color-bg-primary)]">
      <Sidebar snapshot={snapshot} isLoading={isLoading} requestRescan={requestRescan} />
      <main className="flex-1 overflow-y-auto">{main}</main>

      {activeView.kind !== "discover" && selectedSkillRecord && (
        <>
          <div className="skill-detail-overlay" onClick={() => setSelectedSkill(null)} />
          <SkillDetail
            skill={selectedSkillRecord}
            deploymentPath={selectedSkill?.deploymentPath}
            invocationStats={invocationStats}
            onClose={() => setSelectedSkill(null)}
            onRemoveComplete={() => setSelectedSkill(null)}
          />
        </>
      )}

      <ToastContainer />
    </div>
  );
}

export default App;
