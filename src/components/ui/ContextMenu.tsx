import { useEffect, useRef, useCallback, useState } from 'react';
import { motion, AnimatePresence } from 'motion/react';
import { createPortal } from 'react-dom';
import { clsx } from 'clsx';
import type { LucideIcon } from 'lucide-react';

export interface ContextMenuItem {
  id: string;
  label: string;
  icon?: LucideIcon;
  onClick: () => void;
  variant?: 'default' | 'danger';
  disabled?: boolean;
  separator?: boolean;
}

interface ContextMenuProps {
  isOpen: boolean;
  onClose: () => void;
  position: { x: number; y: number };
  items: ContextMenuItem[];
}

export function ContextMenu({ isOpen, onClose, position, items }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [adjustedPosition, setAdjustedPosition] = useState(position);

  // Filter out separator-only items for keyboard navigation
  const navigableItems = items.filter((item) => !item.separator && !item.disabled);

  // Adjust position to keep menu in viewport
  useEffect(() => {
    if (isOpen && menuRef.current) {
      const rect = menuRef.current.getBoundingClientRect();
      const padding = 8;
      let { x, y } = position;

      // Adjust horizontal position
      if (x + rect.width > window.innerWidth - padding) {
        x = window.innerWidth - rect.width - padding;
      }
      if (x < padding) {
        x = padding;
      }

      // Adjust vertical position
      if (y + rect.height > window.innerHeight - padding) {
        y = window.innerHeight - rect.height - padding;
      }
      if (y < padding) {
        y = padding;
      }

      setAdjustedPosition({ x, y });
    }
  }, [isOpen, position]);

  // Reset selection when opening
  useEffect(() => {
    if (isOpen) {
      setSelectedIndex(0);
    }
  }, [isOpen]);

  // Keyboard navigation
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (!isOpen) return;

      switch (e.key) {
        case 'Escape':
          e.preventDefault();
          onClose();
          break;
        case 'ArrowDown':
          e.preventDefault();
          setSelectedIndex((prev) => (prev + 1) % navigableItems.length);
          break;
        case 'ArrowUp':
          e.preventDefault();
          setSelectedIndex((prev) => (prev - 1 + navigableItems.length) % navigableItems.length);
          break;
        case 'Enter':
        case ' ':
          e.preventDefault();
          const selectedItem = navigableItems[selectedIndex];
          if (selectedItem && !selectedItem.disabled) {
            selectedItem.onClick();
            onClose();
          }
          break;
      }
    },
    [isOpen, onClose, navigableItems, selectedIndex]
  );

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);

  // Close on click outside
  useEffect(() => {
    if (!isOpen) return;

    const handleClickOutside = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    };

    const handleScroll = () => {
      onClose();
    };

    // Delay adding listener to prevent immediate close
    const timeoutId = setTimeout(() => {
      document.addEventListener('mousedown', handleClickOutside);
      document.addEventListener('scroll', handleScroll, true);
    }, 0);

    return () => {
      clearTimeout(timeoutId);
      document.removeEventListener('mousedown', handleClickOutside);
      document.removeEventListener('scroll', handleScroll, true);
    };
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  let navigableIndex = -1;

  return createPortal(
    <AnimatePresence>
      {isOpen && (
        <motion.div
          ref={menuRef}
          initial={{ opacity: 0, scale: 0.95 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 0.95 }}
          transition={{ duration: 0.1 }}
          className="fixed z-50 min-w-[160px] py-1 bg-[var(--color-bg-primary)] border border-[var(--color-border)] rounded-lg shadow-xl"
          style={{
            left: adjustedPosition.x,
            top: adjustedPosition.y,
            willChange: 'transform, opacity',
          }}
          role="menu"
          aria-orientation="vertical"
        >
          {items.map((item, index) => {
            if (item.separator) {
              return (
                <div
                  key={`separator-${index}`}
                  className="my-1 border-t border-[var(--color-border)]"
                  role="separator"
                />
              );
            }

            if (!item.disabled) {
              navigableIndex++;
            }
            const isSelected = !item.disabled && navigableIndex === selectedIndex;
            const Icon = item.icon;

            return (
              <button
                key={item.id}
                onClick={() => {
                  if (!item.disabled) {
                    item.onClick();
                    onClose();
                  }
                }}
                disabled={item.disabled}
                className={clsx(
                  'w-full flex items-center gap-2.5 px-3 py-2 text-sm text-left transition-colors',
                  'focus:outline-none',
                  item.disabled && 'opacity-40 cursor-not-allowed',
                  !item.disabled && isSelected && 'bg-[var(--color-bg-hover)]',
                  !item.disabled && !isSelected && 'hover:bg-[var(--color-bg-hover)]',
                  item.variant === 'danger' && !item.disabled && 'text-[var(--color-error)]',
                  item.variant !== 'danger' && 'text-[var(--color-text-primary)]'
                )}
                role="menuitem"
                tabIndex={-1}
              >
                {Icon && (
                  <Icon
                    className={clsx(
                      'w-4 h-4',
                      item.variant === 'danger' ? 'text-[var(--color-error)]' : 'text-[var(--color-text-tertiary)]'
                    )}
                  />
                )}
                {item.label}
              </button>
            );
          })}
        </motion.div>
      )}
    </AnimatePresence>,
    document.body
  );
}

// Hook to handle context menu state
export function useContextMenu() {
  const [contextMenu, setContextMenu] = useState<{
    isOpen: boolean;
    position: { x: number; y: number };
    data: unknown;
  }>({
    isOpen: false,
    position: { x: 0, y: 0 },
    data: null,
  });

  const longPressTimerRef = useRef<number | null>(null);
  const longPressTriggeredRef = useRef(false);

  const open = useCallback((position: { x: number; y: number }, data?: unknown) => {
    setContextMenu({ isOpen: true, position, data });
  }, []);

  const close = useCallback(() => {
    setContextMenu((prev) => ({ ...prev, isOpen: false }));
  }, []);

  // Handle right-click
  const handleContextMenu = useCallback(
    (e: React.MouseEvent, data?: unknown) => {
      e.preventDefault();
      e.stopPropagation();
      open({ x: e.clientX, y: e.clientY }, data);
    },
    [open]
  );

  // Handle long-press start
  const handlePointerDown = useCallback(
    (e: React.PointerEvent, data?: unknown) => {
      // Only handle primary button (left click / touch)
      if (e.button !== 0) return;
      
      longPressTriggeredRef.current = false;
      
      longPressTimerRef.current = window.setTimeout(() => {
        longPressTriggeredRef.current = true;
        open({ x: e.clientX, y: e.clientY }, data);
      }, 500); // 500ms long-press threshold
    },
    [open]
  );

  // Handle long-press cancel
  const handlePointerUp = useCallback(() => {
    if (longPressTimerRef.current) {
      clearTimeout(longPressTimerRef.current);
      longPressTimerRef.current = null;
    }
  }, []);

  const handlePointerCancel = handlePointerUp;
  
  const handlePointerMove = useCallback((e: React.PointerEvent) => {
    // Cancel long-press if pointer moves too much
    if (longPressTimerRef.current && (Math.abs(e.movementX) > 5 || Math.abs(e.movementY) > 5)) {
      clearTimeout(longPressTimerRef.current);
      longPressTimerRef.current = null;
    }
  }, []);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (longPressTimerRef.current) {
        clearTimeout(longPressTimerRef.current);
      }
    };
  }, []);

  return {
    contextMenu,
    open,
    close,
    handlers: {
      onContextMenu: handleContextMenu,
      onPointerDown: handlePointerDown,
      onPointerUp: handlePointerUp,
      onPointerCancel: handlePointerCancel,
      onPointerMove: handlePointerMove,
    },
    wasLongPress: () => longPressTriggeredRef.current,
  };
}
