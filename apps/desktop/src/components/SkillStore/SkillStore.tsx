// ============================================================================
// SkillStore - Main skill discovery and management view
// ============================================================================

import { useState, useEffect, useCallback, useRef } from "react";
import { Button, Drawer, Tabs, TabsContent, TabsList, TabsTrigger } from "@skill-studio/ui";
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

const TAB_TRIGGER_CLASS =
  "text-body font-medium text-text-tertiary after:bg-accent data-active:text-accent hover:text-text-secondary";

/** Tab strip (Browse/Installed): a bare `TabsList` - each tab's toolbar row and results live in that tab's own `TabsContent`, below. */
function SkillStoreTabList({
  installedCount,
  tabsPadding,
}: {
  installedCount: number;
  tabsPadding: string;
}) {
  return (
    <TabsList variant="line" className={`${tabsPadding} bg-bg-secondary`}>
      <TabsTrigger value="browse" className={TAB_TRIGGER_CLASS}>
        Browse
      </TabsTrigger>
      <TabsTrigger value="installed" className={TAB_TRIGGER_CLASS}>
        Installed ({installedCount})
      </TabsTrigger>
    </TabsList>
  );
}

/**
 * Shown in place of Browse's results when a fetch fails - most commonly the
 * Skill Studio server not running (see `api::SkillsShAccess::Server`'s
 * connection-error message). `min-h` matches `SkillBrowser`'s own
 * loading/empty states so retrying doesn't shift the sheet around it.
 */
function BrowseErrorEmptyState({ error, onRetry }: { error: string; onRetry: () => void }) {
  return (
    <div className="flex min-h-[280px] flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
      <h3 className="m-0 text-body font-semibold text-text-primary">
        Browsing isn't available right now
      </h3>
      <p className="m-0 max-w-sm text-small text-text-tertiary">{error}</p>
      <Button onClick={onRetry}>Try again</Button>
    </div>
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
  // Starts true: the mount effect below fetches immediately, so the first
  // render already reflects that instead of flipping it on synchronously
  // inside the effect (which the compiler flags as an avoidable extra render).
  const [isLoading, setIsLoading] = useState(true);
  const [isLoadingMore, setIsLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  // Set when the results area itself couldn't be loaded (most commonly the
  // Skill Studio server not running) - replaces the results area with
  // `BrowseErrorEmptyState` instead of a toast, since there's nothing to show
  // behind it. Cleared at the start of every new attempt.
  const [browseError, setBrowseError] = useState<string | null>(null);
  // Never read during render - only `loadMore`'s paging math needs it, so a
  // ref avoids a re-render on every page bump.
  const pageRef = useRef(0);

  // A Promise chain, not a try/finally statement, so the compiler can still
  // optimize this component (it doesn't support `finally` clauses yet).
  // Kept memoized (not stripped like the other handlers below): the mount
  // effect further down depends on this callback's identity to run exactly
  // once instead of on every render.
  // react-doctor-disable-next-line react-doctor/react-compiler-no-manual-memoization -- the mount effect below depends on this callback's identity to run exactly once
  const loadInitialData = useCallback(() => {
    pageRef.current = 0;
    setBrowseError(null);
    // Load both in parallel
    return Promise.all([getInstalledSkills(projects), getPopularSkills(0, LIMIT)])
      .then(([installed, popularResponse]) => {
        setInstalledSkills(installed);
        setHasMore(popularResponse.has_more);
        setRawResults(popularResponse.skills);
      })
      .catch((err) => {
        setBrowseError(err instanceof Error ? err.message : "Failed to load skills");
      })
      .finally(() => {
        setIsLoading(false);
      });
  }, [projects]);

  const loadInstalledSkills = async () => {
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
  };

  // Load installed and popular skills on mount.
  useEffect(() => {
    loadInitialData();
  }, [loadInitialData]);

  // Every result merged with the current installed status - recomputed
  // whenever either input changes, so an install/remove refreshing
  // `installedSkills` shows up here with no second effect to keep in sync.
  const searchResultsWithStatus = mergeWithInstalledStatus(rawResults, installedSkills);

  // A Promise chain, not a try/finally statement, so the compiler can still
  // optimize this component (it doesn't support `finally` clauses yet).
  const handleSearch = (query: string) => {
    setSearchQuery(query);
    pageRef.current = 0;
    setBrowseError(null);
    setIsLoading(true);

    if (!query.trim() || query.length < 2) {
      // Show popular skills when no search query
      return getPopularSkills(0, LIMIT)
        .then((response) => {
          setRawResults(response.skills);
          setHasMore(response.has_more);
        })
        .catch((err) => {
          setBrowseError(err instanceof Error ? err.message : "Failed to load popular skills");
        })
        .finally(() => {
          setIsLoading(false);
        });
    }

    // The v1 search endpoint has no pagination - a single call returns
    // everything up to LIMIT.
    return searchSkills(query, LIMIT)
      .then((response) => {
        setRawResults(response.skills);
        setHasMore(false);
      })
      .catch((err) => {
        setBrowseError(err instanceof Error ? err.message : "Failed to search skills");
      })
      .finally(() => {
        setIsLoading(false);
      });
  };

  // Re-runs whichever fetch last populated the results area - the
  // `BrowseErrorEmptyState`'s "Try again" button.
  const retry = () => handleSearch(searchQuery);

  // A Promise chain, not a try/finally statement, so the compiler can still
  // optimize this component (it doesn't support `finally` clauses yet).
  const loadMore = () => {
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
  };

  return {
    searchQuery,
    setSearchQuery,
    installedSkills,
    isLoading,
    isLoadingMore,
    hasMore,
    browseError,
    retry,
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
    browseError,
    retry,
  } = useSkillStoreData(projects, addToast);

  const handleInstallStart = (skillName: string) => {
    setInstallProgress({
      isInstalling: true,
      skillName,
      stage: "starting",
      message: "Starting installation…",
    });
  };

  const handleInstallComplete = (result: {
    success: boolean;
    error?: string;
    skillName?: string;
  }) => {
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
  };

  const handleRemoveComplete = () => {
    addToast({
      type: "success",
      title: "Skill Removed",
      message: "Successfully removed skill",
    });
    // Refresh installed skills - `installedSkillsWithStatus`/`searchResultsWithStatus` re-derive from it.
    loadInstalledSkills();
    setSelectedSkillName(null);
  };

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

      <Tabs
        value={activeTab}
        onValueChange={(tab) => {
          setActiveTab(tab);
          setSelectedSkillName(null);
        }}
        className="flex flex-1 flex-col gap-0 overflow-hidden"
      >
        <SkillStoreTabList installedCount={installedSkills.length} tabsPadding={tabsPadding} />

        <TabsContent value="browse" className="flex flex-1 flex-col overflow-hidden">
          {browseError ? (
            <BrowseErrorEmptyState error={browseError} onRetry={retry} />
          ) : (
            <>
              <div className={`flex items-center gap-4 border-b border-border py-4 ${tabsPadding}`}>
                <SkillSearchBar
                  value={searchQuery}
                  onChange={setSearchQuery}
                  onSearch={handleSearch}
                  isLoading={isLoading}
                />
                <div className="ml-auto flex items-center gap-4">
                  <span className="text-small tabular-nums text-text-tertiary">
                    {searchResultsWithStatus.length} skill
                    {searchResultsWithStatus.length !== 1 ? "s" : ""}
                  </span>
                </div>
              </div>

              <div className={`flex flex-1 overflow-hidden ${compact ? "flex-col" : ""}`}>
                <SkillBrowser
                  skills={searchResultsWithStatus}
                  selectedSkill={selectedSkill}
                  onSelectSkill={(skill) => setSelectedSkillName(skill.name)}
                  mode={{ kind: "browse", isLoading, isLoadingMore, hasMore, onLoadMore: loadMore }}
                  emptyMessage={
                    searchQuery ? "No skills found matching your search" : "Loading popular skills…"
                  }
                />
              </div>
            </>
          )}
        </TabsContent>

        <TabsContent value="installed" className="flex flex-1 flex-col overflow-hidden">
          <div className={`flex items-center gap-4 border-b border-border py-4 ${tabsPadding}`}>
            <span className="text-small tabular-nums text-text-tertiary">
              {installedSkills.length} installed skill{installedSkills.length !== 1 ? "s" : ""}
            </span>
          </div>

          <div className={`flex flex-1 overflow-hidden ${compact ? "flex-col" : ""}`}>
            <SkillBrowser
              skills={installedSkillsWithStatus}
              selectedSkill={selectedSkill}
              onSelectSkill={(skill) => setSelectedSkillName(skill.name)}
              mode={{ kind: "installed" }}
              emptyMessage="No skills installed yet"
            />
          </div>
        </TabsContent>
      </Tabs>

      <Drawer
        open={selectedSkill !== null}
        onOpenChange={(open) => !open && setSelectedSkillName(null)}
      >
        {selectedSkill && (
          <SkillDetailPanel
            // Key by skill name so switching skills with the drawer open
            // remounts the panel (resetting InstallControls' install-form
            // state) instead of reusing the previous skill's stale scope /
            // selected-project selection across skills.
            key={selectedSkill.name}
            skill={selectedSkill}
            onClose={() => setSelectedSkillName(null)}
            onInstallStart={handleInstallStart}
            onInstallComplete={handleInstallComplete}
            onRemoveComplete={handleRemoveComplete}
          />
        )}
      </Drawer>

      {installProgress && (
        <InstallProgressModal progress={installProgress} onClose={() => setInstallProgress(null)} />
      )}
    </div>
  );
}
