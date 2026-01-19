// ============================================================================
// GlobalSearch - Raycast-style ⌘K command palette
// ============================================================================

import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import { motion, AnimatePresence } from 'motion/react';
import { 
  Search, 
  Settings, 
  FileText, 
  User, 
  Star, 
  Terminal, 
  Zap,
  Puzzle,
  Server,
  LayoutDashboard,
  Activity,
  Plus,
  RefreshCw,
  Sun,
  Moon,
  ChevronRight,
  Command,
} from 'lucide-react';
import { clsx } from 'clsx';
import { useAppStore } from '../store/appStore';
import type { EntityType, ViewType, DisplayableEntity, ToolType } from '../lib/types';
import { TOOL_COLORS } from '../lib/types';
import { isFlatEntity, isHookEntity, isMcpServerEntity } from '../lib/types';

// ============================================================================
// Types
// ============================================================================

interface SearchAction {
  id: string;
  label: string;
  description?: string;
  shortcut?: string;
  icon: React.ReactNode;
  category: 'action' | 'navigation' | 'create';
  execute: () => void;
}

interface SearchResult {
  id: string;
  type: 'entity' | 'action' | 'navigation' | 'recent';
  label: string;
  description?: string;
  shortcut?: string;
  icon: React.ReactNode;
  category: string;
  entityType?: EntityType;
  tool?: ToolType;
  scope?: string;
  path?: string;
  score: number;
  onSelect: () => void;
}

// ============================================================================
// Constants
// ============================================================================

const ENTITY_ICONS: Record<EntityType, React.ReactNode> = {
  settings: <Settings className="w-4 h-4" />,
  memory: <FileText className="w-4 h-4" />,
  agent: <User className="w-4 h-4" />,
  skill: <Star className="w-4 h-4" />,
  command: <Terminal className="w-4 h-4" />,
  hook: <Zap className="w-4 h-4" />,
  plugin: <Puzzle className="w-4 h-4" />,
  mcp: <Server className="w-4 h-4" />,
};

// ============================================================================
// Component
// ============================================================================

export function GlobalSearch() {
  const isOpen = useAppStore(state => state.isGlobalSearchOpen);
  const query = useAppStore(state => state.globalSearchQuery);
  const recentEntityIds = useAppStore(state => state.recentEntityIds);
  const closeGlobalSearch = useAppStore(state => state.closeGlobalSearch);
  const setGlobalSearchQuery = useAppStore(state => state.setGlobalSearchQuery);
  const addToRecentEntities = useAppStore(state => state.addToRecentEntities);
  const setActiveView = useAppStore(state => state.setActiveView);
  const openPanel = useAppStore(state => state.openPanel);
  const refreshDiscovery = useAppStore(state => state.refreshDiscovery);
  const toggleTheme = useAppStore(state => state.toggleTheme);
  const theme = useAppStore(state => state.theme);
  
  // Get all entities
  const settings = useAppStore(state => state.settings);
  const memory = useAppStore(state => state.memory);
  const agents = useAppStore(state => state.agents);
  const skills = useAppStore(state => state.skills);
  const commands = useAppStore(state => state.commands);
  const hooks = useAppStore(state => state.hooks);
  const plugins = useAppStore(state => state.plugins);
  const mcpServers = useAppStore(state => state.mcpServers);
  
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [_showEntityActions, setShowEntityActions] = useState(false); // Future: Tab to show entity actions
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  
  // Focus input when opening
  useEffect(() => {
    if (isOpen) {
      setSelectedIndex(0);
      setShowEntityActions(false);
      setTimeout(() => inputRef.current?.focus(), 50);
    }
  }, [isOpen]);
  
  // Fuzzy match helper
  const fuzzyMatch = useCallback((text: string, searchQuery: string): { matches: boolean; score: number } => {
    if (!searchQuery) return { matches: true, score: 0 };
    
    const textLower = text.toLowerCase();
    const queryLower = searchQuery.toLowerCase();
    
    if (textLower === queryLower) return { matches: true, score: 100 };
    if (textLower.startsWith(queryLower)) return { matches: true, score: 90 };
    if (textLower.includes(queryLower)) return { matches: true, score: 70 };
    
    // Word boundary match
    const words = textLower.split(/[-_\s/]+/);
    for (const word of words) {
      if (word.startsWith(queryLower)) return { matches: true, score: 60 };
    }
    
    // Fuzzy character matching
    let textIndex = 0;
    let queryIndex = 0;
    while (textIndex < textLower.length && queryIndex < queryLower.length) {
      if (textLower[textIndex] === queryLower[queryIndex]) queryIndex++;
      textIndex++;
    }
    if (queryIndex === queryLower.length) {
      return { matches: true, score: Math.max(20, 50 - (textLower.length - queryIndex)) };
    }
    
    return { matches: false, score: 0 };
  }, []);
  
  // Build actions list
  const actions = useMemo((): SearchAction[] => [
    {
      id: 'create-agent',
      label: 'Create new agent',
      description: 'Create a new agent definition',
      shortcut: '⌘N',
      icon: <Plus className="w-4 h-4" />,
      category: 'create',
      execute: () => {
        closeGlobalSearch();
        // Will be handled by App.tsx
        window.dispatchEvent(new CustomEvent('openCreateDialog', { detail: { type: 'agent' } }));
      },
    },
    {
      id: 'create-skill',
      label: 'Create new skill',
      description: 'Create a new skill definition',
      icon: <Plus className="w-4 h-4" />,
      category: 'create',
      execute: () => {
        closeGlobalSearch();
        window.dispatchEvent(new CustomEvent('openCreateDialog', { detail: { type: 'skill' } }));
      },
    },
    {
      id: 'create-command',
      label: 'Create new command',
      description: 'Create a new slash command',
      icon: <Plus className="w-4 h-4" />,
      category: 'create',
      execute: () => {
        closeGlobalSearch();
        window.dispatchEvent(new CustomEvent('openCreateDialog', { detail: { type: 'command' } }));
      },
    },
    {
      id: 'refresh',
      label: 'Refresh discovery',
      description: 'Rescan for configuration files',
      shortcut: '⌘R',
      icon: <RefreshCw className="w-4 h-4" />,
      category: 'action',
      execute: () => {
        closeGlobalSearch();
        refreshDiscovery();
      },
    },
    {
      id: 'toggle-theme',
      label: theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode',
      description: 'Toggle between dark and light theme',
      icon: theme === 'dark' ? <Sun className="w-4 h-4" /> : <Moon className="w-4 h-4" />,
      category: 'action',
      execute: () => {
        toggleTheme();
      },
    },
    {
      id: 'nav-dashboard',
      label: 'Go to Dashboard',
      icon: <LayoutDashboard className="w-4 h-4" />,
      category: 'navigation',
      execute: () => {
        closeGlobalSearch();
        setActiveView('dashboard');
      },
    },
    {
      id: 'nav-health',
      label: 'Go to Health Check',
      icon: <Activity className="w-4 h-4" />,
      category: 'navigation',
      execute: () => {
        closeGlobalSearch();
        setActiveView('health');
      },
    },
    {
      id: 'nav-agents',
      label: 'Go to Agents',
      icon: <User className="w-4 h-4" />,
      category: 'navigation',
      execute: () => {
        closeGlobalSearch();
        setActiveView('agents');
      },
    },
    {
      id: 'nav-skills',
      label: 'Go to Skills',
      icon: <Star className="w-4 h-4" />,
      category: 'navigation',
      execute: () => {
        closeGlobalSearch();
        setActiveView('skills');
      },
    },
    {
      id: 'nav-commands',
      label: 'Go to Commands',
      icon: <Terminal className="w-4 h-4" />,
      category: 'navigation',
      execute: () => {
        closeGlobalSearch();
        setActiveView('commands');
      },
    },
  ], [closeGlobalSearch, refreshDiscovery, toggleTheme, theme, setActiveView]);
  
  // Convert entity to SearchResult
  const entityToResult = useCallback((
    entity: DisplayableEntity, 
    entityType: EntityType,
    score: number
  ): SearchResult | null => {
    let name = '';
    let path = '';
    let scope = '';
    let tool: ToolType = 'claude';
    
    if (isFlatEntity(entity)) {
      name = entity.name;
      path = entity.path;
      scope = entity.scope;
      tool = entity.tool;
    } else if (isHookEntity(entity)) {
      name = `${entity.event}${entity.matcher ? ` (${entity.matcher})` : ''}`;
      path = entity.source_path;
      scope = entity.source;
      tool = entity.tool;
    } else if (isMcpServerEntity(entity)) {
      name = entity.name;
      path = entity.source_path;
      scope = entity.scope;
      tool = entity.tool;
    } else {
      return null;
    }
    
    return {
      id: entity.id,
      type: 'entity',
      label: name,
      description: path.replace(/^\/Users\/[^/]+/, '~'),
      icon: ENTITY_ICONS[entityType],
      category: entityType,
      entityType,
      tool,
      scope,
      path,
      score,
      onSelect: () => {
        addToRecentEntities(entity.id);
        closeGlobalSearch();
        // Navigate to the appropriate view
        const viewMap: Record<EntityType, ViewType> = {
          settings: 'settings',
          memory: 'memory',
          agent: 'agents',
          skill: 'skills',
          command: 'commands',
          hook: 'hooks',
          plugin: 'plugins',
          mcp: 'mcp',
        };
        setActiveView(viewMap[entityType]);
        openPanel(entity);
      },
    };
  }, [addToRecentEntities, closeGlobalSearch, setActiveView, openPanel]);
  
  // Build search results
  const results = useMemo((): SearchResult[] => {
    const allResults: SearchResult[] = [];
    const searchQuery = query.trim();
    const isActionQuery = searchQuery.startsWith('>');
    const isNavQuery = searchQuery.startsWith('/');
    const actualQuery = isActionQuery || isNavQuery ? searchQuery.slice(1).trim() : searchQuery;
    
    // If no query, show recent items and popular actions
    if (!searchQuery) {
      // Recent entities
      const allEntities = [
        ...settings.map(e => ({ entity: e, type: 'settings' as EntityType })),
        ...memory.map(e => ({ entity: e, type: 'memory' as EntityType })),
        ...agents.map(e => ({ entity: e, type: 'agent' as EntityType })),
        ...skills.map(e => ({ entity: e, type: 'skill' as EntityType })),
        ...commands.map(e => ({ entity: e, type: 'command' as EntityType })),
        ...hooks.map(e => ({ entity: e, type: 'hook' as EntityType })),
        ...plugins.map(e => ({ entity: e, type: 'plugin' as EntityType })),
        ...mcpServers.map(e => ({ entity: e, type: 'mcp' as EntityType })),
      ];
      
      for (const recentId of recentEntityIds) {
        const found = allEntities.find(e => e.entity.id === recentId);
        if (found) {
          const result = entityToResult(found.entity, found.type, 100);
          if (result) {
            result.type = 'recent';
            allResults.push(result);
          }
        }
      }
      
      // Add popular actions
      const popularActions = actions.slice(0, 5);
      for (const action of popularActions) {
        allResults.push({
          id: action.id,
          type: 'action',
          label: action.label,
          description: action.description,
          shortcut: action.shortcut,
          icon: action.icon,
          category: action.category,
          score: 50,
          onSelect: action.execute,
        });
      }
      
      return allResults;
    }
    
    // Filter actions if > prefix or matching
    if (isActionQuery || !isNavQuery) {
      for (const action of actions) {
        if (action.category === 'navigation' && !isNavQuery && isActionQuery) continue;
        
        const match = fuzzyMatch(action.label, actualQuery);
        if (match.matches) {
          allResults.push({
            id: action.id,
            type: 'action',
            label: action.label,
            description: action.description,
            shortcut: action.shortcut,
            icon: action.icon,
            category: action.category,
            score: match.score,
            onSelect: action.execute,
          });
        }
      }
    }
    
    // Filter navigation if / prefix
    if (isNavQuery) {
      for (const action of actions.filter(a => a.category === 'navigation')) {
        const match = fuzzyMatch(action.label, actualQuery);
        if (match.matches) {
          allResults.push({
            id: action.id,
            type: 'navigation',
            label: action.label,
            icon: action.icon,
            category: 'navigation',
            score: match.score,
            onSelect: action.execute,
          });
        }
      }
    }
    
    // Search entities (skip if action or nav query)
    if (!isActionQuery && !isNavQuery) {
      const entityCollections = [
        { entities: settings, type: 'settings' as EntityType },
        { entities: memory, type: 'memory' as EntityType },
        { entities: agents, type: 'agent' as EntityType },
        { entities: skills, type: 'skill' as EntityType },
        { entities: commands, type: 'command' as EntityType },
        { entities: hooks, type: 'hook' as EntityType },
        { entities: plugins, type: 'plugin' as EntityType },
        { entities: mcpServers, type: 'mcp' as EntityType },
      ];
      
      for (const { entities, type } of entityCollections) {
        for (const entity of entities) {
          let name = '';
          if (isFlatEntity(entity)) name = entity.name;
          else if (isHookEntity(entity)) name = entity.event;
          else if (isMcpServerEntity(entity)) name = entity.name;
          
          const match = fuzzyMatch(name, actualQuery);
          if (match.matches) {
            const result = entityToResult(entity, type, match.score);
            if (result) allResults.push(result);
          }
        }
      }
    }
    
    // Sort by score
    allResults.sort((a, b) => b.score - a.score);
    
    // Limit results
    return allResults.slice(0, 20);
  }, [query, recentEntityIds, actions, settings, memory, agents, skills, commands, hooks, plugins, mcpServers, fuzzyMatch, entityToResult]);
  
  // Group results by category
  const groupedResults = useMemo(() => {
    const groups: { title: string; items: SearchResult[] }[] = [];
    
    const recent = results.filter(r => r.type === 'recent');
    const actionResults = results.filter(r => r.type === 'action' && r.category !== 'navigation');
    const navResults = results.filter(r => r.type === 'navigation' || (r.type === 'action' && r.category === 'navigation'));
    const entityResults = results.filter(r => r.type === 'entity');
    
    if (recent.length > 0) groups.push({ title: 'Recent', items: recent });
    if (entityResults.length > 0) groups.push({ title: 'Entities', items: entityResults });
    if (actionResults.length > 0) groups.push({ title: 'Actions', items: actionResults });
    if (navResults.length > 0) groups.push({ title: 'Navigation', items: navResults });
    
    return groups;
  }, [results]);
  
  // Flat list for keyboard navigation
  const flatResults = useMemo(() => 
    groupedResults.flatMap(g => g.items),
  [groupedResults]);
  
  // Keyboard navigation
  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setSelectedIndex(i => Math.min(i + 1, flatResults.length - 1));
        break;
      case 'ArrowUp':
        e.preventDefault();
        setSelectedIndex(i => Math.max(i - 1, 0));
        break;
      case 'Enter':
        e.preventDefault();
        if (flatResults[selectedIndex]) {
          flatResults[selectedIndex].onSelect();
        }
        break;
      case 'Escape':
        e.preventDefault();
        closeGlobalSearch();
        break;
      case 'Tab':
        e.preventDefault();
        // Toggle entity actions (future feature)
        setShowEntityActions(prev => !prev);
        break;
    }
  }, [flatResults, selectedIndex, closeGlobalSearch]);
  
  // Scroll selected item into view
  useEffect(() => {
    if (listRef.current) {
      const selectedEl = listRef.current.querySelector(`[data-index="${selectedIndex}"]`);
      if (selectedEl) {
        selectedEl.scrollIntoView({ block: 'nearest' });
      }
    }
  }, [selectedIndex]);
  
  // Reset selection when results change
  useEffect(() => {
    setSelectedIndex(0);
  }, [query]);
  
  if (!isOpen) return null;
  
  let globalIndex = 0;
  
  return (
    <AnimatePresence>
      <div className="fixed inset-0 z-[100] flex items-start justify-center pt-[15vh]">
        {/* Backdrop */}
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.15 }}
          className="absolute inset-0 bg-black/50 backdrop-blur-sm"
          onClick={closeGlobalSearch}
        />
        
        {/* Modal */}
        <motion.div
          initial={{ opacity: 0, scale: 0.95, y: -20 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.95, y: -20 }}
          transition={{ duration: 0.15, ease: [0.25, 0.46, 0.45, 0.94] }}
          className="relative w-full max-w-xl mx-4 bg-[var(--color-bg-primary)] rounded-xl border border-[var(--color-border)] shadow-2xl overflow-hidden"
          onKeyDown={handleKeyDown}
        >
          {/* Search Input */}
          <div className="flex items-center gap-3 px-4 py-3 border-b border-[var(--color-border)]">
            <Search className="w-5 h-5 text-[var(--color-text-tertiary)] shrink-0" />
            <input
              ref={inputRef}
              type="text"
              value={query}
              onChange={(e) => setGlobalSearchQuery(e.target.value)}
              placeholder="Search entities, type > for actions, / for navigation..."
              className="flex-1 bg-transparent text-[var(--color-text-primary)] text-sm placeholder:text-[var(--color-text-quaternary)] focus:outline-none"
            />
            <kbd className="hidden sm:flex items-center gap-1 px-2 py-1 text-[10px] font-medium text-[var(--color-text-quaternary)] bg-[var(--color-bg-tertiary)] rounded border border-[var(--color-border-subtle)]">
              <Command className="w-3 h-3" />K
            </kbd>
          </div>
          
          {/* Results */}
          <div ref={listRef} className="max-h-[400px] overflow-y-auto">
            {groupedResults.length === 0 ? (
              <div className="px-4 py-8 text-center text-sm text-[var(--color-text-tertiary)]">
                No results found
              </div>
            ) : (
              groupedResults.map((group) => (
                <div key={group.title}>
                  {/* Section Header */}
                  <div className="px-4 py-2 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-quaternary)] bg-[var(--color-bg-secondary)]">
                    {group.title}
                  </div>
                  
                  {/* Items */}
                  {group.items.map((item) => {
                    const currentIndex = globalIndex++;
                    const isSelected = currentIndex === selectedIndex;
                    
                    return (
                      <button
                        key={item.id}
                        data-index={currentIndex}
                        onClick={item.onSelect}
                        className={clsx(
                          'w-full flex items-center gap-3 px-4 py-2.5 text-left transition-colors',
                          isSelected 
                            ? 'bg-[var(--color-accent)] text-white' 
                            : 'hover:bg-[var(--color-bg-hover)]'
                        )}
                      >
                        {/* Icon */}
                        <div className={clsx(
                          'flex items-center justify-center w-8 h-8 rounded-lg shrink-0',
                          isSelected ? 'bg-white/20' : 'bg-[var(--color-bg-elevated)]'
                        )}>
                          <span className={isSelected ? 'text-white' : 'text-[var(--color-text-secondary)]'}>
                            {item.icon}
                          </span>
                        </div>
                        
                        {/* Content */}
                        <div className="flex-1 min-w-0">
                          <div className="flex items-center gap-2">
                            {/* Tool indicator */}
                            {item.tool && (
                              <span 
                                className="w-2 h-2 rounded-full shrink-0"
                                style={{ backgroundColor: TOOL_COLORS[item.tool] }}
                              />
                            )}
                            <span className={clsx(
                              'text-sm font-medium truncate',
                              isSelected ? 'text-white' : 'text-[var(--color-text-primary)]'
                            )}>
                              {item.label}
                            </span>
                            {item.scope && (
                              <span className={clsx(
                                'text-[10px] shrink-0',
                                isSelected ? 'text-white/50' : 'text-[var(--color-text-quaternary)]'
                              )}>
                                {item.scope === 'global' ? 'Global' : 'Project'}
                              </span>
                            )}
                          </div>
                          {item.description && (
                            <div className={clsx(
                              'text-xs truncate mt-0.5',
                              isSelected ? 'text-white/70' : 'text-[var(--color-text-tertiary)]'
                            )}>
                              {item.description}
                            </div>
                          )}
                        </div>
                        
                        {/* Shortcut or Chevron */}
                        {item.shortcut ? (
                          <kbd className={clsx(
                            'px-1.5 py-0.5 text-[10px] font-medium rounded shrink-0',
                            isSelected 
                              ? 'bg-white/20 text-white' 
                              : 'bg-[var(--color-bg-tertiary)] text-[var(--color-text-quaternary)]'
                          )}>
                            {item.shortcut}
                          </kbd>
                        ) : (
                          <ChevronRight className={clsx(
                            'w-4 h-4 shrink-0',
                            isSelected ? 'text-white/70' : 'text-[var(--color-text-quaternary)]'
                          )} />
                        )}
                      </button>
                    );
                  })}
                </div>
              ))
            )}
          </div>
          
          {/* Footer */}
          <div className="flex items-center justify-between px-4 py-2 border-t border-[var(--color-border)] bg-[var(--color-bg-secondary)]">
            <div className="flex items-center gap-4 text-[10px] text-[var(--color-text-quaternary)]">
              <span className="flex items-center gap-1">
                <kbd className="px-1 py-0.5 bg-[var(--color-bg-tertiary)] rounded">↑↓</kbd>
                Navigate
              </span>
              <span className="flex items-center gap-1">
                <kbd className="px-1 py-0.5 bg-[var(--color-bg-tertiary)] rounded">↵</kbd>
                Select
              </span>
              <span className="flex items-center gap-1">
                <kbd className="px-1 py-0.5 bg-[var(--color-bg-tertiary)] rounded">esc</kbd>
                Close
              </span>
            </div>
            <div className="flex items-center gap-1 text-[10px] text-[var(--color-text-quaternary)]">
              <span className="flex items-center gap-1">
                <kbd className="px-1 py-0.5 bg-[var(--color-bg-tertiary)] rounded">Tab</kbd>
                Actions
              </span>
            </div>
          </div>
        </motion.div>
      </div>
    </AnimatePresence>
  );
}
