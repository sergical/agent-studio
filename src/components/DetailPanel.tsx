// ============================================================================
// DetailPanel - Entity detail view with editor
// ============================================================================

import { useState, useEffect, useCallback, lazy, Suspense } from 'react';
import { motion } from 'motion/react';
import { useAppStore } from '../store/appStore';
import { writeFile, deleteFile, deleteDirectory } from '../lib/api';
import { useGlobalShortcuts } from '../hooks/useKeyboardNavigation';
import { Panel } from './ui/Panel';
import { ConfirmDialog } from './ui/ConfirmDialog';
import type { DisplayableEntity, EntityType } from '../lib/types';
import { isFlatEntity, isHookEntity, isMcpServerEntity, isPluginEntity, isSkillEntity, isAgentEntity, isCommandEntity } from '../lib/types';
import { 
  Code, 
  FormInput, 
  Plus, 
  Trash2,
  FileText,
  Loader2,
  Link,
  Sparkles,
  Wrench,
  FileCode,
  Cpu,
  GitFork,
  User,
  Book,
  FolderOpen,
  ExternalLink,
  Bot,
  Shield,
  ShieldOff,
  ShieldCheck,
  ShieldAlert,
  Zap,
  Terminal,
  MessageSquare,
  Ban,
  Eye,
  Pencil
} from 'lucide-react';
import { clsx } from 'clsx';

// Lazy load Monaco editor for better initial load performance
const Editor = lazy(() => import('@monaco-editor/react'));

// Loading skeleton for Monaco
function EditorSkeleton() {
  return (
    <div className="h-full w-full flex items-center justify-center bg-[var(--color-bg-primary)]">
      <div className="flex flex-col items-center gap-3">
        <Loader2 className="w-8 h-8 text-[var(--color-text-tertiary)] animate-spin" />
        <span className="text-sm text-[var(--color-text-tertiary)]">Loading editor...</span>
      </div>
    </div>
  );
}

export function DetailPanel() {
  const selectedEntity = useAppStore(state => state.selectedEntity);
  const closePanel = useAppStore(state => state.closePanel);
  const refreshDiscovery = useAppStore(state => state.refreshDiscovery);
  const removeEntity = useAppStore(state => state.removeEntity);
  const updateEntityContent = useAppStore(state => state.updateEntityContent);
  const addToast = useAppStore(state => state.addToast);
  const theme = useAppStore(state => state.theme);
  
  const [content, setContent] = useState('');
  const [originalContent, setOriginalContent] = useState('');
  const [viewMode, setViewMode] = useState<'code' | 'form'>('code');
  const [editMode, setEditMode] = useState(false);
  const [isDeleting, setIsDeleting] = useState(false);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  
  const hasChanges = content !== originalContent;
  
  // Reset edit mode when entity changes
  useEffect(() => {
    setEditMode(false);
  }, [selectedEntity?.id]);
  
  // Get entity info
  const getEntityInfo = () => {
    if (!selectedEntity) return null;
    
    // Plugin entities need special handling
    if (isPluginEntity(selectedEntity)) {
      return {
        id: selectedEntity.id,
        name: selectedEntity.name,
        path: selectedEntity.path,
        scope: selectedEntity.scope,
        type: 'plugin' as EntityType,
        content: selectedEntity.content,
        isSymlink: selectedEntity.is_symlink,
        symlinkTarget: selectedEntity.symlink_target,
        isDeletable: false,
        isSkill: false,
        isPlugin: true,
      };
    }
    
    // Flat entities have fields directly on the object (not nested in .base)
    if (isFlatEntity(selectedEntity)) {
      return {
        id: selectedEntity.id,
        name: selectedEntity.name,
        path: selectedEntity.path,
        scope: selectedEntity.scope,
        type: selectedEntity.type as EntityType,
        content: selectedEntity.content,
        isSymlink: selectedEntity.is_symlink,
        symlinkTarget: selectedEntity.symlink_target,
        isDeletable: ['agent', 'skill', 'command'].includes(selectedEntity.type),
        isSkill: selectedEntity.type === 'skill',
        skillDir: selectedEntity.type === 'skill' ? (selectedEntity as any).skill_dir : undefined,
        isPlugin: false,
      };
    }
    
    if (isHookEntity(selectedEntity)) {
      return {
        id: selectedEntity.id,
        name: `${selectedEntity.event}${selectedEntity.matcher ? ` (${selectedEntity.matcher})` : ''}`,
        path: selectedEntity.source_path,
        scope: selectedEntity.source as 'global' | 'project',
        type: 'hook' as EntityType,
        content: JSON.stringify(selectedEntity, null, 2),
        isSymlink: false,
        isDeletable: false,
        isSkill: false,
        isPlugin: false,
      };
    }
    
    if (isMcpServerEntity(selectedEntity)) {
      return {
        id: selectedEntity.id,
        name: selectedEntity.name,
        path: selectedEntity.source_path,
        scope: selectedEntity.scope === 'project' ? 'project' : 'global' as 'global' | 'project',
        type: 'mcp' as EntityType,
        content: JSON.stringify(selectedEntity.config, null, 2),
        isSymlink: false,
        isDeletable: false,
        isSkill: false,
        isPlugin: false,
      };
    }
    
    return null;
  };
  
  const entityInfo = getEntityInfo();
  
  // Load content when entity changes
  useEffect(() => {
    if (entityInfo?.content) {
      setContent(entityInfo.content);
      setOriginalContent(entityInfo.content);
    } else {
      setContent('');
      setOriginalContent('');
    }
  }, [selectedEntity]);
  
  // Save handler
  const handleSave = useCallback(async () => {
    if (!entityInfo || !hasChanges) return;
    
    try {
      await writeFile(entityInfo.path, content);
      setOriginalContent(content);
      
      // Update local state
      updateEntityContent(entityInfo.type, entityInfo.id, content);
      
      addToast({
        type: 'success',
        title: 'Saved',
        message: `${entityInfo.name} saved successfully`,
      });
      
      // Refresh to sync any parsed data
      await refreshDiscovery();
    } catch (err) {
      console.error('Failed to save:', err);
      addToast({
        type: 'error',
        title: 'Save Failed',
        message: err instanceof Error ? err.message : 'Unknown error',
      });
    }
  }, [entityInfo, content, hasChanges, updateEntityContent, refreshDiscovery, addToast]);
  
  // Delete handler
  const handleDelete = useCallback(async () => {
    if (!entityInfo || !entityInfo.isDeletable) return;
    
    setIsDeleting(true);
    try {
      // Optimistically remove
      removeEntity(entityInfo.type, entityInfo.id);
      
      // Close panel first for better UX
      closePanel();
      setShowDeleteConfirm(false);
      
      // Actually delete
      if (entityInfo.isSkill && entityInfo.skillDir) {
        await deleteDirectory(entityInfo.skillDir);
      } else {
        await deleteFile(entityInfo.path);
      }
      
      addToast({
        type: 'success',
        title: 'Deleted',
        message: `${entityInfo.name} deleted successfully`,
      });
      
      // Refresh to sync state
      await refreshDiscovery();
    } catch (err) {
      console.error('Failed to delete:', err);
      addToast({
        type: 'error',
        title: 'Delete Failed',
        message: err instanceof Error ? err.message : 'Unknown error',
      });
      // Refresh to restore state on error
      await refreshDiscovery();
    } finally {
      setIsDeleting(false);
    }
  }, [entityInfo, closePanel, removeEntity, refreshDiscovery, addToast]);
  
  // Global save shortcut
  useGlobalShortcuts({
    onSave: handleSave,
  });
  
  if (!selectedEntity || !entityInfo) {
    return (
      <div className="flex-1 flex items-center justify-center bg-[var(--color-bg-primary)]">
        <div className="text-center">
          <div className="w-16 h-16 rounded-2xl bg-[var(--color-bg-secondary)] flex items-center justify-center mx-auto mb-4">
            <FileText className="w-8 h-8 text-[var(--color-text-quaternary)]" />
          </div>
          <p className="text-[var(--color-text-secondary)]">Select an item to view details</p>
          <p className="text-sm text-[var(--color-text-tertiary)] mt-1">
            Use arrow keys to navigate, Enter to select
          </p>
        </div>
      </div>
    );
  }
  
  // Determine file type
  const isJson = entityInfo.path.endsWith('.json');
  const isHook = entityInfo.type === 'hook';
  const isMcp = entityInfo.type === 'mcp';
  const isPlugin = entityInfo.type === 'plugin';
  const isSkill = entityInfo.type === 'skill';
  const isAgent = entityInfo.type === 'agent';
  const isCommand = entityInfo.type === 'command';
  
  const badge = entityInfo.scope;
  const badgeColor = entityInfo.scope === 'global' ? 'var(--color-info)' : 'var(--color-success)';
  
  return (
    <>
      <Panel
        isOpen={!!selectedEntity}
        onClose={closePanel}
        title={entityInfo.name}
        subtitle={entityInfo.path}
        badge={badge}
        badgeColor={badgeColor}
        hasChanges={hasChanges}
        onSave={handleSave}
        canDelete={entityInfo.isDeletable}
        onDelete={() => setShowDeleteConfirm(true)}
      >
        {/* Symlink indicator */}
        {entityInfo.isSymlink && (
          <div className="absolute top-0 left-0 right-0 flex items-center gap-2 px-4 py-2 border-b border-[var(--color-border)] bg-[var(--color-warning-soft)] z-10">
            <Link className="w-3.5 h-3.5 text-[var(--color-warning)]" />
            <span className="text-xs text-[var(--color-warning)]">
              Symlink → {entityInfo.symlinkTarget}
            </span>
          </div>
        )}
        
        {/* View/Edit Mode Toggle - for entities with special views */}
        {(isSkill || isAgent || isCommand) && (
          <div 
            className="absolute left-0 right-0 flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] bg-[var(--color-bg-primary)] z-10"
            style={{ top: entityInfo.isSymlink ? '33px' : '0' }}
          >
            <div className="flex items-center gap-2">
              <button
                onClick={() => setEditMode(false)}
                className={clsx(
                  'flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-md transition-colors',
                  !editMode
                    ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
                    : 'text-[var(--color-text-tertiary)] hover:text-[var(--color-text-secondary)]'
                )}
              >
                <Eye className="w-3.5 h-3.5" />
                View
              </button>
              <button
                onClick={() => setEditMode(true)}
                className={clsx(
                  'flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-md transition-colors',
                  editMode
                    ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
                    : 'text-[var(--color-text-tertiary)] hover:text-[var(--color-text-secondary)]'
                )}
              >
                <Pencil className="w-3.5 h-3.5" />
                Edit
              </button>
            </div>
            {hasChanges && (
              <span className="text-[10px] text-[var(--color-warning)] uppercase font-medium">
                Unsaved changes
              </span>
            )}
          </div>
        )}
        
        {/* View Mode Toggle - for JSON files only */}
        {isJson && !isHook && !isMcp && !isSkill && !isAgent && !isCommand && (
          <div 
            className="absolute left-0 right-0 flex items-center gap-2 px-4 py-2 border-b border-[var(--color-border)] bg-[var(--color-bg-primary)] z-10"
            style={{ top: entityInfo.isSymlink ? '33px' : '0' }}
          >
            <button
              onClick={() => setViewMode('code')}
              className={clsx(
                'flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-md transition-colors',
                viewMode === 'code'
                  ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-tertiary)] hover:text-[var(--color-text-secondary)]'
              )}
            >
              <Code className="w-3.5 h-3.5" />
              Code
            </button>
            <button
              onClick={() => setViewMode('form')}
              className={clsx(
                'flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-md transition-colors',
                viewMode === 'form'
                  ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-tertiary)] hover:text-[var(--color-text-secondary)]'
              )}
            >
              <FormInput className="w-3.5 h-3.5" />
              Form
            </button>
          </div>
        )}
        
        {/* Special views for Hook, MCP, Plugin (no edit mode) */}
        {isHook && <HookDetailView entity={selectedEntity} />}
        {isMcp && <McpDetailView entity={selectedEntity} />}
        {isPlugin && <PluginDetailView entity={selectedEntity} />}
        
        {/* Special views with edit mode toggle */}
        {isSkill && !editMode && <SkillDetailView entity={selectedEntity} />}
        {isAgent && !editMode && <AgentDetailView entity={selectedEntity} />}
        {isCommand && !editMode && <CommandDetailView entity={selectedEntity} />}
        
        {/* Editor for entities with edit mode */}
        {(isSkill || isAgent || isCommand) && editMode && (
          <div 
            className="absolute left-0 right-0 bottom-0"
            style={{ 
              top: 41 + (entityInfo.isSymlink ? 33 : 0) + 'px'
            }}
          >
            <Suspense fallback={<EditorSkeleton />}>
              <Editor
                height="100%"
                width="100%"
                language="markdown"
                  theme={theme === 'dark' ? 'vs-dark' : 'light'}
                value={content}
                onChange={(value) => setContent(value || '')}
                options={{
                  minimap: { enabled: false },
                  fontSize: 13,
                  fontFamily: '"SF Mono", "Fira Code", "Fira Mono", monospace',
                  lineNumbers: 'on',
                  scrollBeyondLastLine: false,
                  wordWrap: 'on',
                  formatOnPaste: true,
                  formatOnType: true,
                  tabSize: 2,
                  padding: { top: 16, bottom: 16 },
                  renderLineHighlight: 'none',
                  automaticLayout: true,
                  scrollbar: {
                    verticalScrollbarSize: 8,
                    horizontalScrollbarSize: 8,
                  },
                }}
              />
            </Suspense>
          </div>
        )}
        
        {/* Editor Container for other entities */}
        {!isHook && !isMcp && !isPlugin && !isSkill && !isAgent && !isCommand && (
          <div 
            className="absolute left-0 right-0 bottom-0"
            style={{ 
              top: (isJson ? 41 : 0) + (entityInfo.isSymlink ? 33 : 0) + 'px'
            }}
          >
            {viewMode === 'code' ? (
              <Suspense fallback={<EditorSkeleton />}>
                <Editor
                  height="100%"
                  width="100%"
                  language={isJson ? 'json' : 'markdown'}
                theme={theme === 'dark' ? 'vs-dark' : 'light'}
                  value={content}
                  onChange={(value) => setContent(value || '')}
                  options={{
                    minimap: { enabled: false },
                    fontSize: 13,
                    fontFamily: '"SF Mono", "Fira Code", "Fira Mono", monospace',
                    lineNumbers: 'on',
                    scrollBeyondLastLine: false,
                    wordWrap: 'on',
                    formatOnPaste: true,
                    formatOnType: true,
                    tabSize: 2,
                    padding: { top: 16, bottom: 16 },
                    renderLineHighlight: 'none',
                    automaticLayout: true,
                    scrollbar: {
                      verticalScrollbarSize: 8,
                      horizontalScrollbarSize: 8,
                    },
                  }}
                />
              </Suspense>
            ) : (
              <FormEditor 
                content={content} 
                onChange={setContent}
              />
            )}
          </div>
        )}
      </Panel>
      
      {/* Delete Confirmation Dialog */}
      <ConfirmDialog
        isOpen={showDeleteConfirm}
        onClose={() => setShowDeleteConfirm(false)}
        onConfirm={handleDelete}
        title={`Delete ${entityInfo.type}`}
        message={`Are you sure you want to delete "${entityInfo.name}"? This action cannot be undone.`}
        confirmLabel="Delete"
        cancelLabel="Cancel"
        variant="danger"
        isLoading={isDeleting}
      />
    </>
  );
}

// Hook detail view
function HookDetailView({ entity }: { entity: DisplayableEntity }) {
  if (!isHookEntity(entity)) return null;
  
  return (
    <div className="absolute inset-0 p-4 overflow-y-auto">
      <div className="space-y-4">
        <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
          <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
            Hook Configuration
          </h3>
          <div className="space-y-3">
            <div>
              <span className="text-xs text-[var(--color-text-tertiary)]">Event</span>
              <p className="text-sm text-[var(--color-text-primary)]">{entity.event}</p>
            </div>
            {entity.matcher && (
              <div>
                <span className="text-xs text-[var(--color-text-tertiary)]">Matcher</span>
                <p className="text-sm text-[var(--color-text-primary)] font-mono">{entity.matcher}</p>
              </div>
            )}
            <div>
              <span className="text-xs text-[var(--color-text-tertiary)]">Source</span>
              <p className="text-sm text-[var(--color-text-primary)]">{entity.source}</p>
            </div>
          </div>
        </div>
        
        <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
          <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
            Hook Actions ({entity.hooks.length})
          </h3>
          <div className="space-y-2">
            {entity.hooks.map((hook, idx) => (
              <div key={idx} className="p-3 bg-[var(--color-bg-tertiary)] rounded-md">
                <div className="flex items-center gap-2 mb-2">
                  <span className={clsx(
                    'text-[10px] px-1.5 py-0.5 rounded uppercase',
                    hook.type === 'command' 
                      ? 'bg-[var(--color-info-soft)] text-[var(--color-info)]'
                      : 'bg-[var(--color-accent-soft)] text-[var(--color-accent)]'
                  )}>
                    {hook.type}
                  </span>
                  {hook.timeout && (
                    <span className="text-[10px] text-[var(--color-text-tertiary)]">
                      {hook.timeout}ms timeout
                    </span>
                  )}
                </div>
                <code className="text-xs font-mono text-[var(--color-text-secondary)] break-all">
                  {hook.command || hook.prompt}
                </code>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

// MCP Server detail view
function McpDetailView({ entity }: { entity: DisplayableEntity }) {
  if (!isMcpServerEntity(entity)) return null;
  
  return (
    <div className="absolute inset-0 p-4 overflow-y-auto">
      <div className="space-y-4">
        <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
          <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
            Server Configuration
          </h3>
          <div className="space-y-3">
            <div>
              <span className="text-xs text-[var(--color-text-tertiary)]">Name</span>
              <p className="text-sm text-[var(--color-text-primary)]">{entity.name}</p>
            </div>
            <div>
              <span className="text-xs text-[var(--color-text-tertiary)]">Transport</span>
              <p className="text-sm text-[var(--color-text-primary)]">
                <span className={clsx(
                  'inline-block px-1.5 py-0.5 rounded text-[10px] uppercase',
                  entity.transport === 'stdio' && 'bg-[var(--color-success-soft)] text-[var(--color-success)]',
                  entity.transport === 'http' && 'bg-[var(--color-info-soft)] text-[var(--color-info)]',
                  entity.transport === 'sse' && 'bg-[var(--color-warning-soft)] text-[var(--color-warning)]',
                )}>
                  {entity.transport}
                </span>
              </p>
            </div>
            <div>
              <span className="text-xs text-[var(--color-text-tertiary)]">Scope</span>
              <p className="text-sm text-[var(--color-text-primary)]">{entity.scope}</p>
            </div>
            {entity.is_from_plugin && (
              <div>
                <span className="text-xs text-[var(--color-text-tertiary)]">Plugin</span>
                <p className="text-sm text-[var(--color-text-primary)]">{entity.plugin_name}</p>
              </div>
            )}
          </div>
        </div>
        
        {entity.config.command && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              Command
            </h3>
            <code className="text-xs font-mono text-[var(--color-text-secondary)] block p-2 bg-[var(--color-bg-tertiary)] rounded">
              {entity.config.command} {entity.config.args?.join(' ')}
            </code>
          </div>
        )}
        
        {entity.config.url && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              URL
            </h3>
            <code className="text-xs font-mono text-[var(--color-text-secondary)] block p-2 bg-[var(--color-bg-tertiary)] rounded break-all">
              {entity.config.url}
            </code>
          </div>
        )}
        
        {entity.config.env && Object.keys(entity.config.env).length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              Environment Variables
            </h3>
            <div className="space-y-1">
              {Object.entries(entity.config.env).map(([key, value]) => (
                <div key={key} className="flex gap-2 text-xs font-mono">
                  <span className="text-[var(--color-text-tertiary)]">{key}=</span>
                  <span className="text-[var(--color-text-secondary)]">{value}</span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// Plugin detail view
function PluginDetailView({ entity }: { entity: DisplayableEntity }) {
  if (!isPluginEntity(entity)) return null;
  
  const manifest = entity.manifest;
  const components = [
    { name: 'Commands', has: entity.has_commands, icon: '⌘' },
    { name: 'Agents', has: entity.has_agents, icon: '🤖' },
    { name: 'Skills', has: entity.has_skills, icon: '✨' },
    { name: 'Hooks', has: entity.has_hooks, icon: '🪝' },
    { name: 'MCP Servers', has: entity.has_mcp, icon: '🔌' },
    { name: 'LSP Servers', has: entity.has_lsp, icon: '📝' },
  ];
  
  const activeComponents = components.filter(c => c.has);
  
  return (
    <div className="absolute inset-0 p-4 overflow-y-auto">
      <div className="space-y-4">
        {/* Plugin Info */}
        <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
          <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
            Plugin Information
          </h3>
          <div className="space-y-3">
            <div>
              <span className="text-xs text-[var(--color-text-tertiary)]">Name</span>
              <p className="text-sm text-[var(--color-text-primary)] font-medium">{entity.name}</p>
            </div>
            {manifest?.description && (
              <div>
                <span className="text-xs text-[var(--color-text-tertiary)]">Description</span>
                <p className="text-sm text-[var(--color-text-secondary)]">{manifest.description}</p>
              </div>
            )}
            {manifest?.version && (
              <div>
                <span className="text-xs text-[var(--color-text-tertiary)]">Version</span>
                <p className="text-sm text-[var(--color-text-primary)]">{manifest.version}</p>
              </div>
            )}
            <div>
              <span className="text-xs text-[var(--color-text-tertiary)]">Scope</span>
              <p className="text-sm text-[var(--color-text-primary)]">
                <span className={clsx(
                  'inline-block px-1.5 py-0.5 rounded text-[10px] uppercase',
                  entity.scope === 'user' && 'bg-[var(--color-info-soft)] text-[var(--color-info)]',
                  entity.scope === 'project' && 'bg-[var(--color-success-soft)] text-[var(--color-success)]',
                  entity.scope === 'local' && 'bg-[var(--color-warning-soft)] text-[var(--color-warning)]',
                )}>
                  {entity.scope}
                </span>
              </p>
            </div>
            {entity.project_path && (
              <div>
                <span className="text-xs text-[var(--color-text-tertiary)]">Project</span>
                <p className="text-sm text-[var(--color-text-secondary)] font-mono">{entity.project_path}</p>
              </div>
            )}
          </div>
        </div>
        
        {/* Components */}
        {activeComponents.length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              Components ({activeComponents.length})
            </h3>
            <div className="flex flex-wrap gap-2">
              {activeComponents.map((comp) => (
                <span
                  key={comp.name}
                  className="inline-flex items-center gap-1.5 px-2.5 py-1.5 bg-[var(--color-bg-tertiary)] rounded-md text-xs text-[var(--color-text-secondary)]"
                >
                  <span>{comp.icon}</span>
                  {comp.name}
                </span>
              ))}
            </div>
          </div>
        )}
        
        {/* Author Info */}
        {manifest?.author && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              Author
            </h3>
            <div className="space-y-2">
              {typeof manifest.author === 'string' ? (
                <p className="text-sm text-[var(--color-text-primary)]">{manifest.author}</p>
              ) : (
                <>
                  <p className="text-sm text-[var(--color-text-primary)]">{manifest.author.name}</p>
                  {manifest.author.email && (
                    <p className="text-xs text-[var(--color-text-tertiary)]">{manifest.author.email}</p>
                  )}
                  {manifest.author.url && (
                    <a 
                      href={manifest.author.url} 
                      target="_blank" 
                      rel="noopener noreferrer"
                      className="text-xs text-[var(--color-accent)] hover:underline"
                    >
                      {manifest.author.url}
                    </a>
                  )}
                </>
              )}
            </div>
          </div>
        )}
        
        {/* Links */}
        {(manifest?.homepage || manifest?.repository) && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              Links
            </h3>
            <div className="space-y-2">
              {manifest.homepage && (
                <a 
                  href={manifest.homepage} 
                  target="_blank" 
                  rel="noopener noreferrer"
                  className="flex items-center gap-2 text-sm text-[var(--color-accent)] hover:underline"
                >
                  <span>🏠</span> Homepage
                </a>
              )}
              {manifest.repository && (
                <a 
                  href={manifest.repository} 
                  target="_blank" 
                  rel="noopener noreferrer"
                  className="flex items-center gap-2 text-sm text-[var(--color-accent)] hover:underline"
                >
                  <span>📦</span> Repository
                </a>
              )}
            </div>
          </div>
        )}
        
        {/* Keywords */}
        {manifest?.keywords && manifest.keywords.length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              Keywords
            </h3>
            <div className="flex flex-wrap gap-1.5">
              {manifest.keywords.map((keyword, idx) => (
                <span 
                  key={idx}
                  className="px-2 py-0.5 bg-[var(--color-bg-tertiary)] text-[var(--color-text-tertiary)] rounded text-xs"
                >
                  {keyword}
                </span>
              ))}
            </div>
          </div>
        )}
        
        {/* Install Path */}
        <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
          <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
            Install Path
          </h3>
          <code className="text-xs font-mono text-[var(--color-text-secondary)] block p-2 bg-[var(--color-bg-tertiary)] rounded break-all">
            {entity.plugin_dir || entity.path}
          </code>
        </div>
      </div>
    </div>
  );
}

// Skill detail view - Enhanced view with frontmatter parsing
function SkillDetailView({ entity }: { entity: DisplayableEntity }) {
  if (!isSkillEntity(entity)) return null;
  
  const frontmatter = entity.frontmatter;
  const supportingFiles = entity.supporting_files || [];
  
  // Get the markdown content (strip frontmatter for display)
  const getMarkdownContent = () => {
    const content = entity.content || '';
    // Match YAML frontmatter at the start of the file
    const frontmatterMatch = content.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
    if (frontmatterMatch) {
      return frontmatterMatch[2].trim();
    }
    return content;
  };
  
  const markdownContent = getMarkdownContent();
  
  // Normalize tools to array
  const getAllowedTools = () => {
    const tools = frontmatter?.['allowed-tools'];
    if (!tools) return [];
    if (Array.isArray(tools)) return tools;
    return [tools];
  };
  
  const allowedTools = getAllowedTools();
  
  // Model display
  const getModelDisplay = (model?: string) => {
    if (!model) return null;
    const modelMap: Record<string, { label: string; color: string }> = {
      'opus': { label: 'Opus', color: 'var(--color-accent)' },
      'sonnet': { label: 'Sonnet', color: 'var(--color-info)' },
      'haiku': { label: 'Haiku', color: 'var(--color-success)' },
      'inherit': { label: 'Inherit', color: 'var(--color-text-tertiary)' },
    };
    return modelMap[model] || { label: model, color: 'var(--color-text-secondary)' };
  };
  
  const modelInfo = getModelDisplay(frontmatter?.model);
  
  return (
    <div className="absolute inset-x-0 bottom-0 p-4 overflow-y-auto" style={{ top: '41px' }}>
      <div className="space-y-4">
        {/* Skill Info */}
        <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
          <div className="flex items-start gap-3 mb-3">
            <div className="w-10 h-10 rounded-lg bg-[var(--color-success-soft)] flex items-center justify-center flex-shrink-0">
              <Sparkles className="w-5 h-5 text-[var(--color-success)]" />
            </div>
            <div className="flex-1 min-w-0">
              <h3 className="text-sm font-semibold text-[var(--color-text-primary)]">
                {frontmatter?.name || entity.name}
              </h3>
              {frontmatter?.description && (
                <p className="text-xs text-[var(--color-text-secondary)] mt-1 line-clamp-2">
                  {frontmatter.description}
                </p>
              )}
            </div>
          </div>
          
          {/* Quick Stats */}
          <div className="flex flex-wrap gap-2 mt-3 pt-3 border-t border-[var(--color-border-subtle)]">
            {frontmatter?.['user-invocable'] && (
              <span className="inline-flex items-center gap-1 px-2 py-1 bg-[var(--color-info-soft)] text-[var(--color-info)] rounded text-[10px] font-medium uppercase">
                <User className="w-3 h-3" />
                User Invocable
              </span>
            )}
            {frontmatter?.context === 'fork' && (
              <span className="inline-flex items-center gap-1 px-2 py-1 bg-[var(--color-warning-soft)] text-[var(--color-warning)] rounded text-[10px] font-medium uppercase">
                <GitFork className="w-3 h-3" />
                Forked Context
              </span>
            )}
            {modelInfo && (
              <span 
                className="inline-flex items-center gap-1 px-2 py-1 rounded text-[10px] font-medium uppercase"
                style={{ 
                  backgroundColor: `color-mix(in srgb, ${modelInfo.color} 15%, transparent)`,
                  color: modelInfo.color
                }}
              >
                <Cpu className="w-3 h-3" />
                {modelInfo.label}
              </span>
            )}
            {frontmatter?.['disable-model-invocation'] && (
              <span className="inline-flex items-center gap-1 px-2 py-1 bg-[var(--color-error-soft)] text-[var(--color-error)] rounded text-[10px] font-medium uppercase">
                No Auto-Invoke
              </span>
            )}
          </div>
        </div>
        
        {/* Allowed Tools */}
        {allowedTools.length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              <Wrench className="w-3.5 h-3.5" />
              Allowed Tools ({allowedTools.length})
            </h3>
            <div className="flex flex-wrap gap-1.5">
              {allowedTools.map((tool, idx) => (
                <span
                  key={idx}
                  className="px-2 py-1 bg-[var(--color-bg-tertiary)] text-[var(--color-text-secondary)] rounded text-xs font-mono"
                >
                  {tool}
                </span>
              ))}
            </div>
          </div>
        )}
        
        {/* Supporting Files */}
        {supportingFiles.length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              <FolderOpen className="w-3.5 h-3.5" />
              Supporting Files ({supportingFiles.length})
            </h3>
            <div className="space-y-1.5">
              {supportingFiles.map((file, idx) => {
                const fileName = file.split('/').pop() || file;
                const isMarkdown = file.endsWith('.md');
                const isScript = file.endsWith('.sh') || file.endsWith('.py') || file.endsWith('.js') || file.endsWith('.ts');
                
                return (
                  <div
                    key={idx}
                    className="flex items-center gap-2 px-2.5 py-1.5 bg-[var(--color-bg-tertiary)] rounded text-xs group"
                  >
                    {isMarkdown ? (
                      <Book className="w-3.5 h-3.5 text-[var(--color-info)]" />
                    ) : isScript ? (
                      <FileCode className="w-3.5 h-3.5 text-[var(--color-warning)]" />
                    ) : (
                      <FileText className="w-3.5 h-3.5 text-[var(--color-text-tertiary)]" />
                    )}
                    <span className="font-mono text-[var(--color-text-secondary)] truncate flex-1">
                      {fileName}
                    </span>
                    <ExternalLink className="w-3 h-3 text-[var(--color-text-quaternary)] opacity-0 group-hover:opacity-100 transition-opacity" />
                  </div>
                );
              })}
            </div>
          </div>
        )}
        
        {/* Hooks */}
        {frontmatter?.hooks && Object.keys(frontmatter.hooks).length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              Hooks ({Object.keys(frontmatter.hooks).length} events)
            </h3>
            <div className="space-y-2">
              {Object.entries(frontmatter.hooks).map(([event, matchers]) => (
                <div key={event} className="p-2.5 bg-[var(--color-bg-tertiary)] rounded-md">
                  <span className="text-xs font-medium text-[var(--color-accent)]">{event}</span>
                  {Array.isArray(matchers) && (
                    <span className="ml-2 text-[10px] text-[var(--color-text-quaternary)]">
                      {matchers.length} {matchers.length === 1 ? 'matcher' : 'matchers'}
                    </span>
                  )}
                </div>
              ))}
            </div>
          </div>
        )}
        
        {/* Metadata */}
        {frontmatter?.metadata && Object.keys(frontmatter.metadata).length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              Metadata
            </h3>
            <div className="space-y-1.5">
              {Object.entries(frontmatter.metadata).map(([key, value]) => (
                <div key={key} className="flex gap-2 text-xs">
                  <span className="text-[var(--color-text-tertiary)]">{key}:</span>
                  <span className="text-[var(--color-text-secondary)]">{value}</span>
                </div>
              ))}
            </div>
          </div>
        )}
        
        {/* License & Compatibility */}
        {(frontmatter?.license || frontmatter?.compatibility) && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              Additional Info
            </h3>
            <div className="space-y-2">
              {frontmatter.license && (
                <div>
                  <span className="text-xs text-[var(--color-text-tertiary)]">License</span>
                  <p className="text-sm text-[var(--color-text-primary)]">{frontmatter.license}</p>
                </div>
              )}
              {frontmatter.compatibility && (
                <div>
                  <span className="text-xs text-[var(--color-text-tertiary)]">Compatibility</span>
                  <p className="text-sm text-[var(--color-text-primary)]">{frontmatter.compatibility}</p>
                </div>
              )}
            </div>
          </div>
        )}
        
        {/* Skill Directory */}
        <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
          <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
            Skill Directory
          </h3>
          <code className="text-xs font-mono text-[var(--color-text-secondary)] block p-2 bg-[var(--color-bg-tertiary)] rounded break-all">
            {entity.skill_dir}
          </code>
        </div>
        
        {/* Markdown Content Preview */}
        {markdownContent && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              Instructions Preview
            </h3>
            <div className="text-xs text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono p-3 bg-[var(--color-bg-tertiary)] rounded max-h-64 overflow-y-auto">
              {markdownContent.slice(0, 1500)}
              {markdownContent.length > 1500 && (
                <span className="text-[var(--color-text-quaternary)]">
                  {'\n\n'}... ({markdownContent.length - 1500} more characters)
                </span>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// Agent detail view - Enhanced view with frontmatter parsing
function AgentDetailView({ entity }: { entity: DisplayableEntity }) {
  if (!isAgentEntity(entity)) return null;
  
  const frontmatter = entity.frontmatter;
  
  // Get the markdown content (strip frontmatter for display)
  const getMarkdownContent = () => {
    const content = entity.content || '';
    const frontmatterMatch = content.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
    if (frontmatterMatch) {
      return frontmatterMatch[2].trim();
    }
    return content;
  };
  
  const markdownContent = getMarkdownContent();
  
  // Normalize tools to array
  const getTools = (tools?: string | string[]) => {
    if (!tools) return [];
    if (Array.isArray(tools)) return tools;
    return [tools];
  };
  
  const allowedTools = getTools(frontmatter?.tools);
  const disallowedTools = getTools(frontmatter?.disallowedTools);
  const skills = frontmatter?.skills || [];
  
  // Model display
  const getModelDisplay = (model?: string) => {
    if (!model) return null;
    const modelMap: Record<string, { label: string; color: string }> = {
      'opus': { label: 'Opus', color: 'var(--color-accent)' },
      'sonnet': { label: 'Sonnet', color: 'var(--color-info)' },
      'haiku': { label: 'Haiku', color: 'var(--color-success)' },
      'inherit': { label: 'Inherit', color: 'var(--color-text-tertiary)' },
    };
    return modelMap[model] || { label: model, color: 'var(--color-text-secondary)' };
  };
  
  // Permission mode display
  const getPermissionModeDisplay = (mode?: string) => {
    if (!mode) return null;
    const modeMap: Record<string, { label: string; color: string; icon: typeof Shield }> = {
      'default': { label: 'Default', color: 'var(--color-text-tertiary)', icon: Shield },
      'acceptEdits': { label: 'Accept Edits', color: 'var(--color-success)', icon: ShieldCheck },
      'dontAsk': { label: "Don't Ask", color: 'var(--color-warning)', icon: ShieldOff },
      'bypassPermissions': { label: 'Bypass', color: 'var(--color-error)', icon: ShieldAlert },
      'plan': { label: 'Plan Mode', color: 'var(--color-info)', icon: Shield },
    };
    return modeMap[mode] || { label: mode, color: 'var(--color-text-secondary)', icon: Shield };
  };
  
  const modelInfo = getModelDisplay(frontmatter?.model);
  const permissionInfo = getPermissionModeDisplay(frontmatter?.permissionMode);
  
  return (
    <div className="absolute inset-x-0 bottom-0 p-4 overflow-y-auto" style={{ top: '41px' }}>
      <div className="space-y-4">
        {/* Agent Info */}
        <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
          <div className="flex items-start gap-3 mb-3">
            <div className="w-10 h-10 rounded-lg bg-[var(--color-warning-soft)] flex items-center justify-center flex-shrink-0">
              <Bot className="w-5 h-5 text-[var(--color-warning)]" />
            </div>
            <div className="flex-1 min-w-0">
              <h3 className="text-sm font-semibold text-[var(--color-text-primary)]">
                {frontmatter?.name || entity.name}
              </h3>
              {frontmatter?.description && (
                <p className="text-xs text-[var(--color-text-secondary)] mt-1 line-clamp-2">
                  {frontmatter.description}
                </p>
              )}
            </div>
          </div>
          
          {/* Quick Stats */}
          <div className="flex flex-wrap gap-2 mt-3 pt-3 border-t border-[var(--color-border-subtle)]">
            {modelInfo && (
              <span 
                className="inline-flex items-center gap-1 px-2 py-1 rounded text-[10px] font-medium uppercase"
                style={{ 
                  backgroundColor: `color-mix(in srgb, ${modelInfo.color} 15%, transparent)`,
                  color: modelInfo.color
                }}
              >
                <Cpu className="w-3 h-3" />
                {modelInfo.label}
              </span>
            )}
            {permissionInfo && (
              <span 
                className="inline-flex items-center gap-1 px-2 py-1 rounded text-[10px] font-medium uppercase"
                style={{ 
                  backgroundColor: `color-mix(in srgb, ${permissionInfo.color} 15%, transparent)`,
                  color: permissionInfo.color
                }}
              >
                <permissionInfo.icon className="w-3 h-3" />
                {permissionInfo.label}
              </span>
            )}
            {allowedTools.length > 0 && (
              <span className="inline-flex items-center gap-1 px-2 py-1 bg-[var(--color-success-soft)] text-[var(--color-success)] rounded text-[10px] font-medium uppercase">
                <Wrench className="w-3 h-3" />
                {allowedTools.length} Tools
              </span>
            )}
            {disallowedTools.length > 0 && (
              <span className="inline-flex items-center gap-1 px-2 py-1 bg-[var(--color-error-soft)] text-[var(--color-error)] rounded text-[10px] font-medium uppercase">
                <Ban className="w-3 h-3" />
                {disallowedTools.length} Blocked
              </span>
            )}
            {skills.length > 0 && (
              <span className="inline-flex items-center gap-1 px-2 py-1 bg-[var(--color-accent-soft)] text-[var(--color-accent)] rounded text-[10px] font-medium uppercase">
                <Sparkles className="w-3 h-3" />
                {skills.length} Skills
              </span>
            )}
          </div>
        </div>
        
        {/* Allowed Tools */}
        {allowedTools.length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              <Wrench className="w-3.5 h-3.5" />
              Allowed Tools ({allowedTools.length})
            </h3>
            <div className="flex flex-wrap gap-1.5">
              {allowedTools.map((tool, idx) => (
                <span
                  key={idx}
                  className="px-2 py-1 bg-[var(--color-success-soft)] text-[var(--color-success)] rounded text-xs font-mono"
                >
                  {tool}
                </span>
              ))}
            </div>
          </div>
        )}
        
        {/* Disallowed Tools */}
        {disallowedTools.length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              <Ban className="w-3.5 h-3.5" />
              Disallowed Tools ({disallowedTools.length})
            </h3>
            <div className="flex flex-wrap gap-1.5">
              {disallowedTools.map((tool, idx) => (
                <span
                  key={idx}
                  className="px-2 py-1 bg-[var(--color-error-soft)] text-[var(--color-error)] rounded text-xs font-mono"
                >
                  {tool}
                </span>
              ))}
            </div>
          </div>
        )}
        
        {/* Skills */}
        {skills.length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              <Sparkles className="w-3.5 h-3.5" />
              Skills ({skills.length})
            </h3>
            <div className="space-y-1.5">
              {skills.map((skill, idx) => (
                <div
                  key={idx}
                  className="flex items-center gap-2 px-2.5 py-1.5 bg-[var(--color-bg-tertiary)] rounded text-xs"
                >
                  <Sparkles className="w-3.5 h-3.5 text-[var(--color-accent)]" />
                  <span className="text-[var(--color-text-secondary)]">{skill}</span>
                </div>
              ))}
            </div>
          </div>
        )}
        
        {/* Hooks */}
        {frontmatter?.hooks && Object.keys(frontmatter.hooks).length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              <Zap className="w-3.5 h-3.5" />
              Hooks ({Object.keys(frontmatter.hooks).length} events)
            </h3>
            <div className="space-y-2">
              {Object.entries(frontmatter.hooks).map(([event, matchers]) => (
                <div key={event} className="p-2.5 bg-[var(--color-bg-tertiary)] rounded-md">
                  <span className="text-xs font-medium text-[var(--color-accent)]">{event}</span>
                  {Array.isArray(matchers) && (
                    <span className="ml-2 text-[10px] text-[var(--color-text-quaternary)]">
                      {matchers.length} {matchers.length === 1 ? 'matcher' : 'matchers'}
                    </span>
                  )}
                </div>
              ))}
            </div>
          </div>
        )}
        
        {/* Agent File Path */}
        <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
          <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
            File Path
          </h3>
          <code className="text-xs font-mono text-[var(--color-text-secondary)] block p-2 bg-[var(--color-bg-tertiary)] rounded break-all">
            {entity.path}
          </code>
        </div>
        
        {/* Markdown Content Preview */}
        {markdownContent && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              Instructions Preview
            </h3>
            <div className="text-xs text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono p-3 bg-[var(--color-bg-tertiary)] rounded max-h-64 overflow-y-auto">
              {markdownContent.slice(0, 1500)}
              {markdownContent.length > 1500 && (
                <span className="text-[var(--color-text-quaternary)]">
                  {'\n\n'}... ({markdownContent.length - 1500} more characters)
                </span>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// Command detail view - Enhanced view for slash commands
function CommandDetailView({ entity }: { entity: DisplayableEntity }) {
  if (!isCommandEntity(entity)) return null;
  
  const frontmatter = entity.frontmatter;
  
  // Get the markdown content (strip frontmatter for display)
  const getMarkdownContent = () => {
    const content = entity.content || '';
    const frontmatterMatch = content.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
    if (frontmatterMatch) {
      return frontmatterMatch[2].trim();
    }
    return content;
  };
  
  const markdownContent = getMarkdownContent();
  
  // Normalize tools to array
  const getAllowedTools = () => {
    const tools = frontmatter?.['allowed-tools'];
    if (!tools) return [];
    if (Array.isArray(tools)) return tools;
    return [tools];
  };
  
  const allowedTools = getAllowedTools();
  
  // Model display
  const getModelDisplay = (model?: string) => {
    if (!model) return null;
    const modelMap: Record<string, { label: string; color: string }> = {
      'opus': { label: 'Opus', color: 'var(--color-accent)' },
      'sonnet': { label: 'Sonnet', color: 'var(--color-info)' },
      'haiku': { label: 'Haiku', color: 'var(--color-success)' },
      'inherit': { label: 'Inherit', color: 'var(--color-text-tertiary)' },
    };
    return modelMap[model] || { label: model, color: 'var(--color-text-secondary)' };
  };
  
  const modelInfo = getModelDisplay(frontmatter?.model);
  
  // Build command name with namespace
  const commandName = entity.namespace 
    ? `/${entity.namespace}:${entity.name.replace('.md', '')}`
    : `/${entity.name.replace('.md', '')}`;
  
  return (
    <div className="absolute inset-x-0 bottom-0 p-4 overflow-y-auto" style={{ top: '41px' }}>
      <div className="space-y-4">
        {/* Command Info */}
        <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
          <div className="flex items-start gap-3 mb-3">
            <div className="w-10 h-10 rounded-lg bg-[var(--color-info-soft)] flex items-center justify-center flex-shrink-0">
              <Terminal className="w-5 h-5 text-[var(--color-info)]" />
            </div>
            <div className="flex-1 min-w-0">
              <h3 className="text-sm font-semibold text-[var(--color-text-primary)] font-mono">
                {commandName}
                {frontmatter?.['argument-hint'] && (
                  <span className="text-[var(--color-text-tertiary)] font-normal ml-1">
                    {frontmatter['argument-hint']}
                  </span>
                )}
              </h3>
              {frontmatter?.description && (
                <p className="text-xs text-[var(--color-text-secondary)] mt-1 line-clamp-2">
                  {frontmatter.description}
                </p>
              )}
            </div>
          </div>
          
          {/* Quick Stats */}
          <div className="flex flex-wrap gap-2 mt-3 pt-3 border-t border-[var(--color-border-subtle)]">
            {entity.namespace && (
              <span className="inline-flex items-center gap-1 px-2 py-1 bg-[var(--color-bg-tertiary)] text-[var(--color-text-secondary)] rounded text-[10px] font-medium uppercase">
                <FolderOpen className="w-3 h-3" />
                {entity.namespace}
              </span>
            )}
            {modelInfo && (
              <span 
                className="inline-flex items-center gap-1 px-2 py-1 rounded text-[10px] font-medium uppercase"
                style={{ 
                  backgroundColor: `color-mix(in srgb, ${modelInfo.color} 15%, transparent)`,
                  color: modelInfo.color
                }}
              >
                <Cpu className="w-3 h-3" />
                {modelInfo.label}
              </span>
            )}
            {frontmatter?.context === 'fork' && (
              <span className="inline-flex items-center gap-1 px-2 py-1 bg-[var(--color-warning-soft)] text-[var(--color-warning)] rounded text-[10px] font-medium uppercase">
                <GitFork className="w-3 h-3" />
                Forked Context
              </span>
            )}
            {frontmatter?.agent && (
              <span className="inline-flex items-center gap-1 px-2 py-1 bg-[var(--color-warning-soft)] text-[var(--color-warning)] rounded text-[10px] font-medium uppercase">
                <Bot className="w-3 h-3" />
                Uses Agent
              </span>
            )}
            {frontmatter?.['disable-model-invocation'] && (
              <span className="inline-flex items-center gap-1 px-2 py-1 bg-[var(--color-error-soft)] text-[var(--color-error)] rounded text-[10px] font-medium uppercase">
                No Auto-Invoke
              </span>
            )}
          </div>
        </div>
        
        {/* Agent Reference */}
        {frontmatter?.agent && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              <Bot className="w-3.5 h-3.5" />
              Delegated Agent
            </h3>
            <div className="flex items-center gap-2 px-2.5 py-2 bg-[var(--color-bg-tertiary)] rounded">
              <Bot className="w-4 h-4 text-[var(--color-warning)]" />
              <span className="text-sm text-[var(--color-text-primary)]">{frontmatter.agent}</span>
            </div>
            <p className="text-[10px] text-[var(--color-text-quaternary)] mt-2">
              This command delegates execution to the specified agent
            </p>
          </div>
        )}
        
        {/* Allowed Tools */}
        {allowedTools.length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              <Wrench className="w-3.5 h-3.5" />
              Allowed Tools ({allowedTools.length})
            </h3>
            <div className="flex flex-wrap gap-1.5">
              {allowedTools.map((tool, idx) => (
                <span
                  key={idx}
                  className="px-2 py-1 bg-[var(--color-bg-tertiary)] text-[var(--color-text-secondary)] rounded text-xs font-mono"
                >
                  {tool}
                </span>
              ))}
            </div>
          </div>
        )}
        
        {/* Hooks */}
        {frontmatter?.hooks && Object.keys(frontmatter.hooks).length > 0 && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              <Zap className="w-3.5 h-3.5" />
              Hooks ({Object.keys(frontmatter.hooks).length} events)
            </h3>
            <div className="space-y-2">
              {Object.entries(frontmatter.hooks).map(([event, matchers]) => (
                <div key={event} className="p-2.5 bg-[var(--color-bg-tertiary)] rounded-md">
                  <span className="text-xs font-medium text-[var(--color-accent)]">{event}</span>
                  {Array.isArray(matchers) && (
                    <span className="ml-2 text-[10px] text-[var(--color-text-quaternary)]">
                      {matchers.length} {matchers.length === 1 ? 'matcher' : 'matchers'}
                    </span>
                  )}
                </div>
              ))}
            </div>
          </div>
        )}
        
        {/* File Path */}
        <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
          <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
            File Path
          </h3>
          <code className="text-xs font-mono text-[var(--color-text-secondary)] block p-2 bg-[var(--color-bg-tertiary)] rounded break-all">
            {entity.path}
          </code>
        </div>
        
        {/* Markdown Content Preview */}
        {markdownContent && (
          <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
            <h3 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
              <MessageSquare className="w-3.5 h-3.5" />
              Prompt Template
            </h3>
            <div className="text-xs text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono p-3 bg-[var(--color-bg-tertiary)] rounded max-h-64 overflow-y-auto">
              {markdownContent.slice(0, 1500)}
              {markdownContent.length > 1500 && (
                <span className="text-[var(--color-text-quaternary)]">
                  {'\n\n'}... ({markdownContent.length - 1500} more characters)
                </span>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// Form-based editor for JSON files
function FormEditor({ 
  content, 
  onChange, 
}: { 
  content: string; 
  onChange: (content: string) => void;
}) {
  const [data, setData] = useState<Record<string, unknown>>({});
  
  useEffect(() => {
    try {
      setData(JSON.parse(content));
    } catch {
      setData({});
    }
  }, [content]);
  
  const updateData = (path: string[], value: unknown) => {
    const newData = { ...data };
    let current: Record<string, unknown> = newData;
    
    for (let i = 0; i < path.length - 1; i++) {
      if (!current[path[i]]) {
        current[path[i]] = {};
      }
      current = current[path[i]] as Record<string, unknown>;
    }
    
    current[path[path.length - 1]] = value;
    setData(newData);
    onChange(JSON.stringify(newData, null, 2));
  };
  
  const permissions = (data.permissions as Record<string, unknown>) || {};
  const allowRules = (permissions.allow as string[]) || [];
  const denyRules = (permissions.deny as string[]) || [];
  
  return (
    <div className="p-4 space-y-6 overflow-y-auto h-full">
      {/* Permissions Section */}
      <FormSection title="Permissions">
        <RulesList
          label="Allow Rules"
          rules={allowRules}
          type="allow"
          onAdd={(rule) => updateData(['permissions', 'allow'], [...allowRules, rule])}
          onRemove={(idx) => updateData(['permissions', 'allow'], allowRules.filter((_, i) => i !== idx))}
        />
        <RulesList
          label="Deny Rules"
          rules={denyRules}
          type="deny"
          onAdd={(rule) => updateData(['permissions', 'deny'], [...denyRules, rule])}
          onRemove={(idx) => updateData(['permissions', 'deny'], denyRules.filter((_, i) => i !== idx))}
        />
      </FormSection>
      
      {/* Model Section */}
      <FormSection title="Model">
        <FormField
          label="Model"
          value={(data.model as string) || ''}
          onChange={(value) => updateData(['model'], value)}
          placeholder="claude-sonnet-4-5-20250929"
        />
      </FormSection>
    </div>
  );
}

function FormSection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="bg-[var(--color-bg-secondary)] rounded-lg border border-[var(--color-border)] p-4">
      <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--color-text-tertiary)] mb-3">
        {title}
      </h3>
      <div className="space-y-4">
        {children}
      </div>
    </div>
  );
}

function FormField({ 
  label, 
  value, 
  onChange, 
  placeholder 
}: { 
  label: string; 
  value: string; 
  onChange: (value: string) => void;
  placeholder?: string;
}) {
  return (
    <div>
      <label className="block text-xs text-[var(--color-text-secondary)] mb-1.5">{label}</label>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="w-full px-3 py-2 text-sm bg-[var(--color-bg-tertiary)] text-[var(--color-text-primary)] 
          rounded-md border border-[var(--color-border)] 
          focus:outline-none focus:border-[var(--color-border-focus)]
          placeholder:text-[var(--color-text-quaternary)]"
      />
    </div>
  );
}

function RulesList({ 
  label, 
  rules, 
  type,
  onAdd, 
  onRemove 
}: { 
  label: string;
  rules: string[]; 
  type: 'allow' | 'deny';
  onAdd: (rule: string) => void;
  onRemove: (index: number) => void;
}) {
  const [newRule, setNewRule] = useState('');
  
  const colors = {
    allow: 'bg-[var(--color-success-soft)] text-[var(--color-success)] border-[var(--color-success)]/20',
    deny: 'bg-[var(--color-error-soft)] text-[var(--color-error)] border-[var(--color-error)]/20',
  };
  
  return (
    <div>
      <label className="block text-xs text-[var(--color-text-secondary)] mb-1.5">{label}</label>
      <div className="space-y-2">
        <div className="flex flex-wrap gap-2">
          {rules.map((rule, idx) => (
            <motion.span
              key={idx}
              initial={{ opacity: 0, scale: 0.9 }}
              animate={{ opacity: 1, scale: 1 }}
              className={clsx(
                'inline-flex items-center gap-1 px-2 py-1 text-xs rounded border',
                colors[type]
              )}
            >
              {rule}
              <button 
                onClick={() => onRemove(idx)}
                className="ml-1 opacity-60 hover:opacity-100"
              >
                <Trash2 className="w-3 h-3" />
              </button>
            </motion.span>
          ))}
        </div>
        <div className="flex gap-2">
          <input
            type="text"
            value={newRule}
            onChange={(e) => setNewRule(e.target.value)}
            placeholder={`Add ${type} rule...`}
            className="flex-1 px-3 py-1.5 text-sm bg-[var(--color-bg-tertiary)] text-[var(--color-text-primary)] 
              rounded-md border border-[var(--color-border)] 
              focus:outline-none focus:border-[var(--color-border-focus)]
              placeholder:text-[var(--color-text-quaternary)]"
            onKeyDown={(e) => {
              if (e.key === 'Enter' && newRule.trim()) {
                onAdd(newRule.trim());
                setNewRule('');
              }
            }}
          />
          <button
            onClick={() => {
              if (newRule.trim()) {
                onAdd(newRule.trim());
                setNewRule('');
              }
            }}
            className="px-3 py-1.5 bg-[var(--color-bg-elevated)] hover:bg-[var(--color-bg-hover)] 
              text-[var(--color-text-secondary)] rounded-md transition-colors"
          >
            <Plus className="w-4 h-4" />
          </button>
        </div>
      </div>
    </div>
  );
}
