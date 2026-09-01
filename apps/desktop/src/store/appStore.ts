// ============================================================================
// Skill Studio - Application State Store
// Toasts, the shell's route state, and the user-added project list
// ============================================================================

import { create } from "zustand";
import { defaultSkillListFilter } from "@skill-studio/lib";
import type { SkillListFilter } from "@skill-studio/lib";
import { USAGE_WINDOWS } from "@skill-studio/lib";
import type { UsageWindow } from "@skill-studio/lib";
import type { Toast } from "@skill-studio/lib";
import { addToast } from "../lib/toast";
import {
  loadStoredTheme,
  resolveTheme,
  stampTheme,
  systemPrefersDark,
  THEME_STORAGE_KEY,
  watchSystemTheme,
} from "../lib/theme";
import type { ResolvedTheme, Theme } from "../lib/theme";

// ============================================================================
// Route State
// ============================================================================

/**
 * Which view the shell's `<main>` shows. `home` is what needs doing across
 * every own skill; `skills` is the unified, filterable skill list - scope
 * (global/project/parked), harness, source, and issue all live in the
 * store's `skillListFilter`, not on this view, so places (the sidebar) stay
 * distinct from filters (the list) and opening a skill and coming back
 * never loses them; `activity` is the full invocation history (year
 * heatmap, per-skill and per-project breakdowns); `packs` is the pack list
 * and detail; `skill` is the full-page view of one installed skill, opened
 * from any other view.
 */
/** One of Learn's explainer sections, deep-linkable from Home and elsewhere. */
export type LearnSection = "broken" | "invoke" | "cost" | "unused";

export type ActiveView =
  | { kind: "home" }
  | { kind: "skills" }
  | { kind: "plugins" }
  | { kind: "activity" }
  | { kind: "packs" }
  | { kind: "learn"; section?: LearnSection }
  | {
      kind: "skill";
      name: string;
      deploymentPath?: string;
      from: ActiveView;
      /** Opens a dialog as soon as the page mounts - "compare" opens `SkillCompareDialog`. Cleared once the dialog opens, so re-entering the page doesn't reopen it. */
      intent?: "compare";
    };

// ============================================================================
// State Interface
// ============================================================================

interface AppState {
  // === Toast Notifications ===
  // Toasts render via sonner (see App.tsx's `<Toaster />`); this action is
  // kept on the store so every existing call site
  // (`useAppStore((state) => state.addToast)`) stayed untouched.
  addToast: (toast: Omit<Toast, "id">) => string;

  // === Shell Route State ===
  activeView: ActiveView;
  setActiveView: (view: ActiveView) => void;
  /**
   * Opens the skill page for `name` over the current view. Opening a skill
   * from an existing skill page reuses that page's `from`, so the back
   * button never lands on another skill page.
   */
  openSkill: (name: string, deploymentPath?: string, intent?: "compare") => void;
  /** Returns to the view the current skill page was opened from. */
  closeSkill: () => void;
  /** Clears the current skill view's `intent`, once its one-shot dialog has opened. */
  clearSkillIntent: () => void;

  // === Skills List Filter ===
  // The Skills view's filter bar state, lifted into the store so it survives
  // opening a skill and coming back, and so the sidebar's search input and
  // Home's deep links can drive it directly - see Sidebar.tsx and
  // HomeView.tsx.
  skillListFilter: SkillListFilter;
  setSkillListFilter: (patch: Partial<SkillListFilter>) => void;
  resetSkillListFilter: () => void;
  /** Whether the Skills view shows the coverage matrix instead of the table. */
  showCoverage: boolean;
  setShowCoverage: (show: boolean) => void;
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

  // === Skill Page Assistant Drawer ===
  // Whether the skill page's assistant panel shows as a right-hand overlay
  // drawer - kept here (not local component state) so it survives navigating
  // between skills. Session-only: always starts closed.
  isAssistantOpen: boolean;
  setIsAssistantOpen: (open: boolean) => void;

  // === Theme ===
  theme: Theme;
  /** What's actually painted right now - resolves "system" against the OS, and follows it live. */
  resolvedTheme: ResolvedTheme;
  setTheme: (theme: Theme) => void;

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
  /** Whether the table renders selection checkboxes at all - see SkillListTable's "Select" ghost button. */
  selectionMode: boolean;
  enterSelectionMode: () => void;
  /** Also clears `selectedSkillPaths` - Cancel/Escape should leave nothing selected behind. */
  exitSelectionMode: () => void;
}

// ============================================================================
// Helper Functions
// ============================================================================

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

/** Cleans up the previous `watchSystemTheme` listener - re-set on every `setTheme` call, so only one is ever live. */
let systemThemeCleanup: (() => void) | null = null;

const initialTheme = loadStoredTheme();

// ============================================================================
// Store Creation
// ============================================================================

export const useAppStore = create<AppState>((set, get) => ({
  addToast,

  activeView: { kind: "home" },
  // Leaving the list (a view change or opening a skill) ends selection mode,
  // so a later return to Skills never lands in a half-finished selection.
  setActiveView: (view) =>
    set({ activeView: view, selectedSkillPaths: new Set(), selectionMode: false }),
  openSkill: (name, deploymentPath, intent) => {
    const current = get().activeView;
    const from = current.kind === "skill" ? current.from : current;
    set({
      activeView: { kind: "skill", name, deploymentPath, from, intent },
      selectedSkillPaths: new Set(),
      selectionMode: false,
    });
  },
  closeSkill: () => {
    const current = get().activeView;
    if (current.kind === "skill") set({ activeView: current.from });
  },
  clearSkillIntent: () => {
    const current = get().activeView;
    if (current.kind === "skill" && current.intent !== undefined) {
      set({ activeView: { ...current, intent: undefined } });
    }
  },

  skillListFilter: defaultSkillListFilter(),
  setSkillListFilter: (patch) =>
    set((state) => ({ skillListFilter: { ...state.skillListFilter, ...patch } })),
  resetSkillListFilter: () => set({ skillListFilter: defaultSkillListFilter() }),

  showCoverage: false,
  setShowCoverage: (show) => set({ showCoverage: show }),

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

  isAssistantOpen: false,
  setIsAssistantOpen: (open) => set({ isAssistantOpen: open }),

  theme: initialTheme,
  resolvedTheme: resolveTheme(initialTheme, systemPrefersDark()),
  setTheme: (theme) => {
    try {
      localStorage.setItem(THEME_STORAGE_KEY, theme);
    } catch {
      // Storage can be unavailable (quota, private mode); the choice is only a convenience.
    }
    systemThemeCleanup?.();
    systemThemeCleanup = null;
    const resolved = resolveTheme(theme, systemPrefersDark());
    stampTheme(resolved);
    set({ theme, resolvedTheme: resolved });
    if (theme === "system") {
      systemThemeCleanup = watchSystemTheme((prefersDark) => {
        const nowResolved: ResolvedTheme = prefersDark ? "dark" : "light";
        stampTheme(nowResolved);
        set({ resolvedTheme: nowResolved });
      });
    }
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

  selectionMode: false,
  enterSelectionMode: () => set({ selectionMode: true }),
  exitSelectionMode: () => set({ selectionMode: false, selectedSkillPaths: new Set() }),
}));

// Wires the live OS-follow listener when the stored preference is "system".
// main.tsx's pre-paint stamp already painted the initial frame, so this
// `setTheme` call re-stamps the same value (a no-op) - it exists only to
// start the listener.
useAppStore.getState().setTheme(useAppStore.getState().theme);
