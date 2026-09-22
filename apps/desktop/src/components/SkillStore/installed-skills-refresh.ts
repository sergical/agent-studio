// ============================================================================
// Skill Studio - installed-skills-refresh
// Re-reads the installed-skill list when the user's project set changes,
// without touching the Browse tab's popular/search state.
// ============================================================================

import type { InstalledSkill } from "@skill-studio/lib";

/**
 * Reads the installed-skill list for a set of project paths. A thin seam over
 * `getInstalledSkills` so tests can inject a faithful fake instead of mocking
 * the Tauri IPC module.
 */
export type InstalledSkillsFetcher = (projectPaths: string[]) => Promise<InstalledSkill[]>;

export interface InstalledSkillsRefreshHandlers {
  onInstalled: (installed: InstalledSkill[]) => void;
  onError: (message: string) => void;
}

/**
 * Start an installed-skills re-read for `projects`, forwarding the result to
 * `onInstalled` or an error message to `onError`. Returns a cancel function:
 * invoking it before the fetch resolves suppresses both callbacks, so a
 * stale `projects` value superseded by a newer one (or an unmount) can't
 * overwrite the installed list with out-of-date data.
 *
 * The narrow signature - `projects`, a fetcher, and result/error callbacks -
 * is the whole point when refreshing on a `projects` change: this refreshes
 * the installed list ONLY. It holds no handle to the Browse tab's
 * `rawResults`, `hasMore`, `pageRef`, or `searchQuery`, so by construction it
 * cannot clobber an active search's committed results. That is the fix for
 * the c0078ad regression in which `loadInitialData` re-ran on every `projects`
 * change and unconditionally overwrote `rawResults` with popular skills.
 */
export function startInstalledSkillsRefresh(
  projects: string[],
  fetchInstalledSkills: InstalledSkillsFetcher,
  handlers: InstalledSkillsRefreshHandlers,
): () => void {
  let cancelled = false;
  void fetchInstalledSkills(projects)
    .then((installed) => {
      if (cancelled) return;
      handlers.onInstalled(installed);
    })
    .catch((error) => {
      if (cancelled) return;
      handlers.onError(error instanceof Error ? error.message : "Failed to load installed skills");
    });
  return () => {
    cancelled = true;
  };
}
