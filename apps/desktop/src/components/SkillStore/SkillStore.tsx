// ============================================================================
// SkillStore - Main skill discovery and management view
// ============================================================================

import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { SkillSearchBar } from "./SkillSearchBar";
import { SkillBrowser } from "./SkillBrowser";
import { SkillDetailPanel } from "./SkillDetailPanel";
import { InstallProgressModal } from "./InstallProgressModal";
import { searchSkills, getInstalledSkills, getPopularSkills } from "../../lib/skill-api";
import type {
  SkillSearchResult,
  InstalledSkill,
  SkillWithStatus,
  InstallProgressState,
} from "@skill-studio/lib";
import { useAppStore } from "../../store/appStore";

const LIMIT = 50;

/**
 * Extract GitHub owner/repo from a URL or source string
 * e.g., "https://github.com/getsentry/skills" -> "getsentry/skills"
 * e.g., "getsentry/skills" -> "getsentry/skills"
 */
function extractGitHubRepo(
  source: string | undefined,
  sourceUrl: string | undefined,
): string | undefined {
  // Try source_url first (more reliable)
  if (sourceUrl) {
    const match = sourceUrl.match(/github\.com\/([^/]+\/[^/]+)/);
    if (match) {
      return match[1].replace(/\.git$/, "");
    }
  }
  // Fall back to source if it looks like owner/repo
  if (source && /^[a-zA-Z0-9_-]+\/[a-zA-Z0-9_.-]+$/.test(source)) {
    return source;
  }
  return undefined;
}

/** Merges installed status into a page of search results - a module-level pure function of its own arguments, so callers can `useMemo` it without a hook dependency. */
function mergeWithInstalledStatus(
  results: SkillSearchResult[],
  installed: InstalledSkill[],
): SkillWithStatus[] {
  const installedMap = new Map(installed.map((s) => [s.name, s]));
  return results.map((skill) => ({
    ...skill,
    is_installed: installedMap.has(skill.name),
    installed_info: installedMap.get(skill.name),
  }));
}

/** Tab strip (Browse/Installed) plus each tab's toolbar row: the search bar and result count for Browse, just a count for Installed. */
function SkillStoreTabs({
  activeTab,
  onTabChange,
  installedCount,
  tabsPadding,
  searchQuery,
  onSearchQueryChange,
  onSearch,
  isSearching,
  resultCount,
}: {
  activeTab: "browse" | "installed";
  onTabChange: (tab: "browse" | "installed") => void;
  installedCount: number;
  tabsPadding: string;
  searchQuery: string;
  onSearchQueryChange: (value: string) => void;
  onSearch: (query: string) => void;
  isSearching: boolean;
  resultCount: number;
}) {
  return (
    <>
      <div className={`flex gap-0 border-b border-border bg-bg-secondary ${tabsPadding}`}>
        <button
          className={`-mb-px border-0 border-b-2 bg-transparent px-5 py-3 text-body font-medium transition-colors ${
            activeTab === "browse"
              ? "border-accent text-accent"
              : "border-transparent text-text-tertiary hover:text-text-secondary"
          }`}
          onClick={() => onTabChange("browse")}
        >
          Browse
        </button>
        <button
          className={`-mb-px border-0 border-b-2 bg-transparent px-5 py-3 text-body font-medium transition-colors ${
            activeTab === "installed"
              ? "border-accent text-accent"
              : "border-transparent text-text-tertiary hover:text-text-secondary"
          }`}
          onClick={() => onTabChange("installed")}
        >
          Installed ({installedCount})
        </button>
      </div>

      {activeTab === "browse" && (
        <div className={`flex items-center gap-4 border-b border-border py-4 ${tabsPadding}`}>
          <SkillSearchBar
            value={searchQuery}
            onChange={onSearchQueryChange}
            onSearch={onSearch}
            isLoading={isSearching}
          />
          <div className="ml-auto flex items-center gap-4">
            <span className="text-small tabular-nums text-text-tertiary">
              {resultCount} skill{resultCount !== 1 ? "s" : ""}
            </span>
          </div>
        </div>
      )}

      {activeTab === "installed" && (
        <div className={`flex items-center gap-4 border-b border-border py-4 ${tabsPadding}`}>
          <span className="text-small tabular-nums text-text-tertiary">
            {installedCount} installed skill{installedCount !== 1 ? "s" : ""}
          </span>
        </div>
      )}
    </>
  );
}

/**
 * Owns the Browse tab's data pipeline: the popular-skills/search fetch, its
 * loading flags and pagination, and the installed-status merge every result
 * gets rendered with. `loadInstalledSkills` is also called from outside the
 * hook (after an install/remove completes), so it's returned alongside the
 * rest rather than kept as a private effect dependency.
 */
function useSkillStoreData(
  projects: string[],
  addToast: ReturnType<typeof useAppStore.getState>["addToast"],
) {
  const [searchQuery, setSearchQuery] = useState("");
  // Raw API results, with no installed-status merged in - `searchResultsWithStatus`
  // below derives that at render time, so an `installedSkills` refresh (after
  // install/remove) can't leave this stale behind a second effect.
  const [rawResults, setRawResults] = useState<SkillSearchResult[]>([]);
  const [installedSkills, setInstalledSkills] = useState<InstalledSkill[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isLoadingMore, setIsLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  // Never read during render - only `loadMore`'s paging math needs it, so a
  // ref avoids a re-render on every page bump.
  const pageRef = useRef(0);

  // A Promise chain, not a try/finally statement, so the compiler can still
  // optimize this component (it doesn't support `finally` clauses yet).
  const loadInitialData = useCallback(() => {
    setIsLoading(true);
    pageRef.current = 0;
    // Load both in parallel
    return Promise.all([getInstalledSkills(projects), getPopularSkills(0, LIMIT)])
      .then(([installed, popularResponse]) => {
        setInstalledSkills(installed);
        setHasMore(popularResponse.has_more);
        if (!searchQuery) setRawResults(popularResponse.skills);
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Failed to Load Skills",
          message: err instanceof Error ? err.message : "Failed to load initial data",
        });
      })
      .finally(() => {
        setIsLoading(false);
      });
  }, [projects, searchQuery, addToast]);

  const loadInstalledSkills = useCallback(async () => {
    try {
      const installed = await getInstalledSkills(projects);
      setInstalledSkills(installed);
    } catch (err) {
      addToast({
        type: "error",
        title: "Failed to Load Installed Skills",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  }, [projects, addToast]);

  // Load installed and popular skills on mount
  useEffect(() => {
    loadInitialData();
  }, [loadInitialData]);

  // Every result merged with the current installed status - recomputed
  // whenever either input changes, so an install/remove refreshing
  // `installedSkills` shows up here with no second effect to keep in sync.
  const searchResultsWithStatus = useMemo(
    () => mergeWithInstalledStatus(rawResults, installedSkills),
    [rawResults, installedSkills],
  );

  // A Promise chain, not a try/finally statement, so the compiler can still
  // optimize this component (it doesn't support `finally` clauses yet).
  const handleSearch = useCallback(
    (query: string) => {
      setSearchQuery(query);
      pageRef.current = 0;

      if (!query.trim() || query.length < 2) {
        // Show popular skills when no search query
        setIsLoading(true);
        return getPopularSkills(0, LIMIT)
          .then((response) => {
            setRawResults(response.skills);
            setHasMore(response.has_more);
          })
          .catch((err) => {
            addToast({
              type: "error",
              title: "Failed to Load Skills",
              message: err instanceof Error ? err.message : "Failed to load popular skills",
            });
          })
          .finally(() => {
            setIsLoading(false);
          });
      }

      setIsLoading(true);
      // The v1 search endpoint has no pagination - a single call returns
      // everything up to LIMIT.
      return searchSkills(query, LIMIT)
        .then((response) => {
          setRawResults(response.skills);
          setHasMore(false);
        })
        .catch((err) => {
          addToast({
            type: "error",
            title: "Search Failed",
            message: err instanceof Error ? err.message : "Failed to search skills",
          });
        })
        .finally(() => {
          setIsLoading(false);
        });
    },
    [addToast],
  );

  // A Promise chain, not a try/finally statement, so the compiler can still
  // optimize this component (it doesn't support `finally` clauses yet).
  const loadMore = useCallback(() => {
    // hasMore is always false while a search is active (the v1 search API
    // returns everything up to LIMIT in one shot), so this only ever pages
    // through popular skills.
    if (isLoadingMore || !hasMore) return;

    setIsLoadingMore(true);
    const nextPage = pageRef.current + 1;

    return getPopularSkills(nextPage, LIMIT)
      .then((response) => {
        setRawResults((prev) => [...prev, ...response.skills]);
        setHasMore(response.has_more);
        pageRef.current = nextPage;
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Load More Failed",
          message: err instanceof Error ? err.message : "Failed to load more skills",
        });
      })
      .finally(() => {
        setIsLoadingMore(false);
      });
  }, [isLoadingMore, hasMore, addToast]);

  return {
    searchQuery,
    setSearchQuery,
    installedSkills,
    isLoading,
    isLoadingMore,
    hasMore,
    searchResultsWithStatus,
    handleSearch,
    loadMore,
    loadInstalledSkills,
  };
}

interface SkillStoreProps {
  /** Embedded inside `AddSkillSheet`'s "Browse skills.sh" tab: drops this
   * view's own header, since the sheet already has one. */
  compact?: boolean;
}

export function SkillStore({ compact = false }: SkillStoreProps = {}) {
  const [activeTab, setActiveTab] = useState<"browse" | "installed">("browse");
  // The selected skill's name only - its full `SkillWithStatus` is looked up
  // from whichever list (browse or installed) is current each render, so a
  // status change (e.g. install completing) is reflected without a sync effect.
  const [selectedSkillName, setSelectedSkillName] = useState<string | null>(null);
  const [installProgress, setInstallProgress] = useState<InstallProgressState | null>(null);

  const addToast = useAppStore((state) => state.addToast);
  const projects = useAppStore((state) => state.userAddedProjects);

  const {
    searchQuery,
    setSearchQuery,
    installedSkills,
    isLoading,
    isLoadingMore,
    hasMore,
    searchResultsWithStatus,
    handleSearch,
    loadMore,
    loadInstalledSkills,
  } = useSkillStoreData(projects, addToast);

  const handleInstallStart = useCallback((skillName: string) => {
    setInstallProgress({
      isInstalling: true,
      skillName,
      stage: "starting",
      message: "Starting installation…",
    });
  }, []);

  const handleInstallComplete = useCallback(
    (result: { success: boolean; error?: string; skillName?: string }) => {
      if (result.success) {
        addToast({
          type: "success",
          title: "Skill Installed",
          message: `Successfully installed ${result.skillName || "skill"}`,
        });
        // Refresh installed skills - `searchResultsWithStatus` re-derives from it.
        loadInstalledSkills();
      } else {
        addToast({
          type: "error",
          title: "Installation Failed",
          message: result.error || "Unknown error",
        });
      }
      setInstallProgress(null);
    },
    [addToast, loadInstalledSkills],
  );

  const handleRemoveComplete = useCallback(() => {
    addToast({
      type: "success",
      title: "Skill Removed",
      message: "Successfully removed skill",
    });
    // Refresh installed skills - `installedSkillsWithStatus`/`searchResultsWithStatus` re-derive from it.
    loadInstalledSkills();
    setSelectedSkillName(null);
  }, [addToast, loadInstalledSkills]);

  // Get installed skills with full status for Installed tab
  const installedSkillsWithStatus: SkillWithStatus[] = installedSkills.map((skill) => {
    // Try to find matching skill from search results for extra metadata
    const found = searchResultsWithStatus.find((s) => s.name === skill.name);
    if (found) return found;

    // Extract GitHub owner/repo from source_url (more reliable) or source
    const topSource = extractGitHubRepo(skill.source, skill.source_url);

    return {
      id: skill.name,
      name: skill.name,
      installs: 0,
      is_installed: true,
      installed_info: skill,
      top_source: topSource,
    };
  });

  // Looked up by name from whichever list is current, so a status change
  // (e.g. install completing) shows up with no sync effect.
  const selectedSkill: SkillWithStatus | null = selectedSkillName
    ? (searchResultsWithStatus.find((s) => s.name === selectedSkillName) ??
      installedSkillsWithStatus.find((s) => s.name === selectedSkillName) ??
      null)
    : null;

  const tabsPadding = compact ? "px-5" : "px-7";

  return (
    <div className="flex flex-1 flex-col overflow-hidden bg-bg-primary">
      {!compact && (
        <div className="border-b border-border px-7 pt-6 pb-4">
          <h2 className="m-0 mb-1 text-pretty text-balance text-title font-semibold text-text-primary">
            skills.sh
          </h2>
          <p className="m-0 text-pretty text-body text-text-tertiary">
            <a
              href="https://skills.sh"
              target="_blank"
              rel="noopener noreferrer"
              className="text-text-tertiary no-underline transition-colors hover:text-accent hover:underline"
            >
              Discover and install skills
            </a>
          </p>
        </div>
      )}

      <SkillStoreTabs
        activeTab={activeTab}
        onTabChange={(tab) => {
          setActiveTab(tab);
          setSelectedSkillName(null);
        }}
        installedCount={installedSkills.length}
        tabsPadding={tabsPadding}
        searchQuery={searchQuery}
        onSearchQueryChange={setSearchQuery}
        onSearch={handleSearch}
        isSearching={isLoading}
        resultCount={searchResultsWithStatus.length}
      />

      <div className={`flex flex-1 overflow-hidden ${compact ? "flex-col" : ""}`}>
        {activeTab === "browse" ? (
          <SkillBrowser
            skills={searchResultsWithStatus}
            selectedSkill={selectedSkill}
            onSelectSkill={(skill) => setSelectedSkillName(skill.name)}
            isLoading={isLoading}
            isLoadingMore={isLoadingMore}
            hasMore={hasMore}
            onLoadMore={loadMore}
            emptyMessage={
              searchQuery ? "No skills found matching your search" : "Loading popular skills…"
            }
          />
        ) : (
          <SkillBrowser
            skills={installedSkillsWithStatus}
            selectedSkill={selectedSkill}
            onSelectSkill={(skill) => setSelectedSkillName(skill.name)}
            isLoading={false}
            isLoadingMore={false}
            hasMore={false}
            onLoadMore={() => {}}
            emptyMessage="No skills installed yet"
            hideInstalledIndicator
          />
        )}

        {selectedSkill && (
          <>
            {/* Decorative scrim: a pointer affordance duplicating the Escape
                path in SkillDetailPanel, so it's aria-hidden rather than a
                focusable control. */}
            <div
              className="fixed inset-0 z-(--z-backdrop) bg-scrim"
              aria-hidden="true"
              onClick={() => setSelectedSkillName(null)}
            />
            <SkillDetailPanel
              skill={selectedSkill}
              onClose={() => setSelectedSkillName(null)}
              onInstallStart={handleInstallStart}
              onInstallComplete={handleInstallComplete}
              onRemoveComplete={handleRemoveComplete}
            />
          </>
        )}
      </div>

      {installProgress && (
        <InstallProgressModal progress={installProgress} onClose={() => setInstallProgress(null)} />
      )}
    </div>
  );
}
