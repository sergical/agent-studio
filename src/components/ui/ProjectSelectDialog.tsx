// ============================================================================
// ProjectSelectDialog - Dialog for selecting a project to copy/symlink to
// ============================================================================

import { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'motion/react';
import { X, Folder, Search, FolderOpen } from 'lucide-react';
import { clsx } from 'clsx';
import type { ProjectInfo } from '../../lib/types';

interface ProjectSelectDialogProps {
  isOpen: boolean;
  onClose: () => void;
  onSelect: (projectPath: string) => void;
  projects: ProjectInfo[];
  title: string;
  description?: string;
  isLoading?: boolean;
}

export function ProjectSelectDialog({
  isOpen,
  onClose,
  onSelect,
  projects,
  title,
  description,
  isLoading = false,
}: ProjectSelectDialogProps) {
  const [searchQuery, setSearchQuery] = useState('');
  
  // Reset search when opening
  useEffect(() => {
    if (isOpen) {
      setSearchQuery('');
    }
  }, [isOpen]);
  
  const filteredProjects = projects.filter(project => 
    project.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
    project.path.toLowerCase().includes(searchQuery.toLowerCase())
  );
  
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      onClose();
    }
  };
  
  const formatPath = (path: string) => {
    return path.replace(/^\/Users\/[^/]+/, '~');
  };
  
  return (
    <AnimatePresence>
      {isOpen && (
        <>
          {/* Backdrop */}
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onClick={onClose}
            className="fixed inset-0 bg-black/50 z-50"
          />
          
          {/* Dialog */}
          <motion.div
            initial={{ opacity: 0, scale: 0.95, y: -10 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.95, y: -10 }}
            transition={{ duration: 0.15 }}
            className="fixed top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 z-50 w-full max-w-lg"
          >
            <div className="bg-[var(--color-bg-elevated)] border border-[var(--color-border)] rounded-xl shadow-2xl overflow-hidden max-h-[70vh] flex flex-col">
              {/* Header */}
              <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)] shrink-0">
                <div className="flex items-center gap-2">
                  <FolderOpen className="w-4 h-4 text-[var(--color-text-tertiary)]" />
                  <h2 className="text-sm font-semibold text-[var(--color-text-primary)]">
                    {title}
                  </h2>
                </div>
                <button
                  onClick={onClose}
                  className="p-1.5 text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] rounded-md transition-colors"
                >
                  <X className="w-4 h-4" />
                </button>
              </div>
              
              {/* Description */}
              {description && (
                <div className="px-4 py-2 text-xs text-[var(--color-text-tertiary)] border-b border-[var(--color-border)] shrink-0">
                  {description}
                </div>
              )}
              
              {/* Search */}
              <div className="px-4 py-3 border-b border-[var(--color-border)] shrink-0">
                <div className="relative">
                  <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-[var(--color-text-quaternary)]" />
                  <input
                    type="text"
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    onKeyDown={handleKeyDown}
                    placeholder="Search projects..."
                    className="w-full pl-10 pr-3 py-2 text-sm bg-[var(--color-bg-tertiary)] text-[var(--color-text-primary)] rounded-md border border-[var(--color-border)] focus:outline-none focus:border-[var(--color-border-focus)] placeholder:text-[var(--color-text-quaternary)]"
                    autoFocus
                  />
                </div>
              </div>
              
              {/* Projects List */}
              <div className="flex-1 overflow-y-auto">
                {filteredProjects.length === 0 ? (
                  <div className="px-4 py-8 text-center text-sm text-[var(--color-text-tertiary)]">
                    {searchQuery ? 'No projects match your search' : 'No projects found'}
                  </div>
                ) : (
                  <div className="py-2">
                    {filteredProjects.map((project) => (
                      <button
                        key={project.path}
                        onClick={() => onSelect(project.path)}
                        disabled={isLoading}
                        className={clsx(
                          'w-full flex items-start gap-3 px-4 py-2.5 text-left transition-colors',
                          'hover:bg-[var(--color-bg-hover)]',
                          isLoading && 'opacity-50 cursor-not-allowed'
                        )}
                      >
                        <Folder className="w-4 h-4 text-[var(--color-text-tertiary)] mt-0.5 shrink-0" />
                        <div className="flex-1 min-w-0">
                          <div className="text-sm font-medium text-[var(--color-text-primary)] truncate">
                            {project.name}
                          </div>
                          <div className="text-xs text-[var(--color-text-tertiary)] truncate">
                            {formatPath(project.path)}
                          </div>
                        </div>
                      </button>
                    ))}
                  </div>
                )}
              </div>
              
              {/* Footer */}
              <div className="px-4 py-3 border-t border-[var(--color-border)] shrink-0">
                <button
                  onClick={onClose}
                  className="w-full px-4 py-2 text-sm font-medium text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] rounded-md transition-colors"
                >
                  Cancel
                </button>
              </div>
            </div>
          </motion.div>
        </>
      )}
    </AnimatePresence>
  );
}
