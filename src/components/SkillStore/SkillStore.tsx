// ============================================================================
// SkillStore - Main skill discovery and management view
// ============================================================================

import { useState, useEffect, useCallback } from 'react';
import { SkillSearchBar } from './SkillSearchBar';
import { SkillBrowser } from './SkillBrowser';
import { SkillDetailPanel } from './SkillDetailPanel';
import { InstallProgressModal } from './InstallProgressModal';
import { searchSkills, getInstalledSkills, getPopularSkills } from '../../lib/skillsApi';
import type { SkillSearchResult, InstalledSkill, SkillWithStatus, InstallProgressState } from '../../lib/skillsTypes';
import { useAppStore } from '../../store/appStore';

const LIMIT = 50;

/**
 * Extract GitHub owner/repo from a URL or source string
 * e.g., "https://github.com/getsentry/skills" -> "getsentry/skills"
 * e.g., "getsentry/skills" -> "getsentry/skills"
 */
function extractGitHubRepo(source: string | undefined, sourceUrl: string | undefined): string | undefined {
  // Try source_url first (more reliable)
  if (sourceUrl) {
    const match = sourceUrl.match(/github\.com\/([^/]+\/[^/]+)/);
    if (match) {
      return match[1].replace(/\.git$/, '');
    }
  }
  // Fall back to source if it looks like owner/repo
  if (source && /^[a-zA-Z0-9_-]+\/[a-zA-Z0-9_.-]+$/.test(source)) {
    return source;
  }
  return undefined;
}

export function SkillStore() {
  const [activeTab, setActiveTab] = useState<'browse' | 'installed'>('browse');
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<SkillWithStatus[]>([]);
  const [installedSkills, setInstalledSkills] = useState<InstalledSkill[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isLoadingMore, setIsLoadingMore] = useState(false);
  const [selectedSkill, setSelectedSkill] = useState<SkillWithStatus | null>(null);
  const [installProgress, setInstallProgress] = useState<InstallProgressState | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [offset, setOffset] = useState(0);

  const addToast = useAppStore(state => state.addToast);

  // Load installed and popular skills on mount
  useEffect(() => {
    loadInitialData();
  }, []);

  const loadInitialData = async () => {
    setIsLoading(true);
    setOffset(0);
    try {
      // Load both in parallel
      const [installed, popularResponse] = await Promise.all([
        getInstalledSkills(),
        getPopularSkills(LIMIT, 0),
      ]);

      setInstalledSkills(installed);
      setHasMore(popularResponse.has_more);

      // Show popular skills with installed status merged
      if (!searchQuery) {
        const installedMap = new Map(installed.map(s => [s.name, s]));
        const popularWithStatus: SkillWithStatus[] = popularResponse.skills.map(skill => ({
          ...skill,
          is_installed: installedMap.has(skill.name),
          installed_info: installedMap.get(skill.name),
        }));
        setSearchResults(popularWithStatus);
      }
    } catch (err) {
      console.error('Failed to load initial data:', err);
    } finally {
      setIsLoading(false);
    }
  };

  const loadInstalledSkills = async () => {
    try {
      const installed = await getInstalledSkills();
      setInstalledSkills(installed);
    } catch (err) {
      console.error('Failed to load installed skills:', err);
    }
  };

  // Re-merge searchResults when installedSkills changes (e.g., after install/remove)
  useEffect(() => {
    if (searchResults.length > 0 && installedSkills.length >= 0) {
      const installedMap = new Map(installedSkills.map(s => [s.name, s]));
      setSearchResults(prev => prev.map(skill => ({
        ...skill,
        is_installed: installedMap.has(skill.name),
        installed_info: installedMap.get(skill.name),
      })));
    }
  }, [installedSkills]);

  // Sync selectedSkill with updated searchResults (e.g., after install changes is_installed)
  useEffect(() => {
    if (selectedSkill && searchResults.length > 0) {
      const updated = searchResults.find(s => s.name === selectedSkill.name);
      if (updated && updated.is_installed !== selectedSkill.is_installed) {
        setSelectedSkill(updated);
      }
    }
  }, [searchResults, selectedSkill]);

  const mergeWithInstalledStatus = (
    results: SkillSearchResult[],
    installed: InstalledSkill[]
  ): SkillWithStatus[] => {
    const installedMap = new Map(installed.map(s => [s.name, s]));
    return results.map(skill => ({
      ...skill,
      is_installed: installedMap.has(skill.name),
      installed_info: installedMap.get(skill.name),
    }));
  };

  const handleSearch = useCallback(async (query: string) => {
    setSearchQuery(query);
    setOffset(0);

    if (!query.trim() || query.length < 2) {
      // Show popular skills when no search query
      setIsLoading(true);
      try {
        const response = await getPopularSkills(LIMIT, 0);
        const resultsWithStatus = mergeWithInstalledStatus(response.skills, installedSkills);
        setSearchResults(resultsWithStatus);
        setHasMore(response.has_more);
      } catch (err) {
        console.error('Failed to load popular skills:', err);
      } finally {
        setIsLoading(false);
      }
      return;
    }

    setIsLoading(true);
    try {
      const response = await searchSkills(query, LIMIT, 0);
      const resultsWithStatus = mergeWithInstalledStatus(response.skills, installedSkills);
      setSearchResults(resultsWithStatus);
      setHasMore(response.has_more);
    } catch (err) {
      addToast({
        type: 'error',
        title: 'Search Failed',
        message: err instanceof Error ? err.message : 'Failed to search skills',
      });
    } finally {
      setIsLoading(false);
    }
  }, [installedSkills, addToast]);

  const loadMore = useCallback(async () => {
    if (isLoadingMore || !hasMore) return;

    setIsLoadingMore(true);
    const newOffset = offset + LIMIT;

    try {
      const response = searchQuery.trim().length >= 2
        ? await searchSkills(searchQuery, LIMIT, newOffset)
        : await getPopularSkills(LIMIT, newOffset);

      const newResults = mergeWithInstalledStatus(response.skills, installedSkills);
      setSearchResults(prev => [...prev, ...newResults]);
      setHasMore(response.has_more);
      setOffset(newOffset);
    } catch (err) {
      addToast({
        type: 'error',
        title: 'Load More Failed',
        message: err instanceof Error ? err.message : 'Failed to load more skills',
      });
    } finally {
      setIsLoadingMore(false);
    }
  }, [isLoadingMore, hasMore, offset, searchQuery, installedSkills, addToast]);

  const handleInstallStart = useCallback((skillName: string) => {
    setInstallProgress({
      isInstalling: true,
      skillName,
      stage: 'starting',
      message: 'Starting installation...',
    });
  }, []);

  const handleInstallComplete = useCallback((result: { success: boolean; error?: string; skillName?: string }) => {
    if (result.success) {
      addToast({
        type: 'success',
        title: 'Skill Installed',
        message: `Successfully installed ${result.skillName || 'skill'}`,
      });
      // Refresh installed skills - useEffect will re-merge searchResults
      loadInstalledSkills();
    } else {
      addToast({
        type: 'error',
        title: 'Installation Failed',
        message: result.error || 'Unknown error',
      });
    }
    setInstallProgress(null);
  }, [addToast]);

  const handleRemoveComplete = useCallback(() => {
    addToast({
      type: 'success',
      title: 'Skill Removed',
      message: 'Successfully removed skill',
    });
    // Refresh installed skills - useEffect will re-merge searchResults
    loadInstalledSkills();
    setSelectedSkill(null);
  }, [addToast]);

  // Get installed skills with full status for Installed tab
  const installedSkillsWithStatus: SkillWithStatus[] = installedSkills.map(skill => {
    // Try to find matching skill from search results for extra metadata
    const found = searchResults.find(s => s.name === skill.name);
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

  return (
    <div className="skill-store">
      <div className="skill-store-header">
        <h2>skills.sh</h2>
        <p className="skill-store-subtitle">
          <a href="https://skills.sh" target="_blank" rel="noopener noreferrer">
            Discover and install skills
          </a>
        </p>
      </div>

      {/* Tab Navigation */}
      <div className="skill-store-tabs">
        <button
          className={`skill-store-tab ${activeTab === 'browse' ? 'active' : ''}`}
          onClick={() => { setActiveTab('browse'); setSelectedSkill(null); }}
        >
          Browse
        </button>
        <button
          className={`skill-store-tab ${activeTab === 'installed' ? 'active' : ''}`}
          onClick={() => { setActiveTab('installed'); setSelectedSkill(null); }}
        >
          Installed ({installedSkills.length})
        </button>
      </div>

      {activeTab === 'browse' && (
        <div className="skill-store-toolbar">
          <SkillSearchBar
            value={searchQuery}
            onChange={setSearchQuery}
            onSearch={handleSearch}
            isLoading={isLoading}
          />
          <div className="skill-store-filters">
            <span className="skill-store-count">
              {searchResults.length} skill{searchResults.length !== 1 ? 's' : ''}
            </span>
          </div>
        </div>
      )}

      {activeTab === 'installed' && (
        <div className="skill-store-toolbar">
          <span className="skill-store-count">
            {installedSkills.length} installed skill{installedSkills.length !== 1 ? 's' : ''}
          </span>
        </div>
      )}

      <div className="skill-store-content">
        {activeTab === 'browse' ? (
          <SkillBrowser
            skills={searchResults}
            selectedSkill={selectedSkill}
            onSelectSkill={setSelectedSkill}
            isLoading={isLoading}
            isLoadingMore={isLoadingMore}
            hasMore={hasMore}
            onLoadMore={loadMore}
            emptyMessage={
              searchQuery
                ? 'No skills found matching your search'
                : 'Loading popular skills...'
            }
          />
        ) : (
          <SkillBrowser
            skills={installedSkillsWithStatus}
            selectedSkill={selectedSkill}
            onSelectSkill={setSelectedSkill}
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
            <div
              className="skill-detail-overlay"
              onClick={() => setSelectedSkill(null)}
            />
            <SkillDetailPanel
              skill={selectedSkill}
              onClose={() => setSelectedSkill(null)}
              onInstallStart={handleInstallStart}
              onInstallComplete={handleInstallComplete}
              onRemoveComplete={handleRemoveComplete}
            />
          </>
        )}
      </div>

      {installProgress && (
        <InstallProgressModal
          progress={installProgress}
          onClose={() => setInstallProgress(null)}
        />
      )}
    </div>
  );
}
