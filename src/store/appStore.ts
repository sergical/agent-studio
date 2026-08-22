// ============================================================================
// Agent Studio - Application State Store
// Toasts, the shell's route state, and the user-added project list
// ============================================================================

import { create } from "zustand";
import type { Toast } from "../lib/skill-types";

// ============================================================================
// Route State
// ============================================================================

/**
 * Which view the shell's `<main>` shows. `global` is every skill deployed at
 * global (or plugin) scope; `project` is one registered project directory.
 */
export type ActiveView =
  | { kind: "dashboard" }
  | { kind: "global" }
  | { kind: "project"; path: string }
  | { kind: "discover" };

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
  selectedSkillName: string | null;
  setSelectedSkillName: (name: string | null) => void;

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
  setActiveView: (view) => set({ activeView: view }),
  selectedSkillName: null,
  setSelectedSkillName: (name) => set({ selectedSkillName: name }),

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
}));

// ============================================================================
// Selectors (for performance optimization)
// ============================================================================

export const selectToasts = (state: AppState) => state.toasts;
export const selectProjects = (state: AppState) => state.userAddedProjects;
export const selectActiveView = (state: AppState) => state.activeView;
