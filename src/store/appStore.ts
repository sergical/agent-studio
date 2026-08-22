// ============================================================================
// Agent Studio - Application State Store
// Skills-only state: toasts and the project list used for scope selection
// ============================================================================

import { create } from "zustand";
import type { Toast } from "../lib/skill-types";

// ============================================================================
// State Interface
// ============================================================================

interface AppState {
  // === Toast Notifications ===
  toasts: Toast[];
  addToast: (toast: Omit<Toast, "id">) => string;
  removeToast: (id: string) => void;

  // === Project Scope Selection ===
  // Directories the user has pointed at for project-scoped skill installs.
  // Populated manually (via a folder picker) since there is no longer a
  // generic project-discovery scan in this app.
  projects: string[];
  addProject: (path: string) => void;
  removeProject: (path: string) => void;
}

// ============================================================================
// Helper Functions
// ============================================================================

function generateToastId(): string {
  return `toast_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
}

/** localStorage key holding the remembered project paths, one absolute path per line. */
const PROJECT_PATHS_STORAGE_KEY = "project-paths";

function loadProjects(): string[] {
  try {
    return (localStorage.getItem(PROJECT_PATHS_STORAGE_KEY) ?? "").split("\n").filter(Boolean);
  } catch {
    return [];
  }
}

function saveProjects(projects: string[]): void {
  try {
    localStorage.setItem(PROJECT_PATHS_STORAGE_KEY, projects.join("\n"));
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

  projects: loadProjects(),

  addProject: (path) => {
    const { projects } = get();
    if (projects.includes(path)) return;
    const updated = [...projects, path];
    saveProjects(updated);
    set({ projects: updated });
  },

  removeProject: (path) => {
    const updated = get().projects.filter((p) => p !== path);
    saveProjects(updated);
    set({ projects: updated });
  },
}));

// ============================================================================
// Selectors (for performance optimization)
// ============================================================================

export const selectToasts = (state: AppState) => state.toasts;
export const selectProjects = (state: AppState) => state.projects;
