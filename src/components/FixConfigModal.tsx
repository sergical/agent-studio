// ============================================================================
// FixConfigModal - Bulk fix AGENTS.md / CLAUDE.md configuration
// ============================================================================

import { useState } from 'react';
import { X, Wrench, Check, AlertCircle, Loader2 } from 'lucide-react';
import { clsx } from 'clsx';
import type { ProjectInfo, ConfigStateType } from '../lib/types';
import { fixProjectConfig } from '../lib/api';
import { useAppStore } from '../store/appStore';

interface FixConfigModalProps {
  projects: ProjectInfo[];
  onClose: () => void;
}

type FixStatus = 'pending' | 'fixing' | 'success' | 'error';

interface ProjectFixState {
  path: string;
  name: string;
  configState: ConfigStateType;
  selected: boolean;
  status: FixStatus;
  message?: string;
}

const getConfigStateLabel = (state: ConfigStateType): string => {
  switch (state) {
    case 'missing_symlink':
      return 'Create CLAUDE.md symlink';
    case 'needs_migration':
      return 'Migrate to AGENTS.md';
    case 'empty':
      return 'Create both files';
    default:
      return 'Unknown action';
  }
};

export function FixConfigModal({ projects, onClose }: FixConfigModalProps) {
  const refreshDiscovery = useAppStore(state => state.refreshDiscovery);
  const addToast = useAppStore(state => state.addToast);

  const [projectStates, setProjectStates] = useState<ProjectFixState[]>(
    projects.map(p => ({
      path: p.path,
      name: p.name,
      configState: p.config_state?.config_state || 'empty',
      selected: true,
      status: 'pending' as FixStatus,
    }))
  );
  const [isFixing, setIsFixing] = useState(false);
  const [isDone, setIsDone] = useState(false);

  const selectedCount = projectStates.filter(p => p.selected).length;

  const toggleProject = (path: string) => {
    if (isFixing || isDone) return;
    setProjectStates(prev =>
      prev.map(p => (p.path === path ? { ...p, selected: !p.selected } : p))
    );
  };

  const toggleAll = () => {
    if (isFixing || isDone) return;
    const allSelected = projectStates.every(p => p.selected);
    setProjectStates(prev => prev.map(p => ({ ...p, selected: !allSelected })));
  };

  const handleFixSelected = async () => {
    const toFix = projectStates.filter(p => p.selected);
    if (toFix.length === 0) return;

    setIsFixing(true);

    // Fix projects sequentially
    for (const project of toFix) {
      // Mark as fixing
      setProjectStates(prev =>
        prev.map(p => (p.path === project.path ? { ...p, status: 'fixing' } : p))
      );

      try {
        const message = await fixProjectConfig(project.path);
        setProjectStates(prev =>
          prev.map(p =>
            p.path === project.path ? { ...p, status: 'success', message } : p
          )
        );
      } catch (error) {
        const errorMessage = error instanceof Error ? error.message : 'Unknown error';
        setProjectStates(prev =>
          prev.map(p =>
            p.path === project.path ? { ...p, status: 'error', message: errorMessage } : p
          )
        );
      }
    }

    setIsFixing(false);
    setIsDone(true);

    // Refresh discovery to update the UI
    await refreshDiscovery();

    // Show summary toast
    const successCount = projectStates.filter(p => p.status === 'success').length;
    const errorCount = projectStates.filter(p => p.status === 'error').length;

    if (errorCount === 0) {
      addToast({
        type: 'success',
        title: 'All configs fixed',
        message: `Successfully fixed ${successCount} project(s)`,
      });
    } else {
      addToast({
        type: 'warning',
        title: 'Some fixes failed',
        message: `Fixed ${successCount}, failed ${errorCount}`,
      });
    }
  };

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal fix-config-modal" onClick={e => e.stopPropagation()}>
        <div className="modal-header">
          <h2 className="modal-title">
            <Wrench className="w-5 h-5" />
            Fix Project Configurations
          </h2>
          <button className="modal-close" onClick={onClose}>
            <X className="w-5 h-5" />
          </button>
        </div>

        <div className="modal-description">
          <p>
            These projects need their AGENTS.md / CLAUDE.md configuration fixed.
            AGENTS.md will be the source of truth, with CLAUDE.md as a symlink.
          </p>
        </div>

        <div className="fix-config-list">
          <div className="fix-config-list-header">
            <label className="fix-config-select-all">
              <input
                type="checkbox"
                checked={projectStates.every(p => p.selected)}
                onChange={toggleAll}
                disabled={isFixing || isDone}
              />
              <span>Select all ({projectStates.length})</span>
            </label>
          </div>

          <div className="fix-config-projects">
            {projectStates.map(project => (
              <label
                key={project.path}
                className={clsx(
                  'fix-config-project',
                  `fix-config-project--${project.status}`,
                  !project.selected && 'fix-config-project--unselected'
                )}
              >
                <input
                  type="checkbox"
                  checked={project.selected}
                  onChange={() => toggleProject(project.path)}
                  disabled={isFixing || isDone}
                />
                <div className="fix-config-project-info">
                  <span className="fix-config-project-name">{project.name}</span>
                  <span className="fix-config-project-action">
                    {getConfigStateLabel(project.configState)}
                  </span>
                  {project.message && (
                    <span className="fix-config-project-message">{project.message}</span>
                  )}
                </div>
                <div className="fix-config-project-status">
                  {project.status === 'fixing' && (
                    <Loader2 className="w-4 h-4 animate-spin" />
                  )}
                  {project.status === 'success' && (
                    <Check className="w-4 h-4 text-green-500" />
                  )}
                  {project.status === 'error' && (
                    <AlertCircle className="w-4 h-4 text-red-500" />
                  )}
                </div>
              </label>
            ))}
          </div>
        </div>

        <div className="modal-actions">
          {isDone ? (
            <button className="modal-btn modal-btn-primary" onClick={onClose}>
              Done
            </button>
          ) : (
            <>
              <button className="modal-btn modal-btn-secondary" onClick={onClose} disabled={isFixing}>
                Cancel
              </button>
              <button
                className="modal-btn modal-btn-primary"
                onClick={handleFixSelected}
                disabled={isFixing || selectedCount === 0}
              >
                {isFixing ? (
                  <>
                    <Loader2 className="w-4 h-4 animate-spin" />
                    Fixing...
                  </>
                ) : (
                  <>
                    <Wrench className="w-4 h-4" />
                    Fix {selectedCount} Project{selectedCount !== 1 ? 's' : ''}
                  </>
                )}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
