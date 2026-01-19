// ============================================================================
// EntityActionsMenu - Dropdown menu for entity actions
// ============================================================================

import { useState, useRef, useEffect } from 'react';
import { motion, AnimatePresence } from 'motion/react';
import { 
  MoreHorizontal, 
  Link, 
  Pencil, 
  Trash2, 
  FolderUp, 
  FolderDown,
  ExternalLink,
  Files,
} from 'lucide-react';
import { clsx } from 'clsx';
import type { EntityType } from '../../lib/types';

export interface EntityAction {
  id: string;
  label: string;
  icon: React.ReactNode;
  variant?: 'default' | 'danger';
  disabled?: boolean;
  shortcut?: string;
}

interface EntityActionsMenuProps {
  entityType: EntityType;
  entityScope: 'global' | 'project';
  onCopyToGlobal?: () => void;
  onCopyToProject?: () => void;
  onCreateSymlink?: () => void;
  onRename?: () => void;
  onDuplicate?: () => void;
  onDelete?: () => void;
  onOpenInFinder?: () => void;
  className?: string;
}

export function EntityActionsMenu({
  entityType,
  entityScope,
  onCopyToGlobal,
  onCopyToProject,
  onCreateSymlink,
  onRename,
  onDuplicate,
  onDelete,
  onOpenInFinder,
  className,
}: EntityActionsMenuProps) {
  const [isOpen, setIsOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  
  // Close menu when clicking outside
  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        setIsOpen(false);
      }
    }
    
    if (isOpen) {
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }
  }, [isOpen]);
  
  // Close menu on escape
  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        setIsOpen(false);
      }
    }
    
    if (isOpen) {
      document.addEventListener('keydown', handleKeyDown);
      return () => document.removeEventListener('keydown', handleKeyDown);
    }
  }, [isOpen]);
  
  // Only show for certain entity types
  const canShowActions = ['agent', 'skill', 'command'].includes(entityType);
  
  if (!canShowActions) {
    return null;
  }
  
  const actions: EntityAction[] = [];
  
  // Copy actions based on current scope
  if (entityScope === 'project' && onCopyToGlobal) {
    actions.push({
      id: 'copy-to-global',
      label: 'Copy to Global',
      icon: <FolderUp className="w-4 h-4" />,
    });
  }
  
  if (entityScope === 'global' && onCopyToProject) {
    actions.push({
      id: 'copy-to-project',
      label: 'Copy to Project',
      icon: <FolderDown className="w-4 h-4" />,
    });
  }
  
  // Symlink action
  if (onCreateSymlink) {
    actions.push({
      id: 'create-symlink',
      label: entityScope === 'global' ? 'Symlink to Project' : 'Symlink to Global',
      icon: <Link className="w-4 h-4" />,
    });
  }
  
  // Duplicate action
  if (onDuplicate) {
    actions.push({
      id: 'duplicate',
      label: 'Duplicate',
      icon: <Files className="w-4 h-4" />,
    });
  }
  
  // Rename action
  if (onRename) {
    actions.push({
      id: 'rename',
      label: 'Rename',
      icon: <Pencil className="w-4 h-4" />,
    });
  }
  
  // Open in Finder
  if (onOpenInFinder) {
    actions.push({
      id: 'open-in-finder',
      label: 'Show in Finder',
      icon: <ExternalLink className="w-4 h-4" />,
    });
  }
  
  // Delete action (always last)
  if (onDelete) {
    actions.push({
      id: 'delete',
      label: 'Delete',
      icon: <Trash2 className="w-4 h-4" />,
      variant: 'danger',
    });
  }
  
  if (actions.length === 0) {
    return null;
  }
  
  const handleAction = (actionId: string) => {
    setIsOpen(false);
    
    switch (actionId) {
      case 'copy-to-global':
        onCopyToGlobal?.();
        break;
      case 'copy-to-project':
        onCopyToProject?.();
        break;
      case 'create-symlink':
        onCreateSymlink?.();
        break;
      case 'duplicate':
        onDuplicate?.();
        break;
      case 'rename':
        onRename?.();
        break;
      case 'open-in-finder':
        onOpenInFinder?.();
        break;
      case 'delete':
        onDelete?.();
        break;
    }
  };
  
  return (
    <div ref={menuRef} className={clsx('relative', className)}>
      <button
        ref={buttonRef}
        onClick={() => setIsOpen(!isOpen)}
        className={clsx(
          'p-1.5 rounded-md transition-colors',
          isOpen 
            ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
            : 'text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'
        )}
        title="More actions"
      >
        <MoreHorizontal className="w-4 h-4" />
      </button>
      
      <AnimatePresence>
        {isOpen && (
          <motion.div
            initial={{ opacity: 0, scale: 0.95, y: -4 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.95, y: -4 }}
            transition={{ duration: 0.1 }}
            className="absolute right-0 top-full mt-1 z-50 min-w-[180px] py-1 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] rounded-lg shadow-lg"
          >
            {actions.map((action, index) => (
              <button
                key={action.id}
                onClick={() => handleAction(action.id)}
                disabled={action.disabled}
                className={clsx(
                  'w-full flex items-center gap-2.5 px-3 py-2 text-sm text-left transition-colors',
                  action.disabled && 'opacity-50 cursor-not-allowed',
                  action.variant === 'danger'
                    ? 'text-[var(--color-error)] hover:bg-[var(--color-error-soft)]'
                    : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)]',
                  // Add separator before delete
                  action.variant === 'danger' && index > 0 && 'border-t border-[var(--color-border)] mt-1 pt-2'
                )}
              >
                {action.icon}
                <span className="flex-1">{action.label}</span>
                {action.shortcut && (
                  <kbd className="px-1.5 py-0.5 text-[10px] bg-[var(--color-bg-tertiary)] text-[var(--color-text-quaternary)] rounded">
                    {action.shortcut}
                  </kbd>
                )}
              </button>
            ))}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
