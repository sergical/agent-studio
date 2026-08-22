// ============================================================================
// Agent Studio - Main Application
// Shell: Sidebar + main view (Dashboard, Global, Project, or Discover) +
// an optional installed-skill detail drawer
// ============================================================================

import { useEffect, useRef } from "react";
import { Sidebar } from "./components/Sidebar/Sidebar";
import { SkillDashboard } from "./components/Dashboard/SkillDashboard";
import { SkillsScopeView } from "./components/SkillsScopeView";
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
  const selectedSkillName = useAppStore((state) => state.selectedSkillName);
  const setSelectedSkillName = useAppStore((state) => state.setSelectedSkillName);
  const userAddedProjects = useAppStore((state) => state.userAddedProjects);
  const excludedProjects = useAppStore((state) => state.excludedProjects);

  // Re-register the user's remembered projects, and re-apply their remembered
  // exclusions, with the backend once on startup, so future background
  // rebuilds always reflect both.
  const didRegisterStartupProjects = useRef(false);
  useEffect(() => {
    if (didRegisterStartupProjects.current) return;
    didRegisterStartupProjects.current = true;
    if (userAddedProjects.length > 0) {
      registerSkillProjects(userAddedProjects);
    }
    for (const path of excludedProjects) {
      unregisterSkillProject(path);
    }
  }, [userAddedProjects, excludedProjects]);

  const selectedSkill = snapshot?.skills.find((s) => s.name === selectedSkillName) ?? null;
  const invocationStats = snapshot?.invocations.find((s) => s.skill === selectedSkillName);

  let main: React.ReactNode;
  if (activeView.kind === "dashboard") {
    main = (
      <SkillDashboard
        snapshot={snapshot}
        isLoading={isLoading}
        onSelectSkill={setSelectedSkillName}
      />
    );
  } else if (activeView.kind === "global") {
    main = (
      <SkillsScopeView
        scope={{ kind: "global" }}
        snapshot={snapshot}
        onSelectSkill={setSelectedSkillName}
      />
    );
  } else if (activeView.kind === "project") {
    main = (
      <SkillsScopeView
        scope={{ kind: "project", path: activeView.path }}
        snapshot={snapshot}
        onSelectSkill={setSelectedSkillName}
      />
    );
  } else {
    main = <SkillStore />;
  }

  return (
    <div className="flex h-screen overflow-hidden bg-[var(--color-bg-primary)]">
      <Sidebar snapshot={snapshot} isLoading={isLoading} requestRescan={requestRescan} />
      <main className="flex-1 overflow-y-auto">{main}</main>

      {activeView.kind !== "discover" && selectedSkill && (
        <>
          <div className="skill-detail-overlay" onClick={() => setSelectedSkillName(null)} />
          <SkillDetail
            skill={selectedSkill}
            invocationStats={invocationStats}
            onClose={() => setSelectedSkillName(null)}
            onRemoveComplete={() => setSelectedSkillName(null)}
          />
        </>
      )}

      <ToastContainer />
    </div>
  );
}

export default App;
