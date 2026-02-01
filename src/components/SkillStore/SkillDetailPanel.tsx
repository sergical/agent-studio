// ============================================================================
// SkillDetailPanel - Detail view for a selected skill
// ============================================================================

import { useState, useCallback, useEffect } from 'react';
import { X, Download, Trash2, RefreshCw, ExternalLink, Check, Clock, GitBranch, FileText } from 'lucide-react';
import { AgentTargetSelector } from './AgentTargetSelector';
import { installSkill, removeSkill, updateSkill, getSkillDetails } from '../../lib/skillsApi';
import { useAppStore } from '../../store/appStore';
import type { SkillWithStatus, AgentId, InstallScope } from '../../lib/skillsTypes';
import { COMMON_AGENTS } from '../../lib/skillsTypes';

// Cache for GitHub repo trees - stores SKILL.md paths per repo
const repoTreeCache = new Map<string, { paths: string[]; timestamp: number }>();
const CACHE_TTL = 5 * 60 * 1000; // 5 minutes
const AGENT_PREFS_STORAGE_KEY = 'skill-store-agent-prefs';

/**
 * Find the path to a SKILL.md file in a GitHub repo using the Tree API.
 * This avoids hardcoding repo structures.
 */
async function findSkillPath(owner: string, repo: string, skillName: string): Promise<string | null> {
  const cacheKey = `${owner}/${repo}`;
  const cached = repoTreeCache.get(cacheKey);

  let paths: string[];

  // Check cache first
  if (cached && Date.now() - cached.timestamp < CACHE_TTL) {
    paths = cached.paths;
    console.log('[findSkillPath] Using cached paths for', cacheKey, 'paths:', paths.length);
  } else {
    // Fetch from GitHub Tree API - try both main and master branches
    let fetchedPaths: string[] | null = null;
    for (const branch of ['main', 'master']) {
      try {
        console.log('[findSkillPath] Fetching tree from', `${owner}/${repo}`, 'branch:', branch);
        const response = await fetch(
          `https://api.github.com/repos/${owner}/${repo}/git/trees/${branch}?recursive=1`
        );
        if (!response.ok) {
          console.log('[findSkillPath] Response not ok:', response.status);
          continue;
        }

        const data = await response.json();
        fetchedPaths = data.tree
          .filter((f: { type: string; path: string }) => f.type === 'blob' && f.path.endsWith('SKILL.md'))
          .map((f: { path: string }) => f.path);
        console.log('[findSkillPath] Found SKILL.md paths:', fetchedPaths);
        break;
      } catch (err) {
        console.log('[findSkillPath] Error fetching tree:', err);
        // Continue to next branch
      }
    }
    if (!fetchedPaths) {
      console.log('[findSkillPath] No paths found for', `${owner}/${repo}`);
      return null;
    }
    paths = fetchedPaths;
    repoTreeCache.set(cacheKey, { paths, timestamp: Date.now() });
  }

  console.log('[findSkillPath] Looking for skill:', skillName, 'in paths:', paths);

  // 1. Exact match: vercel-react-best-practices/SKILL.md
  let match = paths.find(p => p.endsWith(`${skillName}/SKILL.md`));
  if (match) {
    console.log('[findSkillPath] Found exact match:', match);
    return match;
  }

  // 2. Strip common prefixes (vercel-, remotion-, getsentry-, anthropic-, openai-)
  const prefixes = ['vercel-', 'remotion-', 'getsentry-', 'anthropic-', 'openai-'];
  for (const prefix of prefixes) {
    if (skillName.startsWith(prefix)) {
      const stripped = skillName.slice(prefix.length);
      console.log('[findSkillPath] Trying stripped name:', stripped);
      match = paths.find(p => p.endsWith(`${stripped}/SKILL.md`));
      if (match) {
        console.log('[findSkillPath] Found match with stripped prefix:', match);
        return match;
      }
    }
  }

  // 3. Partial match - folder name is part of skill name or vice versa
  match = paths.find(p => {
    const parts = p.split('/');
    const folder = parts[parts.length - 2]; // Folder containing SKILL.md
    if (!folder) return false;
    // Check if skill name contains folder or folder contains normalized skill name
    const matches = skillName.includes(folder) || folder.includes(skillName.replace(/-/g, ''));
    if (matches) {
      console.log('[findSkillPath] Partial match found: folder=', folder, 'path=', p);
    }
    return matches;
  });

  console.log('[findSkillPath] Final match result:', match || 'none');
  return match || null;
}

interface SkillDetailPanelProps {
  skill: SkillWithStatus;
  onClose: () => void;
  onInstallStart: (skillName: string) => void;
  onInstallComplete: (result: { success: boolean; error?: string; skillName?: string }) => void;
  onRemoveComplete: () => void;
}

export function SkillDetailPanel({
  skill,
  onClose,
  onInstallStart,
  onInstallComplete,
  onRemoveComplete,
}: SkillDetailPanelProps) {
  const [selectedAgents, setSelectedAgents] = useState<AgentId[]>(() => {
    const saved = localStorage.getItem(AGENT_PREFS_STORAGE_KEY);
    if (saved) {
      try {
        return JSON.parse(saved);
      } catch {
        return COMMON_AGENTS;
      }
    }
    return COMMON_AGENTS;
  });
  const [installScope, setInstallScope] = useState<InstallScope>('global');
  const [isInstalling, setIsInstalling] = useState(false);
  const [isRemoving, setIsRemoving] = useState(false);
  const [isUpdating, setIsUpdating] = useState(false);
  const [showRemoveConfirm, setShowRemoveConfirm] = useState(false);
  const [skillContent, setSkillContent] = useState<string | null>(null);
  const [isLoadingContent, setIsLoadingContent] = useState(false);
  const [resolvedTopSource, setResolvedTopSource] = useState<string | null>(null);
  const [selectedProject, setSelectedProject] = useState<string | null>(null);

  // Get discovered projects from store
  const projects = useAppStore((state) => state.projects);
  const availableProjects = projects.map(p => p.path);

  // Auto-select first project when switching to project scope
  useEffect(() => {
    if (installScope === 'project' && availableProjects.length > 0 && !selectedProject) {
      setSelectedProject(availableProjects[0]);
    }
  }, [installScope, availableProjects, selectedProject]);

  // Persist agent preferences to localStorage
  useEffect(() => {
    localStorage.setItem(AGENT_PREFS_STORAGE_KEY, JSON.stringify(selectedAgents));
  }, [selectedAgents]);

  // Resolve top_source - either from skill prop or by fetching details
  useEffect(() => {
    if (skill.top_source) {
      setResolvedTopSource(skill.top_source);
      return;
    }

    // Try to fetch skill details to get top_source
    const fetchDetails = async () => {
      try {
        const details = await getSkillDetails(skill.name);
        if (details.top_source) {
          setResolvedTopSource(details.top_source);
        } else {
          setResolvedTopSource(null);
        }
      } catch {
        setResolvedTopSource(null);
      }
    };

    fetchDetails();
  }, [skill.name, skill.top_source]);

  // Fetch SKILL.md content - handles both well-known (direct URL) and GitHub sources
  useEffect(() => {
    const fetchContent = async () => {
      setIsLoadingContent(true);
      console.log('[fetchContent] Starting for skill:', skill.name);
      console.log('[fetchContent] resolvedTopSource:', resolvedTopSource);
      console.log('[fetchContent] installed_info:', skill.installed_info);

      try {
        const installedInfo = skill.installed_info;

        // 1. For well-known skills, source_url IS the SKILL.md URL - fetch directly
        if (installedInfo?.source_type === 'well-known' && installedInfo.source_url) {
          console.log('[fetchContent] Trying well-known URL:', installedInfo.source_url);
          try {
            const response = await fetch(installedInfo.source_url);
            if (response.ok) {
              const content = await response.text();
              console.log('[fetchContent] Successfully fetched from well-known URL');
              setSkillContent(content);
              setIsLoadingContent(false);
              return;
            }
          } catch {
            // Fall through to other methods
          }
        }

        // 2. For GitHub skills with skill_path, construct the raw URL
        if (installedInfo?.source_type === 'github' && installedInfo.source_url && installedInfo.skill_path) {
          // source_url is like "https://github.com/vercel-labs/skills.git"
          // skill_path is like "skills/find-skills/SKILL.md"
          const repoUrl = installedInfo.source_url.replace(/\.git$/, '');
          const match = repoUrl.match(/github\.com\/([^/]+\/[^/]+)/);
          console.log('[fetchContent] Trying GitHub with skill_path:', installedInfo.skill_path);
          if (match) {
            const ownerRepo = match[1];
            const branches = ['main', 'master'];
            for (const branch of branches) {
              const url = `https://raw.githubusercontent.com/${ownerRepo}/${branch}/${installedInfo.skill_path}`;
              console.log('[fetchContent] Trying URL:', url);
              try {
                const response = await fetch(url);
                if (response.ok) {
                  const content = await response.text();
                  console.log('[fetchContent] Successfully fetched from GitHub skill_path');
                  setSkillContent(content);
                  setIsLoadingContent(false);
                  return;
                }
              } catch {
                // Continue to next branch
              }
            }
          }
        }

        // 3. Fall back to top_source for browse tab skills (not installed)
        const source = resolvedTopSource;
        console.log('[fetchContent] Falling back to resolvedTopSource:', source);
        if (!source) {
          console.log('[fetchContent] No source available, setting null');
          setSkillContent(null);
          setIsLoadingContent(false);
          return;
        }

        // 4. If source is a full URL, try fetching directly
        if (source.startsWith('http://') || source.startsWith('https://')) {
          console.log('[fetchContent] Source is URL, fetching directly:', source);
          try {
            const response = await fetch(source);
            if (response.ok) {
              const content = await response.text();
              console.log('[fetchContent] Successfully fetched from URL');
              setSkillContent(content);
              setIsLoadingContent(false);
              return;
            }
          } catch {
            // Fall through to GitHub method
          }
        }

        // 5. GitHub owner/repo format - use Tree API to find SKILL.md path
        if (source.includes('/')) {
          const [owner, repo] = source.split('/');
          console.log('[fetchContent] Parsing owner/repo:', owner, repo);
          const skillPath = await findSkillPath(owner, repo, skill.name);
          console.log('[fetchContent] findSkillPath returned:', skillPath);

          if (skillPath) {
            for (const branch of ['main', 'master']) {
              const url = `https://raw.githubusercontent.com/${source}/${branch}/${skillPath}`;
              console.log('[fetchContent] Trying URL:', url);
              try {
                const response = await fetch(url);
                if (response.ok) {
                  const content = await response.text();
                  console.log('[fetchContent] Successfully fetched from GitHub Tree API path');
                  setSkillContent(content);
                  setIsLoadingContent(false);
                  return;
                } else {
                  console.log('[fetchContent] Response not ok:', response.status);
                }
              } catch (err) {
                console.log('[fetchContent] Fetch error:', err);
                // Continue to next branch
              }
            }
          }
        }

        // No SKILL.md found
        console.log('[fetchContent] No SKILL.md found, setting null');
        setSkillContent(null);
      } catch (err) {
        console.log('[fetchContent] Error:', err);
        setSkillContent(null);
      } finally {
        setIsLoadingContent(false);
      }
    };

    fetchContent();
  }, [resolvedTopSource, skill.name, skill.installed_info]);

  const handleInstall = useCallback(async () => {
    if (selectedAgents.length === 0) {
      return;
    }

    // For project scope, require a selected project
    if (installScope === 'project' && !selectedProject) {
      return;
    }

    setIsInstalling(true);
    onInstallStart(skill.name);

    try {
      // Build skill source: repo/skill-name for multi-skill repos, or just the source
      const repoSource = skill.top_source || resolvedTopSource;
      // If we have a repo source and skill name differs from repo, include both
      const skillSource = repoSource
        ? `${repoSource}/${skill.name}`
        : skill.name;

      const result = await installSkill({
        skill_source: skillSource,
        scope: installScope,
        agents: selectedAgents,
        project_path: installScope === 'project' ? selectedProject ?? undefined : undefined,
      });
      onInstallComplete({ success: result.success, error: result.error, skillName: skill.name });
    } catch (err) {
      onInstallComplete({
        success: false,
        error: err instanceof Error ? err.message : 'Unknown error',
        skillName: skill.name,
      });
    } finally {
      setIsInstalling(false);
    }
  }, [skill, resolvedTopSource, selectedAgents, installScope, selectedProject, onInstallStart, onInstallComplete]);

  const handleRemove = useCallback(async () => {
    setIsRemoving(true);
    try {
      const result = await removeSkill(skill.name, installScope === 'global');
      if (result.success) {
        onRemoveComplete();
      }
    } catch (err) {
      console.error('Failed to remove skill:', err);
    } finally {
      setIsRemoving(false);
      setShowRemoveConfirm(false);
    }
  }, [skill.name, installScope, onRemoveComplete]);

  const handleUpdate = useCallback(async () => {
    setIsUpdating(true);
    try {
      const result = await updateSkill(skill.name, installScope === 'global');
      if (result.success) {
        onInstallComplete({ success: true, skillName: skill.name });
      } else {
        onInstallComplete({ success: false, error: result.error, skillName: skill.name });
      }
    } catch (err) {
      onInstallComplete({
        success: false,
        error: err instanceof Error ? err.message : 'Unknown error',
        skillName: skill.name,
      });
    } finally {
      setIsUpdating(false);
    }
  }, [skill.name, installScope, onInstallComplete]);

  const formatDate = (dateStr: string): string => {
    try {
      return new Date(dateStr).toLocaleDateString();
    } catch {
      return dateStr;
    }
  };

  return (
    <div className="skill-detail-panel">
      <div className="skill-detail-header">
        <div className="skill-detail-title">
          <h3>{skill.name}</h3>
          {skill.is_installed && (
            <span className="skill-detail-badge installed">
              <Check size={12} />
              Installed
            </span>
          )}
          {skill.installed_info?.has_update && (
            <span className="skill-detail-badge update">
              Update available
            </span>
          )}
        </div>
        <button className="skill-detail-close" onClick={onClose}>
          <X size={18} />
        </button>
      </div>

      <div className="skill-detail-meta">
        {/* Source display - handle both GitHub (owner/repo) and well-known (URL) sources */}
        {(() => {
          // For installed well-known skills, show the domain from source_url
          if (skill.installed_info?.source_url && skill.installed_info.source_type === 'well-known') {
            try {
              const url = new URL(skill.installed_info.source_url);
              return (
                <div className="skill-detail-source">
                  <GitBranch size={14} />
                  <a
                    href={skill.installed_info.source_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="skill-detail-link"
                  >
                    {url.hostname}
                    <ExternalLink size={12} />
                  </a>
                </div>
              );
            } catch {
              // Fall through to regular source display
            }
          }

          // GitHub owner/repo format
          const source = skill.top_source || resolvedTopSource;
          if (source) {
            return (
              <div className="skill-detail-source">
                <GitBranch size={14} />
                <a
                  href={`https://github.com/${source}`}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="skill-detail-link"
                >
                  {source}
                  <ExternalLink size={12} />
                </a>
              </div>
            );
          }

          return null;
        })()}
        <div className="skill-detail-stats">
          <div className="skill-detail-stat">
            <Download size={14} />
            <span>{skill.installs.toLocaleString()}</span>
          </div>
          {skill.installed_info && (
            <div className="skill-detail-stat">
              <Clock size={14} />
              <span>{formatDate(skill.installed_info.installed_at)}</span>
            </div>
          )}
        </div>
      </div>

      {skill.tags && skill.tags.length > 0 && (
        <div className="skill-detail-tags">
          {skill.tags.map(tag => (
            <span key={tag} className="skill-detail-tag">{tag}</span>
          ))}
        </div>
      )}

      {/* Skill Content Section */}
      <div className="skill-detail-content-section">
        <div className="skill-detail-content-header">
          <FileText size={14} />
          <span>SKILL.md</span>
        </div>
        <div className="skill-detail-content">
          {isLoadingContent ? (
            <div className="skill-detail-content-loading">
              <span className="spinner" />
              Loading content...
            </div>
          ) : skillContent ? (
            <pre className="skill-detail-content-text">{skillContent}</pre>
          ) : skill.description ? (
            <p className="skill-detail-content-fallback">{skill.description}</p>
          ) : (
            <p className="skill-detail-content-empty">No content available</p>
          )}
        </div>
      </div>

      <div className="skill-detail-divider" />

      {!skill.is_installed ? (
        <>
          <div className="skill-detail-section">
            <h4>Install scope</h4>
            <div className="skill-detail-scope-toggle">
              <button
                type="button"
                className={`scope-option ${installScope === 'global' ? 'selected' : ''}`}
                onClick={() => setInstallScope('global')}
              >
                Global
              </button>
              <button
                type="button"
                className={`scope-option ${installScope === 'project' ? 'selected' : ''}`}
                onClick={() => setInstallScope('project')}
              >
                Project
              </button>
            </div>

            {/* Project selector dropdown when project scope is selected */}
            {installScope === 'project' && availableProjects.length > 0 && (
              <div className="skill-detail-project-select">
                <label>Project directory</label>
                <select
                  value={selectedProject || ''}
                  onChange={(e) => setSelectedProject(e.target.value)}
                >
                  {availableProjects.map(p => (
                    <option key={p} value={p}>
                      {p.split('/').pop()} - {p}
                    </option>
                  ))}
                </select>
              </div>
            )}

            {/* Warning if no projects discovered */}
            {installScope === 'project' && availableProjects.length === 0 && (
              <div className="skill-detail-no-projects">
                No projects discovered. Run discovery first or switch to Global scope.
              </div>
            )}
          </div>

          <div className="skill-detail-section">
            <AgentTargetSelector
              selectedAgents={selectedAgents}
              onChange={setSelectedAgents}
              disabled={isInstalling}
            />
          </div>

          <div className="skill-detail-actions">
            <button
              className="skill-action-button primary"
              onClick={handleInstall}
              disabled={
                isInstalling ||
                selectedAgents.length === 0 ||
                (installScope === 'project' && !selectedProject)
              }
            >
              {isInstalling ? (
                <>
                  <span className="spinner" />
                  Installing...
                </>
              ) : (
                <>
                  <Download size={16} />
                  Install Skill
                </>
              )}
            </button>
          </div>
        </>
      ) : (
        <div className="skill-detail-actions">
          {skill.installed_info?.has_update && (
            <button
              className="skill-action-button primary"
              onClick={handleUpdate}
              disabled={isUpdating}
            >
              {isUpdating ? (
                <>
                  <span className="spinner" />
                  Updating...
                </>
              ) : (
                <>
                  <RefreshCw size={16} />
                  Update Skill
                </>
              )}
            </button>
          )}
          {showRemoveConfirm ? (
            <div style={{ display: 'flex', gap: '8px' }}>
              <button
                className="skill-action-button danger"
                onClick={handleRemove}
                disabled={isRemoving}
                style={{ flex: 1 }}
              >
                {isRemoving ? (
                  <>
                    <span className="spinner" />
                    Removing...
                  </>
                ) : (
                  'Confirm Remove'
                )}
              </button>
              <button
                className="skill-action-button"
                onClick={() => setShowRemoveConfirm(false)}
                disabled={isRemoving}
                style={{ flex: 1, background: 'var(--color-bg-tertiary)', color: 'var(--color-text-secondary)' }}
              >
                Cancel
              </button>
            </div>
          ) : (
            <button
              className="skill-action-button danger"
              onClick={() => setShowRemoveConfirm(true)}
              disabled={isRemoving}
            >
              <Trash2 size={16} />
              Remove Skill
            </button>
          )}
        </div>
      )}
    </div>
  );
}
