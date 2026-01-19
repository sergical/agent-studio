// ============================================================================
// CommandPalette - Entity list for the current view
// ============================================================================

import React, { useRef, useEffect, useMemo, useCallback, useState } from 'react';
import { motion, AnimatePresence } from 'motion/react';
import { 
  FolderOpen, 
  RefreshCw, 
  Plus,
  FileJson,
  FileText,
  Bot,
  Sparkles,
  Trash2,
  Terminal,
  Link,
  Package,
  Server,
  Filter
} from 'lucide-react';
import { useAppStore } from '../store/appStore';
import { SearchInput } from './ui/SearchInput';
import { CommandItem } from './ui/CommandItem';
import { ContextMenu, useContextMenu, type ContextMenuItem } from './ui/ContextMenu';
import { ConfirmDialog } from './ui/ConfirmDialog';
import { useKeyboardNavigation, useGlobalShortcuts } from '../hooks/useKeyboardNavigation';
import { deleteFile, deleteDirectory } from '../lib/api';
import type { EntityType, CommandPaletteItem, DisplayableEntity } from '../lib/types';
import type { LucideIcon } from 'lucide-react';

interface CommandPaletteProps {
  onOpenProject: () => void;
  onNewAgent: () => void;
  onNewSkill: () => void;
  onNewCommand?: () => void;
}

// Icon mapping for entity types
const ENTITY_ICONS: Record<EntityType, LucideIcon> = {
  settings: FileJson,
  memory: FileText,
  agent: Bot,
  skill: Sparkles,
  command: Terminal,
  hook: Link,
  plugin: Package,
  mcp: Server,
};

function getItemIcon(item: CommandPaletteItem): LucideIcon {
  return ENTITY_ICONS[item.entityType] || FileText;
}

function getItemIconColor(item: CommandPaletteItem): string {
  if (item.entityType === 'settings' || item.entityType === 'memory') {
    return 'var(--color-text-secondary)';
  }
  if (item.entityType === 'mcp') {
    return 'var(--color-info)';
  }
  return 'var(--color-claude)';
}

function getBadgeForItem(item: CommandPaletteItem): string | undefined {
  if (item.isSymlink) return 'symlink';
  if (item.isDuplicate) return 'duplicate';
  return undefined;  // Don't show project name as badge anymore
}

function getBadgeColor(item: CommandPaletteItem): 'default' | 'claude' | 'opencode' | 'success' | 'warning' {
  if (item.isSymlink) return 'warning';
  if (item.isDuplicate) return 'warning';
  return 'default';
}

function formatPath(path: string): string {
  return path.replace(/^\/Users\/[^/]+/, '~');
}

export function CommandPalette({ onOpenProject, onNewAgent, onNewSkill, onNewCommand }: CommandPaletteProps) {
  const searchRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  
  const activeView = useAppStore(state => state.activeView);
  const searchQuery = useAppStore(state => state.searchQuery);
  const setSearchQuery = useAppStore(state => state.setSearchQuery);
  const filterScope = useAppStore(state => state.filterScope);
  const setFilterScope = useAppStore(state => state.setFilterScope);
  const filterProject = useAppStore(state => state.filterProject);
  const projects = useAppStore(state => state.projects);
  const isLoading = useAppStore(state => state.isLoading);
  const refreshDiscovery = useAppStore(state => state.refreshDiscovery);
  const getSections = useAppStore(state => state.getSections);
  const openPanel = useAppStore(state => state.openPanel);
  const isPanelOpen = useAppStore(state => state.isPanelOpen);
  const selectedEntity = useAppStore(state => state.selectedEntity);
  const hasInitiallyAnimated = useAppStore(state => state.hasInitiallyAnimated);
  const setHasInitiallyAnimated = useAppStore(state => state.setHasInitiallyAnimated);
  const markItemsAsSeen = useAppStore(state => state.markItemsAsSeen);
  const isNewItem = useAppStore(state => state.isNewItem);
  const removeEntity = useAppStore(state => state.removeEntity);
  const addToast = useAppStore(state => state.addToast);
  
  // Get entities for current view
  const settings = useAppStore(state => state.settings);
  const memory = useAppStore(state => state.memory);
  const agents = useAppStore(state => state.agents);
  const skills = useAppStore(state => state.skills);
  const commands = useAppStore(state => state.commands);
  const hooks = useAppStore(state => state.hooks);
  const plugins = useAppStore(state => state.plugins);
  const mcpServers = useAppStore(state => state.mcpServers);
  
  const [selectedIndex, setSelectedIndex] = useState(0);
  
  // Context menu state
  const { contextMenu, close: closeContextMenu, handlers: contextMenuHandlers } = useContextMenu();
  const [deleteTarget, setDeleteTarget] = useState<{ item: CommandPaletteItem; entity: DisplayableEntity } | null>(null);
  const [isDeleting, setIsDeleting] = useState(false);
  
  // Get sections based on active view
  const sections = useMemo(() => {
    const allSections = getSections();
    
    // If on dashboard, show nothing (dashboard has its own view)
    if (activeView === 'dashboard') return [];
    
    // If on project view, show ALL sections (all entity types for this project)
    if (activeView === 'project') return allSections;
    
    // Filter to current view's entity type
    return allSections.filter(section => {
      switch (activeView) {
        case 'settings': return section.entityType === 'settings';
        case 'memory': return section.entityType === 'memory';
        case 'agents': return section.entityType === 'agent';
        case 'skills': return section.entityType === 'skill';
        case 'commands': return section.entityType === 'command';
        case 'hooks': return section.entityType === 'hook';
        case 'plugins': return section.entityType === 'plugin';
        case 'mcp': return section.entityType === 'mcp';
        default: return false;
      }
    });
  }, [getSections, activeView, filterScope, searchQuery]);
  
  // Flatten items for navigation
  const flatItems = useMemo(() => {
    const items: CommandPaletteItem[] = [];
    for (const section of sections) {
      items.push(...section.items);
    }
    return items;
  }, [sections]);
  
  // Find entity by id
  const findEntityById = useCallback((id: string, entityType: EntityType): DisplayableEntity | null => {
    switch (entityType) {
      case 'settings': return settings.find(s => s.id === id) || null;
      case 'memory': return memory.find(m => m.id === id) || null;
      case 'agent': return agents.find(a => a.id === id) || null;
      case 'skill': return skills.find(s => s.id === id) || null;
      case 'command': return commands.find(c => c.id === id) || null;
      case 'hook': return hooks.find(h => h.id === id) || null;
      case 'plugin': return plugins.find(p => p.id === id) || null;
      case 'mcp': return mcpServers.find(m => m.id === id) || null;
      default: return null;
    }
  }, [settings, memory, agents, skills, commands, hooks, plugins, mcpServers]);
  
  // Handle item selection
  const handleSelectItem = useCallback((index: number) => {
    const item = flatItems[index];
    if (!item) return;
    
    // Update selected index for visual highlight
    setSelectedIndex(index);
    
    const entity = findEntityById(item.id, item.entityType);
    if (entity) {
      openPanel(entity);
    }
  }, [flatItems, findEntityById, openPanel]);
  
  // Handle delete
  const handleDelete = useCallback(async () => {
    if (!deleteTarget) return;
    
    setIsDeleting(true);
    try {
      const { item, entity } = deleteTarget;
      
      // Optimistically remove
      removeEntity(item.entityType, item.id);
      setDeleteTarget(null);
      
      // Actually delete
      if (item.entityType === 'skill' && 'skill_dir' in entity) {
        await deleteDirectory((entity as any).skill_dir);
      } else if ('base' in entity) {
        await deleteFile((entity as any).base.path);
      }
      
      addToast({
        type: 'success',
        title: 'Deleted',
        message: `${item.name} has been deleted`,
      });
      
      // Refresh
      await refreshDiscovery();
    } catch (err) {
      console.error('Failed to delete:', err);
      addToast({
        type: 'error',
        title: 'Delete Failed',
        message: err instanceof Error ? err.message : 'Unknown error',
      });
      await refreshDiscovery();
    } finally {
      setIsDeleting(false);
    }
  }, [deleteTarget, removeEntity, refreshDiscovery, addToast]);
  
  // Context menu items
  const getContextMenuItems = useCallback((item: CommandPaletteItem): ContextMenuItem[] => {
    const entity = findEntityById(item.id, item.entityType);
    const menuItems: ContextMenuItem[] = [
      {
        id: 'open',
        label: 'Open',
        icon: FolderOpen,
        onClick: () => {
          if (entity) openPanel(entity);
        },
      },
    ];
    
    // Only certain entity types are deletable
    if (['agent', 'skill', 'command'].includes(item.entityType)) {
      menuItems.push(
        { id: 'separator', label: '', onClick: () => {}, separator: true },
        {
          id: 'delete',
          label: 'Delete',
          icon: Trash2,
          variant: 'danger',
          onClick: () => entity && setDeleteTarget({ item, entity }),
        }
      );
    }
    
    return menuItems;
  }, [findEntityById, openPanel]);
  
  // Keyboard navigation
  useKeyboardNavigation({
    itemCount: flatItems.length,
    selectedIndex,
    onSelectedIndexChange: setSelectedIndex,
    onSelect: handleSelectItem,
    onEscape: () => {
      if (searchQuery) {
        setSearchQuery('');
      }
    },
  });
  
  // Global shortcuts
  useGlobalShortcuts({
    onOpenSearch: () => searchRef.current?.focus(),
    onRefresh: refreshDiscovery,
    onNewAgent,
    onOpenProject,
  });
  
  // Scroll selected item into view
  useEffect(() => {
    if (listRef.current) {
      const selectedElement = listRef.current.querySelector(`[data-index="${selectedIndex}"]`);
      selectedElement?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    }
  }, [selectedIndex]);
  
  // Mark initial animation as complete
  useEffect(() => {
    if (!hasInitiallyAnimated && flatItems.length > 0) {
      const timer = setTimeout(() => {
        setHasInitiallyAnimated(true);
        markItemsAsSeen();
      }, flatItems.length * 20 + 200);
      
      return () => clearTimeout(timer);
    }
  }, [hasInitiallyAnimated, flatItems.length, setHasInitiallyAnimated, markItemsAsSeen]);
  
  // Reset selection when view changes (not when items change)
  useEffect(() => {
    setSelectedIndex(0);
  }, [activeView]);
  
  // Get title for current view
  const getViewTitle = () => {
    if (activeView === 'project') {
      if (filterProject) {
        const project = projects.find(p => p.path === filterProject);
        return project?.name || filterProject.split('/').pop() || 'Project';
      }
      return 'Project';
    }
    switch (activeView) {
      case 'settings': return 'Settings';
      case 'memory': return 'Memory Files';
      case 'agents': return 'Agents';
      case 'skills': return 'Skills';
      case 'commands': return 'Commands';
      case 'hooks': return 'Hooks';
      case 'plugins': return 'Plugins';
      case 'mcp': return 'MCP Servers';
      default: return 'Items';
    }
  };
  
  // Get new item action
  const getNewItemAction = () => {
    switch (activeView) {
      case 'agents': return { label: 'New Agent', onClick: onNewAgent };
      case 'skills': return { label: 'New Skill', onClick: onNewSkill };
      case 'commands': return onNewCommand ? { label: 'New Command', onClick: onNewCommand } : null;
      default: return null;
    }
  };
  
  const newItemAction = getNewItemAction();
  
  return (
    <>
      <div className="w-80 flex flex-col h-full bg-[var(--color-bg-primary)] border-r border-[var(--color-border)]">
        {/* Header */}
        <div className="p-3 border-b border-[var(--color-border)]">
          <div className="flex items-center justify-between mb-3">
            <span className="text-sm font-semibold text-[var(--color-text-primary)]">
              {getViewTitle()}
            </span>
            <div className="flex items-center gap-1">
              {newItemAction && (
                <button
                  onClick={newItemAction.onClick}
                  className="p-1.5 rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors"
                  title={newItemAction.label}
                >
                  <Plus className="w-4 h-4" />
                </button>
              )}
              <button
                onClick={refreshDiscovery}
                className="p-1.5 rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors"
                title="Refresh"
              >
                <RefreshCw className={`w-4 h-4 ${isLoading ? 'animate-spin' : ''}`} />
              </button>
            </div>
          </div>
          
          <SearchInput
            ref={searchRef}
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder={`Search ${activeView === 'project' ? 'project' : getViewTitle().toLowerCase()}...`}
          />
          
          {/* Scope Filter - hidden in project view */}
          {activeView !== 'project' && (
            <div className="flex items-center gap-1 mt-2">
              <Filter className="w-3 h-3 text-[var(--color-text-quaternary)]" />
              <div className="flex gap-1">
                {(['all', 'global', 'project'] as const).map((scope) => (
                  <button
                    key={scope}
                    onClick={() => setFilterScope(scope)}
                    className={`text-[10px] px-2 py-0.5 rounded transition-colors ${
                      filterScope === scope
                        ? 'bg-[var(--color-bg-active)] text-[var(--color-text-primary)]'
                        : 'text-[var(--color-text-tertiary)] hover:text-[var(--color-text-secondary)]'
                    }`}
                  >
                    {scope.charAt(0).toUpperCase() + scope.slice(1)}
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>
        
        {/* Items List */}
        <div ref={listRef} className="flex-1 overflow-y-auto py-2">
          {isLoading ? (
            <div className="flex items-center justify-center h-32">
              <RefreshCw className="w-5 h-5 text-[var(--color-text-tertiary)] animate-spin" />
            </div>
          ) : flatItems.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-32 text-center px-4">
              {React.createElement(ENTITY_ICONS[activeView === 'agents' ? 'agent' : activeView === 'skills' ? 'skill' : activeView === 'commands' ? 'command' : activeView === 'project' ? 'settings' : 'settings'], {
                className: 'w-8 h-8 text-[var(--color-text-quaternary)] mb-2'
              })}
              <p className="text-sm text-[var(--color-text-tertiary)]">
                {searchQuery ? 'No results found' : activeView === 'project' ? 'No entities in this project' : `No ${getViewTitle().toLowerCase()} found`}
              </p>
              {newItemAction && !searchQuery && (
                <button
                  onClick={newItemAction.onClick}
                  className="mt-2 text-xs text-[var(--color-accent)] hover:underline"
                >
                  Create your first {activeView.replace(/s$/, '')}
                </button>
              )}
            </div>
          ) : activeView === 'project' ? (
            // Project view: Show items grouped by section with headers
            <div className="px-2">
              {sections.map((section) => (
                <div key={section.id} className="mb-3">
                  <div className="px-2 py-1.5 text-[10px] font-medium text-[var(--color-text-tertiary)] uppercase tracking-wider">
                    {section.title}
                  </div>
                  <AnimatePresence mode="popLayout">
                    {section.items.map((item) => {
                      const globalIdx = flatItems.findIndex(i => i.id === item.id);
                      const isNew = isNewItem(item.id);
                      const shouldAnimate = !hasInitiallyAnimated || isNew;
                      
                      return (
                        <motion.div
                          key={item.id}
                          data-index={globalIdx}
                          layout
                          initial={shouldAnimate ? { opacity: 0, y: 8 } : false}
                          animate={{ opacity: 1, y: 0 }}
                          exit={{ opacity: 0, x: -20, transition: { duration: 0.15 } }}
                          transition={{ 
                            duration: 0.2,
                            delay: !hasInitiallyAnimated ? globalIdx * 0.02 : 0,
                          }}
                        >
                          <CommandItem
                            icon={getItemIcon(item)}
                            iconColor={getItemIconColor(item)}
                            label={item.name}
                            sublabel={item.description}
                            location={formatPath(item.path)}
                            scope={item.scope}
                            tool={item.tool}
                            badge={getBadgeForItem(item)}
                            badgeColor={getBadgeColor(item)}
                            isSelected={isPanelOpen && selectedEntity?.id === item.id}
                            hasChevron={true}
                            isDeletable={['agent', 'skill', 'command'].includes(item.entityType)}
                            isNew={isNew}
                            onClick={() => handleSelectItem(globalIdx)}
                            onDelete={() => {
                              const entity = findEntityById(item.id, item.entityType);
                              if (entity) setDeleteTarget({ item, entity });
                            }}
                            onContextMenu={(e) => contextMenuHandlers.onContextMenu(e, item)}
                            onPointerDown={(e) => contextMenuHandlers.onPointerDown(e, item)}
                            onPointerUp={contextMenuHandlers.onPointerUp}
                            onPointerCancel={contextMenuHandlers.onPointerCancel}
                            onPointerMove={contextMenuHandlers.onPointerMove}
                          />
                        </motion.div>
                      );
                    })}
                  </AnimatePresence>
                </div>
              ))}
            </div>
          ) : (
            <div className="px-2">
              <AnimatePresence mode="popLayout">
                {flatItems.map((item, idx) => {
                  const isNew = isNewItem(item.id);
                  const shouldAnimate = !hasInitiallyAnimated || isNew;
                  
                  return (
                    <motion.div
                      key={item.id}
                      data-index={idx}
                      layout
                      initial={shouldAnimate ? { opacity: 0, y: 8 } : false}
                      animate={{ opacity: 1, y: 0 }}
                      exit={{ opacity: 0, x: -20, transition: { duration: 0.15 } }}
                      transition={{ 
                        duration: 0.2,
                        delay: !hasInitiallyAnimated ? idx * 0.02 : 0,
                      }}
                    >
                      <CommandItem
                        icon={getItemIcon(item)}
                        iconColor={getItemIconColor(item)}
                        label={item.name}
                        sublabel={item.description}
                        location={formatPath(item.path)}
                        scope={item.scope}
                        tool={item.tool}
                        badge={getBadgeForItem(item)}
                        badgeColor={getBadgeColor(item)}
                        isSelected={isPanelOpen && selectedEntity?.id === item.id}
                        hasChevron={true}
                        isDeletable={['agent', 'skill', 'command'].includes(item.entityType)}
                        isNew={isNew}
                        onClick={() => handleSelectItem(idx)}
                        onDelete={() => {
                          const entity = findEntityById(item.id, item.entityType);
                          if (entity) setDeleteTarget({ item, entity });
                        }}
                        onContextMenu={(e) => contextMenuHandlers.onContextMenu(e, item)}
                        onPointerDown={(e) => contextMenuHandlers.onPointerDown(e, item)}
                        onPointerUp={contextMenuHandlers.onPointerUp}
                        onPointerCancel={contextMenuHandlers.onPointerCancel}
                        onPointerMove={contextMenuHandlers.onPointerMove}
                      />
                    </motion.div>
                  );
                })}
              </AnimatePresence>
            </div>
          )}
        </div>
      </div>
      
      {/* Context Menu */}
      <ContextMenu
        isOpen={contextMenu.isOpen}
        onClose={closeContextMenu}
        position={contextMenu.position}
        items={contextMenu.data ? getContextMenuItems(contextMenu.data as CommandPaletteItem) : []}
      />
      
      {/* Delete Confirmation Dialog */}
      <ConfirmDialog
        isOpen={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDelete}
        title={`Delete ${deleteTarget?.item.entityType}`}
        message={`Are you sure you want to delete "${deleteTarget?.item.name}"? This action cannot be undone.`}
        confirmLabel="Delete"
        cancelLabel="Cancel"
        variant="danger"
        isLoading={isDeleting}
      />
    </>
  );
}
