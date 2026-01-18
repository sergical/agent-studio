// ============================================================================
// KeyboardShortcutsModal - Shows all available keyboard shortcuts
// ============================================================================

import { useEffect } from 'react';
import { motion, AnimatePresence } from 'motion/react';

interface KeyboardShortcutsModalProps {
  isOpen: boolean;
  onClose: () => void;
}

const SHORTCUTS = [
  { category: 'General', items: [
    { keys: ['⌘', 'N'], description: 'Create new entity' },
    { keys: ['⌘', 'R'], description: 'Refresh / Rescan projects' },
    { keys: ['⌘', '.'], description: 'Show keyboard shortcuts' },
    { keys: ['Esc'], description: 'Close panel / modal' },
  ]},
  { category: 'Navigation', items: [
    { keys: ['↑', '↓'], description: 'Navigate list items' },
    { keys: ['Enter'], description: 'Select / Open item' },
    { keys: ['⌘', 'K'], description: 'Focus search' },
  ]},
  { category: 'Editing', items: [
    { keys: ['⌘', 'S'], description: 'Save changes' },
    { keys: ['⌘', 'Z'], description: 'Undo' },
    { keys: ['⌘', '⇧', 'Z'], description: 'Redo' },
  ]},
];

export function KeyboardShortcutsModal({ isOpen, onClose }: KeyboardShortcutsModalProps) {
  // Close on escape
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && isOpen) {
        onClose();
      }
    };
    
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [isOpen, onClose]);
  
  return (
    <AnimatePresence>
      {isOpen && (
        <>
          {/* Backdrop */}
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.15 }}
            className="fixed inset-0 bg-black/60 backdrop-blur-sm z-50"
            onClick={onClose}
          />
          
          {/* Modal */}
          <motion.div
            initial={{ opacity: 0, scale: 0.95 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.95 }}
            transition={{ duration: 0.15 }}
            className="fixed top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 z-50 w-full max-w-md"
          >
            <div className="bg-[var(--color-bg-elevated)] border border-[var(--color-border)] rounded-xl shadow-2xl overflow-hidden">
              {/* Header */}
              <div className="px-5 py-4 border-b border-[var(--color-border)]">
                <div className="flex items-center justify-between">
                  <h2 className="text-base font-semibold text-[var(--color-text-primary)]">
                    Keyboard Shortcuts
                  </h2>
                  <button
                    onClick={onClose}
                    className="w-6 h-6 flex items-center justify-center rounded-md hover:bg-[var(--color-bg-hover)] text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] transition-colors"
                  >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <path d="M18 6L6 18M6 6l12 12" />
                    </svg>
                  </button>
                </div>
              </div>
              
              {/* Content */}
              <div className="px-5 py-4 max-h-[60vh] overflow-y-auto">
                {SHORTCUTS.map((section) => (
                  <div key={section.category} className="mb-5 last:mb-0">
                    <h3 className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-quaternary)] mb-2">
                      {section.category}
                    </h3>
                    <div className="space-y-2">
                      {section.items.map((shortcut, idx) => (
                        <div 
                          key={idx}
                          className="flex items-center justify-between py-1.5"
                        >
                          <span className="text-sm text-[var(--color-text-secondary)]">
                            {shortcut.description}
                          </span>
                          <div className="flex items-center gap-1">
                            {shortcut.keys.map((key, keyIdx) => (
                              <kbd
                                key={keyIdx}
                                className="min-w-[24px] h-6 px-1.5 flex items-center justify-center rounded-md bg-[var(--color-bg-tertiary)] border border-[var(--color-border)] text-[11px] font-medium text-[var(--color-text-secondary)]"
                              >
                                {key}
                              </kbd>
                            ))}
                          </div>
                        </div>
                      ))}
                    </div>
                  </div>
                ))}
              </div>
              
              {/* Footer */}
              <div className="px-5 py-3 border-t border-[var(--color-border)] bg-[var(--color-bg-secondary)]">
                <p className="text-xs text-[var(--color-text-quaternary)] text-center">
                  Press <kbd className="px-1.5 py-0.5 rounded bg-[var(--color-bg-tertiary)] text-[10px]">Esc</kbd> to close
                </p>
              </div>
            </div>
          </motion.div>
        </>
      )}
    </AnimatePresence>
  );
}
