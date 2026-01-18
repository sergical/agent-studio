import { useRef, useState } from 'react';
import { motion, useMotionValue, useTransform, useAnimation, PanInfo } from 'motion/react';
import { clsx } from 'clsx';
import type { LucideIcon } from 'lucide-react';
import { ChevronRight, Trash2 } from 'lucide-react';

interface CommandItemProps {
  icon?: LucideIcon;
  iconColor?: string;
  label: string;
  sublabel?: string;
  location?: string;  // Project path or "global"
  scope?: 'global' | 'project';
  badge?: string;
  badgeColor?: 'default' | 'claude' | 'opencode' | 'success' | 'warning';
  shortcut?: string;
  isSelected?: boolean;
  isActive?: boolean;
  hasChevron?: boolean;
  isDeletable?: boolean;
  isNew?: boolean;
  onClick?: () => void;
  onDelete?: () => void;
  onContextMenu?: (e: React.MouseEvent) => void;
  onPointerDown?: (e: React.PointerEvent) => void;
  onPointerUp?: (e: React.PointerEvent) => void;
  onPointerCancel?: (e: React.PointerEvent) => void;
  onPointerMove?: (e: React.PointerEvent) => void;
}

const SWIPE_THRESHOLD = 60;
const DELETE_BUTTON_WIDTH = 80;

export function CommandItem({
  icon: Icon,
  iconColor,
  label,
  sublabel,
  location,
  scope,
  badge,
  badgeColor = 'default',
  shortcut,
  isSelected,
  isActive,
  hasChevron,
  isDeletable = false,
  isNew = false,
  onClick,
  onDelete,
  onContextMenu,
  onPointerDown,
  onPointerUp,
  onPointerCancel,
  onPointerMove,
}: CommandItemProps) {
  const controls = useAnimation();
  const x = useMotionValue(0);
  const [isSwipeRevealed, setIsSwipeRevealed] = useState(false);
  const isDragging = useRef(false);
  
  // Transform for delete button opacity
  const deleteOpacity = useTransform(x, [-DELETE_BUTTON_WIDTH, -SWIPE_THRESHOLD, 0], [1, 0.8, 0]);
  const deleteScale = useTransform(x, [-DELETE_BUTTON_WIDTH, -SWIPE_THRESHOLD, 0], [1, 0.9, 0.8]);
  
  const handleDragStart = () => {
    isDragging.current = true;
  };
  
  const handleDragEnd = (_: unknown, info: PanInfo) => {
    isDragging.current = false;
    
    if (!isDeletable) {
      controls.start({ x: 0 });
      return;
    }
    
    if (info.offset.x < -SWIPE_THRESHOLD) {
      // Snap to reveal delete button
      controls.start({ x: -DELETE_BUTTON_WIDTH });
      setIsSwipeRevealed(true);
    } else {
      // Snap back
      controls.start({ x: 0 });
      setIsSwipeRevealed(false);
    }
  };
  
  const handleClick = () => {
    // Don't trigger click if we were dragging or swipe is revealed
    if (isDragging.current) return;
    
    if (isSwipeRevealed) {
      // Close swipe and don't trigger click
      controls.start({ x: 0 });
      setIsSwipeRevealed(false);
      return;
    }
    
    onClick?.();
  };
  
  const handleDeleteClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onDelete?.();
    // Reset swipe state
    controls.start({ x: 0 });
    setIsSwipeRevealed(false);
  };
  
  // Handle context menu events
  const handleContextMenu = (e: React.MouseEvent) => {
    onContextMenu?.(e);
  };
  
  const handlePointerDown = (e: React.PointerEvent) => {
    onPointerDown?.(e);
  };
  
  const handlePointerUp = (e: React.PointerEvent) => {
    onPointerUp?.(e);
  };

  return (
    <div className="relative overflow-hidden rounded-lg">
      {/* Delete button behind */}
      {isDeletable && (
        <motion.button
          className="absolute right-0 top-0 bottom-0 flex items-center justify-center bg-[var(--color-error)] text-white"
          style={{ 
            width: DELETE_BUTTON_WIDTH,
            opacity: deleteOpacity,
            scale: deleteScale,
          }}
          onClick={handleDeleteClick}
        >
          <Trash2 className="w-5 h-5" />
        </motion.button>
      )}
      
      {/* Main item */}
      <motion.button
        onClick={handleClick}
        onContextMenu={handleContextMenu}
        onPointerDown={handlePointerDown}
        onPointerUp={handlePointerUp}
        onPointerCancel={onPointerCancel}
        onPointerMove={onPointerMove}
        className={clsx(
          'relative w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-left',
          'transition-colors duration-100',
          isSelected && 'bg-[var(--color-accent)]',
          isActive && !isSelected && 'bg-[var(--color-bg-hover)]',
          !isSelected && !isActive && 'hover:bg-[var(--color-bg-hover)]',
          'focus:outline-none',
          'active:scale-[0.99] active:transition-transform'
        )}
        style={{ x }}
        drag={isDeletable ? 'x' : false}
        dragConstraints={{ left: -DELETE_BUTTON_WIDTH, right: 0 }}
        dragElastic={0.1}
        onDragStart={handleDragStart}
        onDragEnd={handleDragEnd}
        animate={controls}
        initial={isNew ? { opacity: 0, y: 8 } : false}
        whileInView={isNew ? { opacity: 1, y: 0 } : undefined}
        transition={{ duration: 0.2 }}
      >
        {Icon && (
          <div
            className={clsx(
              'flex items-center justify-center w-8 h-8 rounded-md shrink-0',
              isSelected ? 'bg-white/20' : 'bg-[var(--color-bg-elevated)]'
            )}
          >
            <Icon
              className="w-4 h-4"
              style={{ color: isSelected ? 'white' : iconColor }}
            />
          </div>
        )}
        
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <span className={clsx(
              'text-[13px] font-medium truncate',
              isSelected ? 'text-white' : 'text-[var(--color-text-primary)]'
            )}>
              {label}
            </span>
            {scope && (
              <span className={clsx(
                'text-[10px] shrink-0',
                isSelected ? 'text-white/40' : 'text-[var(--color-text-quaternary)]'
              )}>
                {scope === 'global' ? 'Global' : 'Project'}
              </span>
            )}
          </div>
          {sublabel && (
            <div className={clsx(
              'text-[11px] truncate mt-0.5',
              isSelected ? 'text-white/70' : 'text-[var(--color-text-tertiary)]'
            )}>
              {sublabel}
            </div>
          )}
          {location && (
            <div className={clsx(
              'text-[10px] truncate mt-0.5 font-mono',
              isSelected ? 'text-white/50' : 'text-[var(--color-text-quaternary)]'
            )}>
              {location}
            </div>
          )}
        </div>
        
        {badge && (
          <span className={clsx(
            'px-2 py-0.5 text-[9px] font-semibold uppercase tracking-wide rounded shrink-0',
            isSelected 
              ? 'bg-white/20 text-white' 
              : badgeColor === 'warning' 
                ? 'bg-[var(--color-warning-soft)] text-[var(--color-warning)]'
                : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-tertiary)]'
          )}>
            {badge}
          </span>
        )}
        
        {shortcut && (
          <kbd className={clsx(
            'hidden sm:flex items-center gap-0.5 px-1.5 py-0.5 text-[10px] font-medium rounded shrink-0',
            isSelected
              ? 'bg-white/20 text-white'
              : 'bg-[var(--color-bg-tertiary)] text-[var(--color-text-quaternary)] border border-[var(--color-border-subtle)]'
          )}>
            {shortcut}
          </kbd>
        )}
        
        {hasChevron && (
          <ChevronRight className={clsx(
            'w-4 h-4 shrink-0',
            isSelected ? 'text-white/70' : 'text-[var(--color-text-quaternary)]'
          )} />
        )}
      </motion.button>
    </div>
  );
}
