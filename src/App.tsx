// ============================================================================
// Agent Studio - Main Application
// Comprehensive Claude Code configuration management
// ============================================================================

import { useEffect, useCallback, useState } from 'react';
import { useAppStore } from './store/appStore';
import { Navigation } from './components/Navigation';
import { Dashboard } from './components/Dashboard';
import { HealthCheck } from './components/HealthCheck';
import { CommandPalette } from './components/CommandPalette';
import { DetailPanel } from './components/DetailPanel';
import { ToastContainer } from './components/ui/ToastContainer';
import { KeyboardShortcutsModal } from './components/ui/KeyboardShortcutsModal';
import { CreateEntityDialog } from './components/ui/CreateEntityDialog';
import { open } from '@tauri-apps/plugin-dialog';
import { getHomeDirectory } from './lib/api';
import './App.css';

type CreatableEntityType = 'agent' | 'skill' | 'command' | 'memory';

function App() {
  const discoverAll = useAppStore((state) => state.discoverAll);
  const refreshDiscovery = useAppStore((state) => state.refreshDiscovery);
  const activeView = useAppStore((state) => state.activeView);
  const addToast = useAppStore((state) => state.addToast);
  const setHasInitiallyAnimated = useAppStore((state) => state.setHasInitiallyAnimated);
  const isLoading = useAppStore((state) => state.isLoading);
  const lastDiscovery = useAppStore((state) => state.lastDiscovery);
  
  const [showShortcuts, setShowShortcuts] = useState(false);
  const [showCreateDialog, setShowCreateDialog] = useState(false);
  const [createDialogType, setCreateDialogType] = useState<CreatableEntityType>('agent');
  const isFirstLoad = lastDiscovery === null;
  const theme = useAppStore((state) => state.theme);

  // Initialize theme on mount
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
  }, []);

  // Initial discovery on mount
  useEffect(() => {
    const init = async () => {
      try {
        const homeDir = await getHomeDirectory();
        // Scan from home directory - Rust will handle filtering non-project dirs
        await discoverAll([homeDir]);
        setHasInitiallyAnimated(true);
      } catch (err) {
        console.error('Failed to discover configurations:', err);
      }
    };
    init();
  }, [discoverAll, setHasInitiallyAnimated]);
  
  // Auto-refresh on window focus (when coming back from another app)
  useEffect(() => {
    let lastRefresh = Date.now();
    const MIN_REFRESH_INTERVAL = 30000; // Don't refresh more than once every 30 seconds
    
    const handleFocus = () => {
      const now = Date.now();
      // Only refresh if enough time has passed and we have done initial load
      if (now - lastRefresh > MIN_REFRESH_INTERVAL && lastDiscovery !== null) {
        lastRefresh = now;
        refreshDiscovery();
      }
    };
    
    window.addEventListener('focus', handleFocus);
    return () => window.removeEventListener('focus', handleFocus);
  }, [refreshDiscovery, lastDiscovery]);

  // Keyboard shortcuts
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // ⌘. - Show keyboard shortcuts
      if ((e.metaKey || e.ctrlKey) && e.key === '.') {
        e.preventDefault();
        setShowShortcuts(prev => !prev);
        return;
      }
      
      // ⌘R - Refresh
      if ((e.metaKey || e.ctrlKey) && e.key === 'r') {
        e.preventDefault();
        refreshDiscovery();
        addToast({
          type: 'info',
          title: 'Refreshing...',
          message: 'Scanning for Claude Code configurations',
          duration: 2000,
        });
      }
      
      // ⌘N - Create new entity (based on current view)
      if ((e.metaKey || e.ctrlKey) && e.key === 'n') {
        e.preventDefault();
        const viewTypeMap: Record<string, CreatableEntityType> = {
          agents: 'agent',
          skills: 'skill',
          commands: 'command',
          memory: 'memory',
        };
        const type = viewTypeMap[activeView] || 'agent';
        setCreateDialogType(type);
        setShowCreateDialog(true);
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [refreshDiscovery, addToast, activeView]);

  const handleAddProject = useCallback(async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: 'Add Project Directory',
      });
      
      if (selected) {
        const homeDir = await getHomeDirectory();
        // Rescan home + the newly selected directory
        await discoverAll([homeDir, selected as string]);
        addToast({
          type: 'success',
          title: 'Project Added',
          message: `Scanning ${(selected as string).split('/').pop()}...`,
        });
      }
    } catch (err) {
      console.error('Failed to add project:', err);
      addToast({
        type: 'error',
        title: 'Failed to Add Project',
        message: err instanceof Error ? err.message : 'Unknown error',
      });
    }
  }, [discoverAll, addToast]);

  const handleNewAgent = useCallback(() => {
    setCreateDialogType('agent');
    setShowCreateDialog(true);
  }, []);

  const handleNewSkill = useCallback(() => {
    setCreateDialogType('skill');
    setShowCreateDialog(true);
  }, []);

  const handleNewCommand = useCallback(() => {
    setCreateDialogType('command');
    setShowCreateDialog(true);
  }, []);

  return (
    <div className="flex h-screen overflow-hidden bg-[var(--color-bg-primary)]">
      {/* Loading Overlay - Only show on initial load */}
      {isLoading && isFirstLoad && (
        <div className="loading-overlay">
          <div className="loading-spinner" />
          <div className="loading-text">Discovering configurations...</div>
          <div className="loading-subtext">Scanning projects from home directory</div>
        </div>
      )}
      
      {/* Navigation Sidebar */}
      <Navigation onOpenShortcuts={() => setShowShortcuts(true)} />
      
      {/* Main Content Area */}
      <div className="flex flex-1 overflow-hidden">
        {activeView === 'dashboard' ? (
          <Dashboard />
        ) : activeView === 'health' ? (
          <HealthCheck />
        ) : (
          <>
            {/* Command Palette - Left Side */}
            <CommandPalette
              onOpenProject={handleAddProject}
              onNewAgent={handleNewAgent}
              onNewSkill={handleNewSkill}
              onNewCommand={handleNewCommand}
            />
            
            {/* Detail Panel - Right Side */}
            <DetailPanel />
          </>
        )}
      </div>
      
      {/* Toast Notifications */}
      <ToastContainer />
      
      {/* Keyboard Shortcuts Modal */}
      <KeyboardShortcutsModal 
        isOpen={showShortcuts} 
        onClose={() => setShowShortcuts(false)} 
      />
      
      {/* Create Entity Dialog */}
      <CreateEntityDialog
        isOpen={showCreateDialog}
        onClose={() => setShowCreateDialog(false)}
        initialEntityType={createDialogType}
      />
    </div>
  );
}

export default App;
