// ============================================================================
// Toast - Individual notification component
// ============================================================================

import { motion } from 'motion/react';
import type { Toast as ToastType } from '../../lib/types';

interface ToastProps {
  toast: ToastType;
  onDismiss: (id: string) => void;
}

const ICONS = {
  success: (
    <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
      <circle cx="10" cy="10" r="10" fill="currentColor" fillOpacity="0.15" />
      <path
        d="M6 10l2.5 2.5L14 7"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  ),
  error: (
    <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
      <circle cx="10" cy="10" r="10" fill="currentColor" fillOpacity="0.15" />
      <path
        d="M7 7l6 6M13 7l-6 6"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  ),
  warning: (
    <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
      <circle cx="10" cy="10" r="10" fill="currentColor" fillOpacity="0.15" />
      <path
        d="M10 6v5M10 13.5v.5"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  ),
  info: (
    <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
      <circle cx="10" cy="10" r="10" fill="currentColor" fillOpacity="0.15" />
      <path
        d="M10 6.5v.5M10 9v5"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  ),
};

const COLORS = {
  success: {
    bg: 'var(--color-bg-elevated)',
    border: 'rgba(34, 197, 94, 0.4)',
    icon: '#22c55e',
  },
  error: {
    bg: 'var(--color-bg-elevated)',
    border: 'rgba(239, 68, 68, 0.4)',
    icon: '#ef4444',
  },
  warning: {
    bg: 'var(--color-bg-elevated)',
    border: 'rgba(234, 179, 8, 0.4)',
    icon: '#eab308',
  },
  info: {
    bg: 'var(--color-bg-elevated)',
    border: 'rgba(59, 130, 246, 0.4)',
    icon: '#3b82f6',
  },
};

export function Toast({ toast, onDismiss }: ToastProps) {
  const colors = COLORS[toast.type];
  
  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 20, scale: 0.95 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, y: -20, scale: 0.95 }}
      transition={{ type: 'spring', stiffness: 400, damping: 25 }}
      style={{
        background: colors.bg,
        borderColor: colors.border,
      }}
      className="toast"
      onClick={() => onDismiss(toast.id)}
    >
      <span className="toast-icon" style={{ color: colors.icon }}>
        {ICONS[toast.type]}
      </span>
      <div className="toast-content">
        <span className="toast-title">{toast.title}</span>
        {toast.message && (
          <span className="toast-message">{toast.message}</span>
        )}
      </div>
      <button
        className="toast-dismiss"
        onClick={(e) => {
          e.stopPropagation();
          onDismiss(toast.id);
        }}
      >
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
          <path
            d="M4 4l6 6M10 4l-6 6"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
          />
        </svg>
      </button>
    </motion.div>
  );
}
