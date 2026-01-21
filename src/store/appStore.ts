// ============================================================================
// Agent Studio - Application State Store
// Comprehensive state management with auto-discovery
// ============================================================================

import { create } from 'zustand';
import type {
  DiscoveryResult,
  SettingsEntity,
  MemoryEntity,
  AgentEntity,
  SkillEntity,
  CommandEntity,
  HookEntity,
  PluginEntity,
  McpServerEntity,
  ProjectInfo,
  DuplicateGroup,
  SymlinkInfo,
  Toast,
  ViewType,
  FilterScope,
  DisplayableEntity,
  EntityType,
  CommandPaletteItem,
  CommandPaletteSection,
  ToolType,
} from '../lib/types';
import { discoverAll, getHomeDirectory } from '../lib/api';

// ============================================================================
// State Interface
// ============================================================================

interface AppState {
  // === Discovery State ===
  isLoading: boolean;
  lastDiscovery: number | null;
  error: string | null;
  
  // === Global Config ===
  globalConfigPath: string;
  homeDirectory: string;
  
  // === Detected Projects ===
  projects: ProjectInfo[];
  
  // === All Entities ===
  settings: SettingsEntity[];
  memory: MemoryEntity[];
  agents: AgentEntity[];
  skills: SkillEntity[];
  commands: CommandEntity[];
  hooks: HookEntity[];
  plugins: PluginEntity[];
  mcpServers: McpServerEntity[];
  
  // === Analysis ===
  duplicates: DuplicateGroup[];
  symlinks: SymlinkInfo[];
  
  // === UI State ===
  activeView: ViewType;
  selectedEntity: DisplayableEntity | null;
  searchQuery: string;
  filterScope: FilterScope;
  filterProject: string | null;
  filterTool: 'all' | ToolType;  // Filter by tool (claude/opencode)
  isPanelOpen: boolean;
  theme: 'dark' | 'light';
  
  // === Global Search State ===
  isGlobalSearchOpen: boolean;
  globalSearchQuery: string;
  recentEntityIds: string[];  // Track recently accessed entities (max 5)
  
  // === Toast Notifications ===
  toasts: Toast[];
  
  // === Animation State ===
  hasInitiallyAnimated: boolean;
  previousItemIds: Set<string>;
  
  // === Cached Computations ===
  _cachedSections: CommandPaletteSection[] | null;
  
  // === Computed Getters ===
  getSections: () => CommandPaletteSection[];
  getFilteredSections: () => CommandPaletteSection[];
  getCurrentEntityList: () => DisplayableEntity[];
  isNewItem: (id: string) => boolean;
  getEntityCounts: () => Record<EntityType, number>;
  
  // === Discovery Actions ===
  discoverAll: (projectPaths?: string[]) => Promise<void>;
  refreshDiscovery: () => Promise<void>;
  
  // === UI Actions ===
  setActiveView: (view: ViewType) => void;
  selectEntity: (entity: DisplayableEntity | null) => void;
  openPanel: (entity: DisplayableEntity) => void;
  closePanel: () => void;
  setSearchQuery: (query: string) => void;
  setFilterScope: (scope: FilterScope) => void;
  setFilterProject: (path: string | null) => void;
  setActiveProject: (projectPath: string) => void;
  setFilterTool: (tool: 'all' | ToolType) => void;
  setTheme: (theme: 'dark' | 'light') => void;
  toggleTheme: () => void;
  
  // === Global Search Actions ===
  openGlobalSearch: () => void;
  closeGlobalSearch: () => void;
  setGlobalSearchQuery: (query: string) => void;
  addToRecentEntities: (id: string) => void;
  
  // === Entity Actions ===
  updateEntityContent: (entityType: EntityType, id: string, content: string) => void;
  removeEntity: (entityType: EntityType, id: string) => void;
  
  // === Toast Actions ===
  addToast: (toast: Omit<Toast, 'id'>) => string;
  removeToast: (id: string) => void;
  
  // === Animation Actions ===
  setHasInitiallyAnimated: (value: boolean) => void;
  markItemsAsSeen: () => void;
  
  // === Error Handling ===
  setError: (error: string | null) => void;
}

// ============================================================================
// Helper Functions
// ============================================================================

function generateToastId(): string {
  return `toast_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
}

function buildSections(state: AppState): CommandPaletteSection[] {
  const sections: CommandPaletteSection[] = [];
  const { filterScope, filterProject, filterTool, searchQuery } = state;
  
  // Filter entities with flattened base fields (scope, project_path, tool directly on entity)
  const filterFlatEntity = <T extends { scope: string; project_path: string | null; tool: ToolType }>(
    entities: T[]
  ): T[] => {
    return entities.filter(e => {
      if (filterScope === 'global' && e.scope !== 'global') return false;
      if (filterScope === 'project' && e.scope !== 'project') return false;
      if (filterProject && e.project_path !== filterProject) return false;
      if (filterTool !== 'all' && e.tool !== filterTool) return false;
      return true;
    });
  };
  
  // Simple fuzzy matching with scoring
  const fuzzyMatch = (text: string, query: string): { matches: boolean; score: number } => {
    if (!query) return { matches: true, score: 0 };
    
    const textLower = text.toLowerCase();
    const queryLower = query.toLowerCase();
    
    // Exact match gets highest score
    if (textLower === queryLower) return { matches: true, score: 100 };
    
    // Starts with query gets high score
    if (textLower.startsWith(queryLower)) return { matches: true, score: 90 };
    
    // Contains query as substring gets medium score
    if (textLower.includes(queryLower)) return { matches: true, score: 70 };
    
    // Word boundary match (query matches start of a word)
    const words = textLower.split(/[-_\s/]+/);
    for (const word of words) {
      if (word.startsWith(queryLower)) return { matches: true, score: 60 };
    }
    
    // Fuzzy character matching (all query chars appear in order)
    let textIndex = 0;
    let queryIndex = 0;
    let matchCount = 0;
    
    while (textIndex < textLower.length && queryIndex < queryLower.length) {
      if (textLower[textIndex] === queryLower[queryIndex]) {
        matchCount++;
        queryIndex++;
      }
      textIndex++;
    }
    
    if (queryIndex === queryLower.length) {
      // All query characters found in order
      const score = Math.max(20, 50 - (textLower.length - matchCount));
      return { matches: true, score };
    }
    
    return { matches: false, score: 0 };
  };
  
  const matchesSearch = (item: CommandPaletteItem): { matches: boolean; score: number } => {
    if (!searchQuery.trim()) return { matches: true, score: 0 };
    
    const query = searchQuery.trim();
    
    // Check name (highest priority)
    const nameMatch = fuzzyMatch(item.name, query);
    if (nameMatch.matches) return { matches: true, score: nameMatch.score };
    
    // Check description
    if (item.description) {
      const descMatch = fuzzyMatch(item.description, query);
      if (descMatch.matches) return { matches: true, score: descMatch.score - 10 }; // Slightly lower priority
    }
    
    // Check path
    const pathMatch = fuzzyMatch(item.path, query);
    if (pathMatch.matches) return { matches: true, score: pathMatch.score - 20 }; // Lower priority
    
    return { matches: false, score: 0 };
  };
  
  // Helper to filter and sort items by search score
  const filterAndSort = (items: CommandPaletteItem[]): CommandPaletteItem[] => {
    const withScores = items
      .map(item => ({ item, ...matchesSearch(item) }))
      .filter(({ matches }) => matches);
    
    // Sort by score (highest first), then by name
    withScores.sort((a, b) => {
      if (b.score !== a.score) return b.score - a.score;
      return a.item.name.localeCompare(b.item.name);
    });
    
    return withScores.map(({ item }) => item);
  };
  
  // Settings
  const filteredSettings = filterFlatEntity(state.settings);
  if (filteredSettings.length > 0) {
    const items: CommandPaletteItem[] = filteredSettings.map(s => ({
      id: s.id,
      name: s.name,
      description: s.variant === 'local' ? 'Local Settings' : s.variant === 'project' ? 'Project Settings' : 'Global Settings',
      scope: s.scope as 'global' | 'project',
      projectName: s.project_path?.split('/').pop(),
      entityType: 'settings' as EntityType,
      path: s.path,
      isSymlink: s.is_symlink,
      tool: s.tool,
    }));
    
    const filtered = filterAndSort(items);
    if (filtered.length > 0) {
      sections.push({ id: 'settings', title: 'Settings', entityType: 'settings', items: filtered });
    }
  }
  
  // Memory (CLAUDE.md / AGENTS.md)
  const filteredMemory = filterFlatEntity(state.memory);
  if (filteredMemory.length > 0) {
    const items: CommandPaletteItem[] = filteredMemory.map(m => ({
      id: m.id,
      name: m.name,
      description: m.variant === 'root' ? 'Root Memory' : m.variant === 'dotopencode' ? '.opencode Memory' : '.claude Memory',
      scope: m.scope as 'global' | 'project',
      projectName: m.project_path?.split('/').pop(),
      entityType: 'memory' as EntityType,
      path: m.path,
      isSymlink: m.is_symlink,
      tool: m.tool,
    }));
    
    const filtered = filterAndSort(items);
    if (filtered.length > 0) {
      sections.push({ id: 'memory', title: 'Memory', entityType: 'memory', items: filtered });
    }
  }
  
  // Agents
  const filteredAgents = filterFlatEntity(state.agents);
  if (filteredAgents.length > 0) {
    const items: CommandPaletteItem[] = filteredAgents.map(a => ({
      id: a.id,
      name: a.name,
      description: (a.frontmatter?.description as string) || undefined,
      scope: a.scope as 'global' | 'project',
      projectName: a.project_path?.split('/').pop(),
      entityType: 'agent' as EntityType,
      path: a.path,
      isSymlink: a.is_symlink,
      isDuplicate: state.duplicates.some(d => d.entity_type === 'agent' && d.entities.some(e => e.id === a.id)),
      tool: a.tool,
    }));
    
    const filtered = filterAndSort(items);
    if (filtered.length > 0) {
      sections.push({ id: 'agents', title: 'Agents', entityType: 'agent', items: filtered });
    }
  }
  
  // Skills
  const filteredSkills = filterFlatEntity(state.skills);
  if (filteredSkills.length > 0) {
    const items: CommandPaletteItem[] = filteredSkills.map(s => ({
      id: s.id,
      name: s.name,
      description: (s.frontmatter?.description as string) || undefined,
      scope: s.scope as 'global' | 'project',
      projectName: s.project_path?.split('/').pop(),
      entityType: 'skill' as EntityType,
      path: s.path,
      isSymlink: s.is_symlink,
      isDuplicate: state.duplicates.some(d => d.entity_type === 'skill' && d.entities.some(e => e.id === s.id)),
      tool: s.tool,
    }));
    
    const filtered = filterAndSort(items);
    if (filtered.length > 0) {
      sections.push({ id: 'skills', title: 'Skills', entityType: 'skill', items: filtered });
    }
  }
  
  // Commands
  const filteredCommands = filterFlatEntity(state.commands);
  if (filteredCommands.length > 0) {
    const items: CommandPaletteItem[] = filteredCommands.map(c => ({
      id: c.id,
      name: c.namespace ? `${c.namespace}/${c.name}` : c.name,
      description: (c.frontmatter?.description as string) || undefined,
      scope: c.scope as 'global' | 'project',
      projectName: c.project_path?.split('/').pop(),
      entityType: 'command' as EntityType,
      path: c.path,
      isSymlink: c.is_symlink,
      isDuplicate: state.duplicates.some(d => d.entity_type === 'command' && d.entities.some(e => e.id === c.id)),
      tool: c.tool,
    }));
    
    const filtered = filterAndSort(items);
    if (filtered.length > 0) {
      sections.push({ id: 'commands', title: 'Commands', entityType: 'command', items: filtered });
    }
  }
  
  // Plugins
  const filteredPlugins = filterFlatEntity(state.plugins);
  if (filteredPlugins.length > 0) {
    const items: CommandPaletteItem[] = filteredPlugins.map(p => ({
      id: p.id,
      name: p.name,
      description: (p.manifest?.description as string) || undefined,
      scope: p.scope as 'global' | 'project',
      projectName: p.project_path?.split('/').pop(),
      entityType: 'plugin' as EntityType,
      path: p.path,
      isSymlink: p.is_symlink,
      tool: p.tool,
    }));
    
    const filtered = filterAndSort(items);
    if (filtered.length > 0) {
      sections.push({ id: 'plugins', title: 'Plugins', entityType: 'plugin', items: filtered });
    }
  }
  
  // MCP Servers
  if (state.mcpServers.length > 0) {
    const items: CommandPaletteItem[] = state.mcpServers
      .filter(m => {
        if (filterScope === 'global' && m.scope !== 'global' && m.scope !== 'user') return false;
        if (filterScope === 'project' && m.scope !== 'project') return false;
        if (filterProject && !m.source_path.startsWith(filterProject + '/')) return false;
        if (filterTool !== 'all' && m.tool !== filterTool) return false;
        return true;
      })
      .map(m => ({
        id: m.id,
        name: m.name,
        description: `${m.transport} ${m.is_from_plugin ? `(from ${m.plugin_name})` : ''}`,
        scope: m.scope === 'project' ? 'project' : 'global',
        entityType: 'mcp' as EntityType,
        path: m.source_path,
        tool: m.tool,
      }));
    
    const filtered = filterAndSort(items);
    if (filtered.length > 0) {
      sections.push({ id: 'mcp', title: 'MCP Servers', entityType: 'mcp', items: filtered });
    }
  }
  
  // Hooks (grouped by event)
  if (state.hooks.length > 0) {
    const items: CommandPaletteItem[] = state.hooks
      .filter(h => {
        if (filterScope === 'global' && h.source !== 'global') return false;
        if (filterScope === 'project' && h.source !== 'project' && h.source !== 'local') return false;
        if (filterProject && !h.source_path.startsWith(filterProject + '/')) return false;
        if (filterTool !== 'all' && h.tool !== filterTool) return false;
        return true;
      })
      .map(h => ({
        id: h.id,
        name: `${h.event}${h.matcher ? ` (${h.matcher})` : ''}`,
        description: `${h.hooks.length} hook(s) - ${h.source}`,
        scope: h.source === 'global' ? 'global' : 'project',
        entityType: 'hook' as EntityType,
        path: h.source_path,
        tool: h.tool,
      }));
    
    const filtered = filterAndSort(items);
    if (filtered.length > 0) {
      sections.push({ id: 'hooks', title: 'Hooks', entityType: 'hook', items: filtered });
    }
  }
  
  return sections;
}

// ============================================================================
// Store Creation
// ============================================================================

export const useAppStore = create<AppState>((set, get) => ({
  // === Initial State ===
  isLoading: false,
  lastDiscovery: null,
  error: null,
  
  globalConfigPath: '',
  homeDirectory: '',
  
  projects: [],
  
  settings: [],
  memory: [],
  agents: [],
  skills: [],
  commands: [],
  hooks: [],
  plugins: [],
  mcpServers: [],
  
  duplicates: [],
  symlinks: [],
  
  activeView: 'dashboard',
  selectedEntity: null,
  searchQuery: '',
  filterScope: 'all',
  filterProject: null,
  filterTool: (typeof localStorage !== 'undefined' && localStorage.getItem('filterTool') as 'all' | ToolType) || 'all',
  isPanelOpen: false,
  theme: (typeof localStorage !== 'undefined' && localStorage.getItem('theme') as 'dark' | 'light') || 'dark',
  
  // === Global Search State ===
  isGlobalSearchOpen: false,
  globalSearchQuery: '',
  recentEntityIds: (typeof localStorage !== 'undefined' && JSON.parse(localStorage.getItem('recentEntityIds') || '[]')) || [],
  
  toasts: [],
  
  hasInitiallyAnimated: false,
  previousItemIds: new Set(),
  
  _cachedSections: null,
  
  // === Computed Getters ===
  
  getSections: () => {
    const state = get();
    if (state._cachedSections) {
      return state._cachedSections;
    }
    const sections = buildSections(state);
    // Note: We don't cache here to avoid stale data issues
    // Caching is handled in the actions that modify data
    return sections;
  },
  
  getFilteredSections: () => {
    return get().getSections();
  },
  
  getCurrentEntityList: () => {
    const { activeView, settings, memory, agents, skills, commands, hooks, plugins, mcpServers } = get();
    
    switch (activeView) {
      case 'settings':
        return settings as DisplayableEntity[];
      case 'memory':
        return memory as DisplayableEntity[];
      case 'agents':
        return agents as DisplayableEntity[];
      case 'skills':
        return skills as DisplayableEntity[];
      case 'commands':
        return commands as DisplayableEntity[];
      case 'hooks':
        return hooks as DisplayableEntity[];
      case 'plugins':
        return plugins as DisplayableEntity[];
      case 'mcp':
        return mcpServers as DisplayableEntity[];
      default:
        return [];
    }
  },
  
  isNewItem: (id: string) => {
    const { hasInitiallyAnimated, previousItemIds } = get();
    if (!hasInitiallyAnimated) return false;
    return !previousItemIds.has(id);
  },
  
  getEntityCounts: () => {
    const state = get();
    return {
      settings: state.settings.length,
      memory: state.memory.length,
      agent: state.agents.length,
      skill: state.skills.length,
      command: state.commands.length,
      hook: state.hooks.length,
      plugin: state.plugins.length,
      mcp: state.mcpServers.length,
    };
  },
  
  // === Discovery Actions ===
  
  discoverAll: async (projectPaths) => {
    const { previousItemIds, hasInitiallyAnimated, _cachedSections } = get();
    
    // Collect current item IDs for animation tracking
    const currentIds = new Set<string>();
    if (_cachedSections) {
      for (const section of _cachedSections) {
        for (const item of section.items) {
          currentIds.add(item.id);
        }
      }
    }
    
    const newPreviousIds = hasInitiallyAnimated ? currentIds : previousItemIds;
    
    set({ isLoading: true, error: null });
    
    try {
      // Get home directory first
      const homeDir = await getHomeDirectory();
      
      // Discover all entities
      const result: DiscoveryResult = await discoverAll(projectPaths);
      
      set({
        isLoading: false,
        lastDiscovery: result.discovered_at,
        globalConfigPath: result.global_config_path,
        homeDirectory: homeDir,
        projects: result.projects,
        settings: result.settings,
        memory: result.memory,
        agents: result.agents,
        skills: result.skills,
        commands: result.commands,
        hooks: result.hooks,
        plugins: result.plugins,
        mcpServers: result.mcp_servers,
        duplicates: result.duplicates,
        symlinks: result.symlinks,
        previousItemIds: newPreviousIds,
        _cachedSections: null, // Invalidate cache
      });
    } catch (err) {
      set({
        isLoading: false,
        error: err instanceof Error ? err.message : 'Failed to discover configurations',
      });
      
      get().addToast({
        type: 'error',
        title: 'Discovery Failed',
        message: err instanceof Error ? err.message : 'Unknown error',
        duration: 5000,
      });
    }
  },
  
  refreshDiscovery: async () => {
    const { projects } = get();
    const projectPaths = projects.map(p => p.path);
    await get().discoverAll(projectPaths.length > 0 ? projectPaths : undefined);
  },
  
  // === UI Actions ===
  
  setActiveView: (view) => {
    // Reset search and filters when changing views
    set({ 
      activeView: view, 
      selectedEntity: null, 
      isPanelOpen: false,
      searchQuery: '',
      filterScope: 'all',
      filterProject: null,
      _cachedSections: null,
    });
  },
  
  selectEntity: (entity) => {
    set({ selectedEntity: entity });
  },
  
  openPanel: (entity) => {
    set({ selectedEntity: entity, isPanelOpen: true });
  },
  
  closePanel: () => {
    set({ selectedEntity: null, isPanelOpen: false });
  },
  
  setSearchQuery: (query) => {
    set({ searchQuery: query, _cachedSections: null });
  },
  
  setFilterScope: (scope) => {
    set({ filterScope: scope, _cachedSections: null });
  },
  
  setFilterProject: (path) => {
    set({ filterProject: path, _cachedSections: null });
  },
  
  setActiveProject: (projectPath) => {
    // Navigate to project view with this project pre-selected
    set({
      activeView: 'project',
      filterProject: projectPath,
      selectedEntity: null,
      isPanelOpen: false,
      searchQuery: '',
      filterScope: 'all',
      _cachedSections: null,
    });
  },
  
  setFilterTool: (tool) => {
    localStorage.setItem('filterTool', tool);
    set({ filterTool: tool, _cachedSections: null });
  },
  
  setTheme: (theme) => {
    localStorage.setItem('theme', theme);
    document.documentElement.setAttribute('data-theme', theme);
    set({ theme });
  },
  
  toggleTheme: () => {
    const newTheme = get().theme === 'dark' ? 'light' : 'dark';
    localStorage.setItem('theme', newTheme);
    document.documentElement.setAttribute('data-theme', newTheme);
    set({ theme: newTheme });
  },
  
  // === Global Search Actions ===
  
  openGlobalSearch: () => {
    set({ isGlobalSearchOpen: true, globalSearchQuery: '' });
  },
  
  closeGlobalSearch: () => {
    set({ isGlobalSearchOpen: false, globalSearchQuery: '' });
  },
  
  setGlobalSearchQuery: (query) => {
    set({ globalSearchQuery: query });
  },
  
  addToRecentEntities: (id) => {
    const { recentEntityIds } = get();
    // Remove if already exists, then add to front
    const filtered = recentEntityIds.filter(existingId => existingId !== id);
    const updated = [id, ...filtered].slice(0, 5); // Keep max 5
    localStorage.setItem('recentEntityIds', JSON.stringify(updated));
    set({ recentEntityIds: updated });
  },
  
  // === Entity Actions ===
  
  updateEntityContent: (entityType, id, content) => {
    const state = get();
    
    switch (entityType) {
      case 'settings':
        set({
          settings: state.settings.map(s =>
            s.id === id ? { ...s, content } : s
          ),
          _cachedSections: null,
        });
        break;
      case 'memory':
        set({
          memory: state.memory.map(m =>
            m.id === id ? { ...m, content } : m
          ),
          _cachedSections: null,
        });
        break;
      case 'agent':
        set({
          agents: state.agents.map(a =>
            a.id === id ? { ...a, content } : a
          ),
          _cachedSections: null,
        });
        break;
      case 'skill':
        set({
          skills: state.skills.map(s =>
            s.id === id ? { ...s, content } : s
          ),
          _cachedSections: null,
        });
        break;
      case 'command':
        set({
          commands: state.commands.map(c =>
            c.id === id ? { ...c, content } : c
          ),
          _cachedSections: null,
        });
        break;
      case 'plugin':
        set({
          plugins: state.plugins.map(p =>
            p.id === id ? { ...p, content } : p
          ),
          _cachedSections: null,
        });
        break;
    }
  },
  
  removeEntity: (entityType, id) => {
    const state = get();
    const { previousItemIds } = state;
    
    const newPreviousIds = new Set(previousItemIds);
    newPreviousIds.delete(id);
    
    switch (entityType) {
      case 'settings':
        set({
          settings: state.settings.filter(s => s.id !== id),
          previousItemIds: newPreviousIds,
          _cachedSections: null,
        });
        break;
      case 'memory':
        set({
          memory: state.memory.filter(m => m.id !== id),
          previousItemIds: newPreviousIds,
          _cachedSections: null,
        });
        break;
      case 'agent':
        set({
          agents: state.agents.filter(a => a.id !== id),
          previousItemIds: newPreviousIds,
          _cachedSections: null,
        });
        break;
      case 'skill':
        set({
          skills: state.skills.filter(s => s.id !== id),
          previousItemIds: newPreviousIds,
          _cachedSections: null,
        });
        break;
      case 'command':
        set({
          commands: state.commands.filter(c => c.id !== id),
          previousItemIds: newPreviousIds,
          _cachedSections: null,
        });
        break;
      case 'plugin':
        set({
          plugins: state.plugins.filter(p => p.id !== id),
          previousItemIds: newPreviousIds,
          _cachedSections: null,
        });
        break;
      case 'mcp':
        set({
          mcpServers: state.mcpServers.filter(m => m.id !== id),
          previousItemIds: newPreviousIds,
          _cachedSections: null,
        });
        break;
      case 'hook':
        set({
          hooks: state.hooks.filter(h => h.id !== id),
          previousItemIds: newPreviousIds,
          _cachedSections: null,
        });
        break;
    }
  },
  
  // === Toast Actions ===
  
  addToast: (toast) => {
    const id = generateToastId();
    const newToast: Toast = { ...toast, id };
    
    set(state => ({
      toasts: [...state.toasts, newToast],
    }));
    
    // Auto-remove after duration
    if (toast.duration !== 0) {
      setTimeout(() => {
        get().removeToast(id);
      }, toast.duration || 4000);
    }
    
    return id;
  },
  
  removeToast: (id) => {
    set(state => ({
      toasts: state.toasts.filter(t => t.id !== id),
    }));
  },
  
  // === Animation Actions ===
  
  setHasInitiallyAnimated: (value) => {
    set({ hasInitiallyAnimated: value });
  },
  
  markItemsAsSeen: () => {
    const sections = get().getSections();
    const ids = new Set<string>();
    for (const section of sections) {
      for (const item of section.items) {
        ids.add(item.id);
      }
    }
    set({ previousItemIds: ids });
  },
  
  // === Error Handling ===
  
  setError: (error) => {
    set({ error });
  },
}));

// ============================================================================
// Selectors (for performance optimization)
// ============================================================================

export const selectIsLoading = (state: AppState) => state.isLoading;
export const selectError = (state: AppState) => state.error;
export const selectActiveView = (state: AppState) => state.activeView;
export const selectSelectedEntity = (state: AppState) => state.selectedEntity;
export const selectSearchQuery = (state: AppState) => state.searchQuery;
export const selectFilterTool = (state: AppState) => state.filterTool;
export const selectToasts = (state: AppState) => state.toasts;
export const selectDuplicates = (state: AppState) => state.duplicates;
export const selectSymlinks = (state: AppState) => state.symlinks;
export const selectProjects = (state: AppState) => state.projects;

// Entity selectors
export const selectSettings = (state: AppState) => state.settings;
export const selectMemory = (state: AppState) => state.memory;
export const selectAgents = (state: AppState) => state.agents;
export const selectSkills = (state: AppState) => state.skills;
export const selectCommands = (state: AppState) => state.commands;
export const selectHooks = (state: AppState) => state.hooks;
export const selectPlugins = (state: AppState) => state.plugins;
export const selectMcpServers = (state: AppState) => state.mcpServers;
