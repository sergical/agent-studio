// ============================================================================
// Sidebar - Left-hand navigation: Overview, Skills (Global + projects), Find
// ============================================================================

import { useCallback, useState } from "react";
import { homeDir } from "@tauri-apps/api/path";
import { ask, open } from "@tauri-apps/plugin-dialog";
import {
  Blocks,
  FolderPlus,
  Globe,
  LayoutDashboard,
  LayoutGrid,
  RefreshCw,
  Search,
  X,
} from "lucide-react";
import { registerSkillProjects, unregisterSkillProject } from "../../lib/skill-api";
import { ownSkillsView, pluginHarnessCounts } from "../../lib/skill-plugin-partition";
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

/** Own-skill count deployed at global (or plugin) scope. */
function globalSkillCount(snapshot: SkillSnapshot | undefined): number {
  const own = ownSkillsView(snapshot?.skills ?? []);
  return own.filter((s) => s.deployments.some((d) => d.scope === "global" || d.scope === "plugin"))
    .length;
}

/** Own-skill count deployed to a specific project directory. */
function projectSkillCount(snapshot: SkillSnapshot | undefined, path: string): number {
  const own = ownSkillsView(snapshot?.skills ?? []);
  return own.filter((s) => s.deployments.some((d) => d.project_path === path)).length;
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
  const addToast = useAppStore((state) => state.addToast);

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

    const home = await homeDir().catch(() => null);
    if (home && selected === home) {
      addToast({
        type: "error",
        title: "Can't add the home directory",
        message: "It's the global scope, not a project.",
      });
      return;
    }

    try {
      // Register with the backend too - it drops the home directory from any
      // batch on its own, but a single-path call still gets a clear error.
      await registerSkillProjects([selected]);
      addProject(selected);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't add project",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  }, [addProject, addToast]);

  const handleRemoveProject = useCallback(
    async (path: string) => {
      const confirmed = await ask(
        `Stop tracking ${path}? Skills in it stay on disk; you can add it again with Add project…`,
        { title: "Stop tracking project", kind: "warning" },
      );
      if (!confirmed) return;

      removeProject(path);
      await unregisterSkillProject(path);
      if (activeView.kind === "project" && activeView.path === path) {
        setActiveView({ kind: "dashboard" });
      }
    },
    [removeProject, activeView, setActiveView],
  );

  const spinning = isRescanning && isLoading;
  const pluginHarnesses = [...pluginHarnessCounts(snapshot?.skills ?? [])].sort(([a], [b]) =>
    a.localeCompare(b),
  );

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

      {pluginHarnesses.length > 0 && (
        <div className="skill-sidebar-section">
          <div className="skill-sidebar-section-title">Plugins</div>
          {pluginHarnesses.map(([harness, count]) => (
            <button
              key={harness}
              className={`skill-sidebar-item ${
                activeView.kind === "plugins" && activeView.harness === harness ? "active" : ""
              }`}
              onClick={() => setActiveView({ kind: "plugins", harness })}
            >
              <Blocks size={14} />
              <span>{harness}</span>
              <span className="skill-sidebar-badge">{count}</span>
            </button>
          ))}
        </div>
      )}

      <div className="skill-sidebar-section">
        <button
          className={`skill-sidebar-item ${activeView.kind === "coverage" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "coverage" })}
        >
          <LayoutGrid size={14} />
          <span>Coverage</span>
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
