// ============================================================================
// useProjectFolderActions - Add, stop tracking, or remove a project folder,
// shared by the Skills filter bar and the Settings "Project folders" card.
// ============================================================================

import { homeDir } from "@tauri-apps/api/path";
import { open } from "@tauri-apps/plugin-dialog";
import { isProjectScope } from "@skill-studio/lib";
import {
  invokeErrorMessage,
  registerSkillProjects,
  removeSkillProject,
  unregisterSkillProject,
} from "../lib/skill-api";
import { useAppStore } from "../store/appStore";

/** Resets the Skills scope to All when it is scoped to `path`, a folder that is no longer tracked.
 * Reads the store directly instead of a render-time value, since this runs after an `await`. */
function resetScopeToAll(path: string): void {
  const { skillListFilter, setSkillListFilter } = useAppStore.getState();
  if (isProjectScope(skillListFilter.scope) && skillListFilter.scope.project === path) {
    setSkillListFilter({ scope: "all" });
  }
}

/**
 * Add/stop-tracking/remove a project folder, shared by the Skills filter
 * bar (`SkillsView`) and the Settings "Project folders" card.
 */
export function useProjectFolderActions() {
  const setTrackedProjects = useAppStore((state) => state.setTrackedProjects);
  const addToast = useAppStore((state) => state.addToast);

  /** Opens a directory picker and registers the pick. Resolves to the added path, or `null` when
   * the picker was cancelled, the home directory was rejected, or registration failed. */
  const addProject = async (): Promise<string | null> => {
    const selected = await open({ directory: true, multiple: false, title: "Add Project" });
    if (!selected) return null;

    const home = await homeDir().catch(() => null);
    if (home && selected === home) {
      addToast({
        type: "error",
        title: "Can't add the home directory",
        message: "It's the global scope, not a project.",
      });
      return null;
    }

    try {
      const projects = await registerSkillProjects([selected]);
      setTrackedProjects(projects);
      return selected;
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't add project",
        message: invokeErrorMessage(err),
      });
      return null;
    }
  };

  /** Saves a path or `*` pattern typed by hand ("Type a path or pattern…"). Resolves to `null` on
   * success (after the same store refresh `addProject` does), or the backend's error message on
   * failure, so the caller can show it inline instead of a toast. */
  const addTypedPath = async (value: string): Promise<string | null> => {
    try {
      const projects = await registerSkillProjects([value]);
      setTrackedProjects(projects);
      return null;
    } catch (err) {
      return invokeErrorMessage(err);
    }
  };

  /**
   * Un-registers `path` (a discovered folder's "Stop tracking") with the
   * backend first; the store (and the scope, if it was the active project)
   * only updates once that succeeds, so a failed unregister leaves tracking
   * state unchanged and reports an error instead of silently un-tracking a
   * project the backend still has.
   */
  const stopTracking = async (path: string) => {
    try {
      setTrackedProjects(await unregisterSkillProject(path));
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't stop tracking project",
        message: invokeErrorMessage(err),
      });
      return;
    }
    resetScopeToAll(path);
  };

  /** Removes `path` (an added folder's "Remove"), recording no exclusion. Same store update and
   * scope reset as `stopTracking`. */
  const removeProject = async (path: string) => {
    try {
      setTrackedProjects(await removeSkillProject(path));
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't remove project",
        message: invokeErrorMessage(err),
      });
      return;
    }
    resetScopeToAll(path);
  };

  return { addProject, addTypedPath, stopTracking, removeProject };
}
