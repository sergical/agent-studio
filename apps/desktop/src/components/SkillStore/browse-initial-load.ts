// ============================================================================
// Skill Studio - Browse initial load
// The installed + popular reload behind the Browse tab, and what it may replace
// ============================================================================

import type { InstalledSkill, PaginatedSkillsResponse } from "@skill-studio/lib";

/** True when `query` runs a skills.sh search; a blank or one-character query shows popular skills instead. */
export function isSearchQuery(query: string): boolean {
  return query.trim() !== "" && query.length >= 2;
}

interface BrowseInitialLoadSources {
  getInstalled: () => Promise<InstalledSkill[]>;
  getPopular: () => Promise<PaginatedSkillsResponse>;
}

interface BrowseInitialLoad {
  installed: InstalledSkill[];
  /** `null` when a search is active once the reload lands: the results area belongs to that search. */
  popular: PaginatedSkillsResponse | null;
}

/**
 * Loads installed and popular skills in parallel. The reload also runs when the tracked project
 * list changes, for example after a project folder is added in the install drawer, so it can land
 * while the search box holds a query. `activeQuery` is read after both requests finish, which also
 * covers a search typed while the reload was in flight.
 */
export function loadBrowseInitialData(
  sources: BrowseInitialLoadSources,
  activeQuery: () => string,
): Promise<BrowseInitialLoad> {
  return Promise.all([sources.getInstalled(), sources.getPopular()]).then(
    ([installed, popular]) => ({
      installed,
      popular: isSearchQuery(activeQuery()) ? null : popular,
    }),
  );
}
