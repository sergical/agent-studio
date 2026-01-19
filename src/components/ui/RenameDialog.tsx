// ============================================================================
// RenameDialog - Dialog for renaming entities
// ============================================================================

import { useState, useEffect, useRef } from 'react';
import { motion, AnimatePresence } from 'motion/react';
import { X, Pencil } from 'lucide-react';
import { clsx } from 'clsx';

interface RenameDialogProps {
  isOpen: boolean;
  onClose: () => void;
  onRename: (newName: string) => void;
  currentName: string;
  entityType: string;
  isLoading?: boolean;
}

export function RenameDialog({
  isOpen,
  onClose,
  onRename,
  currentName,
  entityType,
  isLoading = false,
}: RenameDialogProps) {
  const [newName, setNewName] = useState(currentName);
  const inputRef = useRef<HTMLInputElement>(null);
  
  // Reset and focus when opening
  useEffect(() => {
    if (isOpen) {
      setNewName(currentName.replace('.md', ''));
      setTimeout(() => {
        inputRef.current?.focus();
        inputRef.current?.select();
      }, 100);
    }
  }, [isOpen, currentName]);
  
  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (newName.trim() && newName.trim() !== currentName.replace('.md', '')) {
      onRename(newName.trim());
    }
  };
  
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      onClose();
    }
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
            className="fixed top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 z-50 w-full max-w-md"
          >
            <div className="bg-[var(--color-bg-elevated)] border border-[var(--color-border)] rounded-xl shadow-2xl overflow-hidden">
              {/* Header */}
              <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)]">
                <div className="flex items-center gap-2">
                  <Pencil className="w-4 h-4 text-[var(--color-text-tertiary)]" />
                  <h2 className="text-sm font-semibold text-[var(--color-text-primary)]">
                    Rename {entityType}
                  </h2>
                </div>
                <button
                  onClick={onClose}
                  className="p-1.5 text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] rounded-md transition-colors"
                >
                  <X className="w-4 h-4" />
                </button>
              </div>
              
              {/* Content */}
              <form onSubmit={handleSubmit} className="p-4">
                <div className="mb-4">
                  <label className="block text-xs text-[var(--color-text-secondary)] mb-1.5">
                    New name
                  </label>
                  <input
                    ref={inputRef}
                    type="text"
                    value={newName}
                    onChange={(e) => setNewName(e.target.value)}
                    onKeyDown={handleKeyDown}
                    placeholder="Enter new name"
                    className="w-full px-3 py-2 text-sm bg-[var(--color-bg-tertiary)] text-[var(--color-text-primary)] rounded-md border border-[var(--color-border)] focus:outline-none focus:border-[var(--color-border-focus)] placeholder:text-[var(--color-text-quaternary)]"
                    disabled={isLoading}
                  />
                </div>
                
                <div className="flex justify-end gap-2">
                  <button
                    type="button"
                    onClick={onClose}
                    className="px-4 py-2 text-sm font-medium text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] rounded-md transition-colors"
                    disabled={isLoading}
                  >
                    Cancel
                  </button>
                  <button
                    type="submit"
                    disabled={!newName.trim() || newName.trim() === currentName.replace('.md', '') || isLoading}
                    className={clsx(
                      'px-4 py-2 text-sm font-medium rounded-md transition-colors',
                      'bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent-hover)]',
                      'disabled:opacity-50 disabled:cursor-not-allowed'
                    )}
                  >
                    {isLoading ? 'Renaming...' : 'Rename'}
                  </button>
                </div>
              </form>
            </div>
          </motion.div>
        </>
      )}
    </AnimatePresence>
  );
}
