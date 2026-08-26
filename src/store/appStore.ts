// ============================================================================
// Skill Studio - Application State Store
// Toasts, the shell's route state, and the user-added project list
// ============================================================================

import { create } from "zustand";
import type { HealthIssueKind } from "../lib/skill-health";
import { USAGE_WINDOWS } from "../lib/skill-stats";
import type { UsageWindow } from "../lib/skill-stats";
import type { Toast } from "../lib/skill-types";

// ============================================================================
// Route State
// ============================================================================

/**
 * Which view the shell's `<main>` shows. `global` is every own skill
 * deployed at global scope; `project` is one registered project directory;
 * `plugins` is every skill shipped by one harness's plugin cache; `parked`
 * is every parked (disabled globally) skill; `coverage` is the skill x agent
 * deployment matrix; `issues` is every health issue
 * across own skills, optionally pre-filtered to one `issueKind` for
 * deep-linking from the dashboard; `activity` is the full invocation
 * history (year heatmap, per-skill and per-project breakdowns); `skill` is
 * the full-page view of one installed skill, opened from any other view.
 */
export type ActiveView =
  | { kind: "dashboard" }
  | { kind: "global" }
  | { kind: "project"; path: string }
  | { kind: "plugins"; harness: string }
  | { kind: "parked" }
  | { kind: "coverage" }
  | { kind: "issues"; issueKind?: HealthIssueKind }
  | { kind: "activity" }
  | { kind: "discover" }
  | { kind: "packs" }
  | { kind: "skill"; name: string; deploymentPath?: string; from: ActiveView };

// ============================================================================
// State Interface
// ============================================================================

interface AppState {
  // === Toast Notifications ===
  toasts: Toast[];
  addToast: (toast: Omit<Toast, "id">) => string;
  removeToast: (id: string) => void;

  // === Shell Route State ===
  activeView: ActiveView;
  setActiveView: (view: ActiveView) => void;
  /**
   * Opens the skill page for `name` over the current view. Opening a skill
   * from an existing skill page reuses that page's `from`, so the back
   * button never lands on another skill page.
   */
  openSkill: (name: string, deploymentPath?: string) => void;
  /** Returns to the view the current skill page was opened from. */
  closeSkill: () => void;

  // === Project Scope Selection ===
  // Directories the user has pointed at (via a folder picker), for
  // project-scoped skill installs. Registered with the backend on startup
  // and whenever the user adds one.
  userAddedProjects: string[];
  // Directories the user explicitly removed from the Sidebar ("Stop
  // tracking"), including ones the backend discovers on its own (Codex
  // config, Claude Code transcripts). Un-registered with the backend on
  // startup and whenever the user removes one, so they don't reappear just
  // because `project_discovery` still finds them.
  excludedProjects: string[];
  addProject: (path: string) => void;
  removeProject: (path: string) => void;

  // === Usage Window ===
  // The invocation window ("24h" .. "30d") shown in the dashboard's top
  // skills list and the Activity page's "By skill" table. Shared so
  // switching it in one place is reflected in the other.
  usageWindow: UsageWindow;
  setUsageWindow: (window: UsageWindow) => void;

  // === Add-skill Sheet ===
  addSkillSheet: { open: boolean; prefill?: string };
  openAddSkillSheet: (prefill?: string) => void;
  closeAddSkillSheet: () => void;

  // === Multi-select (SkillListTable -> "Create pack") ===
  // Keyed by the row's deployment directory path (`Deployment.path`), not by
  // skill name - a pack member is bundled from one specific deployment, and
  // two rows can share a name (project vs. plugin) but not a path. Cleared
  // whenever the active view changes, so a selection made in Global doesn't
  // linger into Project.
  selectedSkillPaths: Set<string>;
  toggleSkillSelection: (path: string) => void;
  clearSkillSelection: () => void;
  selectSkills: (paths: string[]) => void;
}

// ============================================================================
// Helper Functions
// ============================================================================

function generateToastId(): string {
  return `toast_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
}

/** localStorage key holding the remembered user-added project paths, one absolute path per line. */
const PROJECT_PATHS_STORAGE_KEY = "project-paths";
/** localStorage key holding the remembered excluded project paths, one absolute path per line. */
const EXCLUDED_PROJECT_PATHS_STORAGE_KEY = "excluded-project-paths";
/** localStorage key holding the remembered usage window. */
const USAGE_WINDOW_STORAGE_KEY = "usage-window";
const USAGE_WINDOWS_SET: Set<string> = new Set(USAGE_WINDOWS.map((w) => w.id));

function loadUsageWindow(): UsageWindow {
  try {
    const stored = localStorage.getItem(USAGE_WINDOW_STORAGE_KEY);
    // SAFETY: just checked `stored` is one of the four UsageWindow literals.
    return stored && USAGE_WINDOWS_SET.has(stored) ? (stored as UsageWindow) : "30d";
  } catch {
    return "30d";
  }
}

function loadPathList(key: string): string[] {
  try {
    return (localStorage.getItem(key) ?? "").split("\n").filter(Boolean);
  } catch {
    return [];
  }
}

function savePathList(key: string, paths: string[]): void {
  try {
    localStorage.setItem(key, paths.join("\n"));
  } catch {
    // Storage can be unavailable (quota, private mode); the list is only a convenience.
  }
}

// ============================================================================
// Store Creation
// ============================================================================

export const useAppStore = create<AppState>((set, get) => ({
  toasts: [],

  addToast: (toast) => {
    const id = generateToastId();
    const newToast: Toast = { ...toast, id };

    set((state) => ({
      toasts: [...state.toasts, newToast],
    }));

    // Auto-remove after duration
    if (toast.duration !== 0) {
      setTimeout(() => {
        get().removeToast(id);
      }, toast.duration || 4000);
    }

    return id;
  },

  removeToast: (id) => {
    set((state) => ({
      toasts: state.toasts.filter((t) => t.id !== id),
    }));
  },

  activeView: { kind: "dashboard" },
  setActiveView: (view) => set({ activeView: view, selectedSkillPaths: new Set() }),
  openSkill: (name, deploymentPath) => {
    const current = get().activeView;
    const from = current.kind === "skill" ? current.from : current;
    set({ activeView: { kind: "skill", name, deploymentPath, from } });
  },
  closeSkill: () => {
    const current = get().activeView;
    if (current.kind === "skill") set({ activeView: current.from });
  },

  userAddedProjects: loadPathList(PROJECT_PATHS_STORAGE_KEY),
  excludedProjects: loadPathList(EXCLUDED_PROJECT_PATHS_STORAGE_KEY),

  addProject: (path) => {
    const { userAddedProjects, excludedProjects } = get();
    const updatedAdded = userAddedProjects.includes(path)
      ? userAddedProjects
      : [...userAddedProjects, path];
    const updatedExcluded = excludedProjects.filter((p) => p !== path);
    savePathList(PROJECT_PATHS_STORAGE_KEY, updatedAdded);
    savePathList(EXCLUDED_PROJECT_PATHS_STORAGE_KEY, updatedExcluded);
    set({ userAddedProjects: updatedAdded, excludedProjects: updatedExcluded });
  },

  removeProject: (path) => {
    const { userAddedProjects, excludedProjects } = get();
    const updatedAdded = userAddedProjects.filter((p) => p !== path);
    const updatedExcluded = excludedProjects.includes(path)
      ? excludedProjects
      : [...excludedProjects, path];
    savePathList(PROJECT_PATHS_STORAGE_KEY, updatedAdded);
    savePathList(EXCLUDED_PROJECT_PATHS_STORAGE_KEY, updatedExcluded);
    set({ userAddedProjects: updatedAdded, excludedProjects: updatedExcluded });
  },

  usageWindow: loadUsageWindow(),
  setUsageWindow: (window) => {
    try {
      localStorage.setItem(USAGE_WINDOW_STORAGE_KEY, window);
    } catch {
      // Storage can be unavailable (quota, private mode); the window is only a convenience.
    }
    set({ usageWindow: window });
  },

  addSkillSheet: { open: false },
  openAddSkillSheet: (prefill) => set({ addSkillSheet: { open: true, prefill } }),
  closeAddSkillSheet: () => set({ addSkillSheet: { open: false } }),

  selectedSkillPaths: new Set<string>(),
  toggleSkillSelection: (path) => {
    const next = new Set(get().selectedSkillPaths);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    set({ selectedSkillPaths: next });
  },
  clearSkillSelection: () => set({ selectedSkillPaths: new Set() }),
  selectSkills: (paths) => set({ selectedSkillPaths: new Set(paths) }),
}));

// ============================================================================
// Selectors (for performance optimization)
// ============================================================================

export const selectToasts = (state: AppState) => state.toasts;
export const selectProjects = (state: AppState) => state.userAddedProjects;
export const selectActiveView = (state: AppState) => state.activeView;
export const selectSelectedSkillPaths = (state: AppState) => state.selectedSkillPaths;
