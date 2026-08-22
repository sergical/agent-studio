// ============================================================================
// Sidebar - Left-hand navigation: Overview, Skills (Global + projects), Find
// ============================================================================

import { useCallback, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderPlus, Globe, LayoutDashboard, RefreshCw, Search, X } from "lucide-react";
import { registerSkillProjects, unregisterSkillProject } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";
import type { SkillSnapshot } from "../../lib/skill-types";

interface SidebarProps {
  snapshot: SkillSnapshot | undefined;
  isLoading: boolean;
  requestRescan: () => Promise<void>;
}

function relativeScanTime(scannedAt: string | undefined): string {
  if (!scannedAt) return "never scanned";
  const ms = Date.now() - new Date(scannedAt).getTime();
  if (ms < 0 || Number.isNaN(ms)) return "just now";
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

/** Skill count deployed at global (or plugin) scope. */
function globalSkillCount(snapshot: SkillSnapshot | undefined): number {
  if (!snapshot) return 0;
  return snapshot.skills.filter((s) =>
    s.deployments.some((d) => d.scope === "global" || d.scope === "plugin"),
  ).length;
}

/** Skill count deployed to a specific project directory. */
function projectSkillCount(snapshot: SkillSnapshot | undefined, path: string): number {
  if (!snapshot) return 0;
  return snapshot.skills.filter((s) => s.deployments.some((d) => d.project_path === path)).length;
}

/**
 * Left-hand navigation: Overview (Dashboard), Skills (Global + one row per
 * registered project, with an "Add project…" action), and Find (Discover).
 * The footer shows the snapshot's age and a manual rescan button.
 */
export function Sidebar({ snapshot, isLoading, requestRescan }: SidebarProps) {
  const [isRescanning, setIsRescanning] = useState(false);
  const activeView = useAppStore((state) => state.activeView);
  const setActiveView = useAppStore((state) => state.setActiveView);
  const userAddedProjects = useAppStore((state) => state.userAddedProjects);
  const excludedProjects = useAppStore((state) => state.excludedProjects);
  const addProject = useAppStore((state) => state.addProject);
  const removeProject = useAppStore((state) => state.removeProject);

  // Every project row: user-added paths plus every path the backend
  // discovered on its own (Codex config, Claude Code transcripts), minus
  // anything just excluded - so "Stop tracking" hides a row immediately,
  // without waiting for the next background snapshot rebuild.
  const projects = Array.from(
    new Set([...userAddedProjects, ...(snapshot?.projects ?? [])]),
  ).filter((path) => !excludedProjects.includes(path));

  const handleRefresh = useCallback(async () => {
    setIsRescanning(true);
    await requestRescan();
    // Cleared when the next snapshot lands and `isLoading` flips back to false.
  }, [requestRescan]);

  const handleAddProject = useCallback(async () => {
    const selected = await open({ directory: true, multiple: false, title: "Add Project" });
    if (!selected) return;
    addProject(selected);
    await registerSkillProjects([selected]);
  }, [addProject]);

  const handleRemoveProject = useCallback(
    async (path: string) => {
      removeProject(path);
      await unregisterSkillProject(path);
      if (activeView.kind === "project" && activeView.path === path) {
        setActiveView({ kind: "dashboard" });
      }
    },
    [removeProject, activeView, setActiveView],
  );

  const spinning = isRescanning && isLoading;

  return (
    <nav className="skill-sidebar">
      <div className="skill-sidebar-section">
        <div className="skill-sidebar-section-title">Overview</div>
        <button
          className={`skill-sidebar-item ${activeView.kind === "dashboard" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "dashboard" })}
        >
          <LayoutDashboard size={14} />
          <span>Dashboard</span>
        </button>
      </div>

      <div className="skill-sidebar-section">
        <div className="skill-sidebar-section-title">Skills</div>
        <button
          className={`skill-sidebar-item ${activeView.kind === "global" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "global" })}
        >
          <Globe size={14} />
          <span>Global</span>
          <span className="skill-sidebar-badge">{globalSkillCount(snapshot)}</span>
        </button>

        {projects.map((path) => {
          const basename = path.split("/").filter(Boolean).pop() ?? path;
          return (
            <div
              key={path}
              className={`skill-sidebar-item skill-sidebar-project ${
                activeView.kind === "project" && activeView.path === path ? "active" : ""
              }`}
              title={path}
            >
              <button
                className="skill-sidebar-project-open"
                onClick={() => setActiveView({ kind: "project", path })}
              >
                <span className="skill-sidebar-project-name">{basename}</span>
                <span className="skill-sidebar-badge">{projectSkillCount(snapshot, path)}</span>
              </button>
              <button
                className="skill-sidebar-project-remove"
                title="Stop tracking this project"
                onClick={() => handleRemoveProject(path)}
              >
                <X size={12} />
              </button>
            </div>
          );
        })}

        <button className="skill-sidebar-item skill-sidebar-add-project" onClick={handleAddProject}>
          <FolderPlus size={14} />
          <span>Add project…</span>
        </button>
      </div>

      <div className="skill-sidebar-section">
        <div className="skill-sidebar-section-title">Find</div>
        <button
          className={`skill-sidebar-item ${activeView.kind === "discover" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "discover" })}
        >
          <Search size={14} />
          <span>Discover</span>
        </button>
      </div>

      <div className="skill-sidebar-footer">
        <span className="skill-sidebar-scanned-at">{relativeScanTime(snapshot?.scanned_at)}</span>
        <button
          className="skill-sidebar-refresh"
          onClick={handleRefresh}
          disabled={spinning}
          title="Rescan installed skills"
        >
          <RefreshCw size={13} className={spinning ? "skill-sidebar-refresh-spinning" : ""} />
        </button>
      </div>
    </nav>
  );
}
