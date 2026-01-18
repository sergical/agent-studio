import { motion } from 'motion/react';
import { X, Save, Trash2 } from 'lucide-react';

interface PanelProps {
  isOpen: boolean;
  onClose: () => void;
  title: string;
  subtitle?: string;
  badge?: string;
  badgeColor?: string;
  hasChanges?: boolean;
  onSave?: () => void;
  canDelete?: boolean;
  onDelete?: () => void;
  children: React.ReactNode;
}

export function Panel({
  isOpen,
  onClose,
  title,
  subtitle,
  badge,
  badgeColor,
  hasChanges,
  onSave,
  canDelete,
  onDelete,
  children,
}: PanelProps) {
  if (!isOpen) {
    return null;
  }

  return (
    <motion.div
      initial={{ opacity: 0, x: 20 }}
      animate={{ opacity: 1, x: 0 }}
      exit={{ opacity: 0, x: 20 }}
      transition={{ duration: 0.2, ease: [0.25, 0.46, 0.45, 0.94] }}
      className="flex-1 flex flex-col bg-[var(--color-bg-primary)] border-l border-[var(--color-border)] overflow-hidden"
    >
      {/* Panel Header */}
      <div className="flex items-center justify-between h-12 px-4 border-b border-[var(--color-border)] shrink-0">
        <div className="flex items-center gap-3 min-w-0 flex-1">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <h2 className="text-sm font-semibold text-[var(--color-text-primary)] truncate">
                {title}
              </h2>
              {hasChanges && (
                <div className="w-2 h-2 rounded-full bg-[var(--color-warning)] shrink-0" title="Unsaved changes" />
              )}
            </div>
            {(subtitle || badge) && (
              <div className="flex items-center gap-2 mt-0.5">
                {subtitle && (
                  <span className="text-xs text-[var(--color-text-tertiary)] truncate max-w-[300px]">
                    {subtitle}
                  </span>
                )}
                {badge && (
                  <span 
                    className="px-1.5 py-0.5 text-[10px] font-medium rounded shrink-0"
                    style={{ 
                      backgroundColor: badgeColor ? `${badgeColor}20` : 'var(--color-bg-elevated)',
                      color: badgeColor || 'var(--color-text-tertiary)'
                    }}
                  >
                    {badge}
                  </span>
                )}
              </div>
            )}
          </div>
        </div>
        
        <div className="flex items-center gap-2 shrink-0">
          {/* Delete button */}
          {canDelete && onDelete && (
            <button
              onClick={onDelete}
              className="flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium text-[var(--color-error)] hover:bg-[var(--color-error-soft)] rounded-md transition-colors"
              title="Delete"
            >
              <Trash2 className="w-3.5 h-3.5" />
              <span className="hidden sm:inline">Delete</span>
            </button>
          )}
          
          {/* Save button */}
          {hasChanges && onSave && (
            <button
              onClick={onSave}
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-white bg-[var(--color-accent)] hover:bg-[var(--color-accent-hover)] rounded-md transition-colors"
            >
              <Save className="w-3.5 h-3.5" />
              Save
              <kbd className="ml-1 px-1 py-0.5 text-[9px] bg-white/20 rounded">⌘S</kbd>
            </button>
          )}
          
          {/* Close button */}
          <button
            onClick={onClose}
            className="p-1.5 text-[var(--color-text-tertiary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] rounded-md transition-colors"
          >
            <X className="w-4 h-4" />
          </button>
        </div>
      </div>
      
      {/* Panel Content - relative container for absolute positioned children */}
      <div className="flex-1 relative overflow-hidden">
        {children}
      </div>
    </motion.div>
  );
}
