// ============================================================================
// Sidebar - Left-hand navigation: Overview, Skills (Global + projects),
// Review (Issues + Coverage + Activity), Find
// ============================================================================

import { useCallback, useState } from "react";
import { homeDir } from "@tauri-apps/api/path";
import { ask, open } from "@tauri-apps/plugin-dialog";
import {
  Activity as ActivityIcon,
  AlertCircle,
  Folder,
  FolderGit2,
  FolderPlus,
  Globe,
  LayoutDashboard,
  LayoutGrid,
  RefreshCw,
  Search,
  X,
} from "lucide-react";
import { registerSkillProjects, unregisterSkillProject } from "../../lib/skill-api";
import { collectDashboardIssues } from "../../lib/skill-health";
import { ownSkillsView, pluginHarnessCounts } from "../../lib/skill-plugin-partition";
import { useAppStore } from "../../store/appStore";
import type { ActiveView } from "../../store/appStore";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import type { SkillSnapshot } from "../../lib/skill-types";

/**
 * The row a sidebar item should highlight against: a skill page isn't a row
 * of its own, so it anchors to the view it was opened from (Global, a
 * project, Issues, …), which keeps that row active while the page is open.
 */
export function sidebarAnchorView(activeView: ActiveView): ActiveView {
  return activeView.kind === "skill" ? activeView.from : activeView;
}

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
 * registered project, with an "Add project…" action), Review (Issues +
 * Coverage + Activity), and Find (Discover). The footer shows the snapshot's
 * age and a manual rescan button.
 */
export function Sidebar({ snapshot, isLoading, requestRescan }: SidebarProps) {
  const [isRescanning, setIsRescanning] = useState(false);
  const activeView = useAppStore((state) => state.activeView);
  const anchorView = sidebarAnchorView(activeView);
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
      // The removed project might be the active project view, or the view an
      // open skill page anchors back to (its "from") - either way, that view
      // no longer exists, so bounce to the dashboard.
      const anchor = sidebarAnchorView(activeView);
      if (anchor.kind === "project" && anchor.path === path) {
        setActiveView({ kind: "dashboard" });
      }
    },
    [removeProject, activeView, setActiveView],
  );

  const spinning = isRescanning && isLoading;
  const pluginHarnesses = [...pluginHarnessCounts(snapshot?.skills ?? [])].sort(([a], [b]) =>
    a.localeCompare(b),
  );
  const issuesCount = collectDashboardIssues(ownSkillsView(snapshot?.skills ?? [])).length;

  return (
    <nav className="skill-sidebar">
      <div className="skill-sidebar-section">
        <button
          className={`skill-sidebar-item ${anchorView.kind === "dashboard" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "dashboard" })}
        >
          <LayoutDashboard size={15} />
          <span>Dashboard</span>
        </button>
      </div>

      <div className="skill-sidebar-section">
        <div className="section-label">Skills</div>
        <button
          className={`skill-sidebar-item ${anchorView.kind === "global" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "global" })}
        >
          <Globe size={15} />
          <span>Global</span>
          {globalSkillCount(snapshot) > 0 && (
            <span className="skill-sidebar-badge">{globalSkillCount(snapshot)}</span>
          )}
        </button>

        {projects.map((path) => {
          const basename = path.split("/").filter(Boolean).pop() ?? path;
          const count = projectSkillCount(snapshot, path);
          return (
            <div
              key={path}
              className={`skill-sidebar-item skill-sidebar-project ${
                anchorView.kind === "project" && anchorView.path === path ? "active" : ""
              }`}
              title={path}
            >
              <button
                className="skill-sidebar-project-open"
                onClick={() => setActiveView({ kind: "project", path })}
              >
                <Folder size={15} className="skill-sidebar-project-icon" />
                <span className="skill-sidebar-project-name">{basename}</span>
                {count > 0 && <span className="skill-sidebar-badge">{count}</span>}
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
          <FolderPlus size={15} />
          <span>Add project…</span>
        </button>
      </div>

      {pluginHarnesses.length > 0 && (
        <div className="skill-sidebar-section">
          <div className="section-label">Plugins</div>
          {pluginHarnesses.map(([harness, count]) => (
            <button
              key={harness}
              className={`skill-sidebar-item ${
                anchorView.kind === "plugins" && anchorView.harness === harness ? "active" : ""
              }`}
              onClick={() => setActiveView({ kind: "plugins", harness })}
            >
              {harnessIdFromLabel(harness) ? (
                <HarnessIcon harness={harnessIdFromLabel(harness)!} size={15} />
              ) : (
                <FolderGit2 size={15} />
              )}
              <span>{harness}</span>
              {count > 0 && <span className="skill-sidebar-badge">{count}</span>}
            </button>
          ))}
        </div>
      )}

      <div className="skill-sidebar-section">
        <div className="section-label">Review</div>
        <button
          className={`skill-sidebar-item ${anchorView.kind === "issues" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "issues" })}
        >
          <AlertCircle size={15} />
          <span>Issues</span>
          {issuesCount > 0 && <span className="skill-sidebar-badge">{issuesCount}</span>}
        </button>
        <button
          className={`skill-sidebar-item ${anchorView.kind === "coverage" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "coverage" })}
        >
          <LayoutGrid size={15} />
          <span>Coverage</span>
        </button>
        <button
          className={`skill-sidebar-item ${anchorView.kind === "activity" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "activity" })}
        >
          <ActivityIcon size={15} />
          <span>Activity</span>
        </button>
      </div>

      <div className="skill-sidebar-section">
        <div className="section-label">Find</div>
        <button
          className={`skill-sidebar-item ${anchorView.kind === "discover" ? "active" : ""}`}
          onClick={() => setActiveView({ kind: "discover" })}
        >
          <Search size={15} />
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
