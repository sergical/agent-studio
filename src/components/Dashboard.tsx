// ============================================================================
// Dashboard - Overview of all Claude Code entities
// ============================================================================

import type { ReactNode } from 'react';
import { useState } from 'react';
import { useAppStore } from '../store/appStore';
import { openInFinder } from '../lib/api';
import type { EntityType, ViewType } from '../lib/types';

interface EntityConfig {
  type: EntityType;
  view: ViewType;
  label: string;
  icon: ReactNode;
  color: string;
}

const ENTITY_CONFIG: EntityConfig[] = [
  {
    type: 'settings',
    view: 'settings',
    label: 'Settings',
    color: '#8b5cf6',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <path d="M12 15a3 3 0 100-6 3 3 0 000 6z" />
        <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-2 2 2 2 0 01-2-2v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83 0 2 2 0 010-2.83l.06-.06a1.65 1.65 0 00.33-1.82 1.65 1.65 0 00-1.51-1H3a2 2 0 01-2-2 2 2 0 012-2h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 010-2.83 2 2 0 012.83 0l.06.06a1.65 1.65 0 001.82.33H9a1.65 1.65 0 001-1.51V3a2 2 0 012-2 2 2 0 012 2v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 0 2 2 0 010 2.83l-.06.06a1.65 1.65 0 00-.33 1.82V9a1.65 1.65 0 001.51 1H21a2 2 0 012 2 2 2 0 01-2 2h-.09a1.65 1.65 0 00-1.51 1z" />
      </svg>
    ),
  },
  {
    type: 'memory',
    view: 'memory',
    label: 'Memory',
    color: '#06b6d4',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z" />
        <polyline points="14,2 14,8 20,8" />
        <line x1="16" y1="13" x2="8" y2="13" />
        <line x1="16" y1="17" x2="8" y2="17" />
      </svg>
    ),
  },
  {
    type: 'agent',
    view: 'agents',
    label: 'Agents',
    color: '#f59e0b',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <circle cx="12" cy="8" r="4" />
        <path d="M20 21a8 8 0 10-16 0" />
      </svg>
    ),
  },
  {
    type: 'skill',
    view: 'skills',
    label: 'Skills',
    color: '#10b981',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <polygon points="12,2 15.09,8.26 22,9.27 17,14.14 18.18,21.02 12,17.77 5.82,21.02 7,14.14 2,9.27 8.91,8.26" />
      </svg>
    ),
  },
  {
    type: 'command',
    view: 'commands',
    label: 'Commands',
    color: '#3b82f6',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <polyline points="4,17 10,11 4,5" />
        <line x1="12" y1="19" x2="20" y2="19" />
      </svg>
    ),
  },
  {
    type: 'hook',
    view: 'hooks',
    label: 'Hooks',
    color: '#ec4899',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <path d="M10 13a5 5 0 007.54.54l3-3a5 5 0 00-7.07-7.07l-1.72 1.71" />
        <path d="M14 11a5 5 0 00-7.54-.54l-3 3a5 5 0 007.07 7.07l1.71-1.71" />
      </svg>
    ),
  },
  {
    type: 'plugin',
    view: 'plugins',
    label: 'Plugins',
    color: '#a855f7',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <rect x="4" y="4" width="16" height="16" rx="2" />
        <path d="M9 9h6v6H9z" />
        <path d="M9 1v3M15 1v3M9 20v3M15 20v3M1 9h3M1 15h3M20 9h3M20 15h3" />
      </svg>
    ),
  },
  {
    type: 'mcp',
    view: 'mcp',
    label: 'MCP Servers',
    color: '#14b8a6',
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
        <rect x="2" y="2" width="20" height="8" rx="2" />
        <rect x="2" y="14" width="20" height="8" rx="2" />
        <circle cx="6" cy="6" r="1" fill="currentColor" />
        <circle cx="6" cy="18" r="1" fill="currentColor" />
      </svg>
    ),
  },
];

export function Dashboard() {
  const settings = useAppStore(state => state.settings);
  const memory = useAppStore(state => state.memory);
  const agents = useAppStore(state => state.agents);
  const skills = useAppStore(state => state.skills);
  const commands = useAppStore(state => state.commands);
  const hooks = useAppStore(state => state.hooks);
  const plugins = useAppStore(state => state.plugins);
  const mcpServers = useAppStore(state => state.mcpServers);
  const duplicates = useAppStore(state => state.duplicates);
  const projects = useAppStore(state => state.projects);
  const setActiveView = useAppStore(state => state.setActiveView);
  const setActiveProject = useAppStore(state => state.setActiveProject);
  const lastDiscovery = useAppStore(state => state.lastDiscovery);
  
  const [showAllProjects, setShowAllProjects] = useState(false);
  const [expandedDuplicate, setExpandedDuplicate] = useState<string | null>(null);
  
  const counts: Record<string, number> = {
    settings: settings.length,
    memory: memory.length,
    agent: agents.length,
    skill: skills.length,
    command: commands.length,
    hook: hooks.length,
    plugin: plugins.length,
    mcp: mcpServers.length,
  };
  
  const totalEntities = Object.values(counts).reduce((a, b) => a + b, 0);
  
  const formatTime = (timestamp: number | null) => {
    if (!timestamp) return 'Never';
    const date = new Date(timestamp);
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  };
  
  const formatPath = (path: string) => {
    return path.replace(/^\/Users\/[^/]+/, '~');
  };
  
  // Build hierarchical project list with nesting levels
  const getProjectsWithNesting = () => {
    // Sort by path to ensure parent dirs come before children
    const sortedProjects = [...projects].sort((a, b) => a.path.localeCompare(b.path));
    
    return sortedProjects.map(project => {
      // Find nesting level by checking how many parent projects contain this one
      let nestLevel = 0;
      for (const other of sortedProjects) {
        if (other.path !== project.path && project.path.startsWith(other.path + '/')) {
          nestLevel++;
        }
      }
      return { ...project, nestLevel };
    });
  };
  
  const projectsWithNesting = getProjectsWithNesting();
  const visibleProjects = showAllProjects ? projectsWithNesting : projectsWithNesting.slice(0, 12);
  
  return (
    <div className="dashboard">
      <div className="dashboard-header">
        <h1 className="dashboard-title">Welcome to Agent Studio</h1>
        <p className="dashboard-subtitle">
          {totalEntities} entities across {projects.length} projects • Last scan: {formatTime(lastDiscovery)}
        </p>
      </div>
      
      {/* Entity Cards Grid */}
      <div className="dashboard-grid">
        {ENTITY_CONFIG.map((config) => (
          <button
            key={config.type}
            className="entity-card"
            onClick={() => setActiveView(config.view)}
          >
            <div 
              className="entity-card-icon"
              style={{ color: config.color }}
            >
              {config.icon}
            </div>
            <span className="entity-card-count">{counts[config.type]}</span>
            <span className="entity-card-label">{config.label}</span>
          </button>
        ))}
      </div>
      
      {/* Two Column Layout for Info Sections */}
      <div className="dashboard-sections">
        {/* Projects Section */}
        {projects.length > 0 && (
          <div className="dashboard-section dashboard-section-wide">
            <div className="dashboard-section-header">
              <h2 className="dashboard-section-title">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{ opacity: 0.5 }}>
                  <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z" />
                </svg>
                Projects ({projects.length})
              </h2>
              {projects.length > 12 && (
                <button 
                  className="dashboard-section-toggle"
                  onClick={() => setShowAllProjects(!showAllProjects)}
                >
                  {showAllProjects ? 'Show less' : `Show all ${projects.length}`}
                </button>
              )}
            </div>
            <div className="projects-grid">
              {visibleProjects.map((project) => {
                const counts = project.entity_counts;
                const totalEntitiesInProject = 
                  counts.settings + counts.memory + counts.agents + 
                  counts.skills + counts.commands + counts.plugins + 
                  counts.hooks + counts.mcp;
                
                const nestLevel = (project as any).nestLevel || 0;
                
                const handleProjectClick = () => {
                  // Navigate to project view for this project
                  setActiveProject(project.path);
                };
                
                return (
                  <button 
                    key={project.path} 
                    className="project-card"
                    data-nest-level={nestLevel}
                    onClick={handleProjectClick}
                    style={{ paddingLeft: `${12 + nestLevel * 20}px` }}
                  >
                    <div className="project-card-header">
                      {nestLevel > 0 ? (
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{ opacity: 0.3, flexShrink: 0 }}>
                          <polyline points="9,6 15,12 9,18" />
                        </svg>
                      ) : (
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{ opacity: 0.4, flexShrink: 0 }}>
                          <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z" />
                        </svg>
                      )}
                      <div className="project-card-name">{project.name}</div>
                    </div>
                    <div className="project-card-path">{formatPath(project.path)}</div>
                    {totalEntitiesInProject > 0 && (
                      <div className="project-card-stats">
                        {counts.agents > 0 && <span data-type="agents">{counts.agents}a</span>}
                        {counts.skills > 0 && <span data-type="skills">{counts.skills}s</span>}
                        {counts.commands > 0 && <span data-type="commands">{counts.commands}c</span>}
                        {counts.mcp > 0 && <span data-type="mcp">{counts.mcp}m</span>}
                      </div>
                    )}
                    <span
                      className="project-card-action"
                      onClick={(e) => {
                        e.stopPropagation();
                        openInFinder(project.path);
                      }}
                      title="Open in Finder"
                    >
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                        <path d="M18 13v6a2 2 0 01-2 2H5a2 2 0 01-2-2V8a2 2 0 012-2h6" />
                        <polyline points="15,3 21,3 21,9" />
                        <line x1="10" y1="14" x2="21" y2="3" />
                      </svg>
                    </span>
                  </button>
                );
              })}
            </div>
          </div>
        )}
        
        {/* Warnings Section */}
        {duplicates.length > 0 && (
          <div className="dashboard-section">
            <h2 className="dashboard-section-title">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#f59e0b" strokeWidth="2">
                <path d="M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0z" />
                <line x1="12" y1="9" x2="12" y2="13" />
                <line x1="12" y1="17" x2="12.01" y2="17" />
              </svg>
              Duplicates ({duplicates.length})
            </h2>
            <div className="duplicates-list">
              {duplicates.map((dup) => {
                const dupKey = `${dup.entity_type}-${dup.name}`;
                const isExpanded = expandedDuplicate === dupKey;
                
                return (
                  <div key={dupKey} className="duplicate-item">
                    <button
                      className="duplicate-item-header"
                      onClick={() => setExpandedDuplicate(isExpanded ? null : dupKey)}
                    >
                      <span className="duplicate-item-name">{dup.name}</span>
                      <span className="duplicate-item-meta">
                        <span className="duplicate-item-type">{dup.entity_type}</span>
                        <span className="duplicate-item-count">{dup.entities.length} locations</span>
                        <svg 
                          width="12" 
                          height="12" 
                          viewBox="0 0 24 24" 
                          fill="none" 
                          stroke="currentColor" 
                          strokeWidth="2"
                          style={{ 
                            transform: isExpanded ? 'rotate(180deg)' : 'rotate(0deg)',
                            transition: 'transform 0.15s ease'
                          }}
                        >
                          <polyline points="6,9 12,15 18,9" />
                        </svg>
                      </span>
                    </button>
                    {isExpanded && (
                      <div className="duplicate-item-locations">
                        {dup.entities
                          .sort((a, b) => a.precedence - b.precedence)
                          .map((entity, idx) => (
                            <div key={entity.id} className="duplicate-location">
                              <span className="duplicate-location-precedence">
                                {idx === 0 ? 'Active' : `#${idx + 1}`}
                              </span>
                              <span className="duplicate-location-scope">
                                {entity.scope}
                              </span>
                              <span className="duplicate-location-path">
                                {formatPath(entity.path)}
                              </span>
                            </div>
                          ))}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </div>
      
      {/* Empty State */}
      {totalEntities === 0 && projects.length === 0 && (
        <div className="dashboard-empty">
          <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1" style={{ opacity: 0.3, marginBottom: 16 }}>
            <circle cx="12" cy="12" r="10" />
            <path d="M12 6v6l4 2" />
          </svg>
          <p style={{ marginBottom: 8 }}>No configurations found yet</p>
          <p style={{ fontSize: 12, opacity: 0.6 }}>Press ⌘R to refresh or add a project directory</p>
        </div>
      )}
    </div>
  );
}
