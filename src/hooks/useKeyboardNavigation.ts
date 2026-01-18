import { useEffect, useCallback } from 'react';

interface UseKeyboardNavigationOptions {
  itemCount: number;
  selectedIndex: number;
  onSelectedIndexChange: (index: number) => void;
  onSelect: (index: number) => void;
  onEscape?: () => void;
  isEnabled?: boolean;
}

export function useKeyboardNavigation({
  itemCount,
  selectedIndex,
  onSelectedIndexChange,
  onSelect,
  onEscape,
  isEnabled = true,
}: UseKeyboardNavigationOptions) {
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (!isEnabled) return;

      switch (e.key) {
        case 'ArrowDown':
        case 'j':
          e.preventDefault();
          onSelectedIndexChange(
            selectedIndex < itemCount - 1 ? selectedIndex + 1 : 0
          );
          break;
        case 'ArrowUp':
        case 'k':
          e.preventDefault();
          onSelectedIndexChange(
            selectedIndex > 0 ? selectedIndex - 1 : itemCount - 1
          );
          break;
        case 'Enter':
          e.preventDefault();
          if (selectedIndex >= 0 && selectedIndex < itemCount) {
            onSelect(selectedIndex);
          }
          break;
        case 'Escape':
          e.preventDefault();
          onEscape?.();
          break;
      }
    },
    [isEnabled, itemCount, selectedIndex, onSelectedIndexChange, onSelect, onEscape]
  );

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);
}

interface UseGlobalShortcutsOptions {
  onOpenSearch?: () => void;
  onSave?: () => void;
  onRefresh?: () => void;
  onNewAgent?: () => void;
  onOpenProject?: () => void;
}

export function useGlobalShortcuts({
  onOpenSearch,
  onSave,
  onRefresh,
  onNewAgent,
  onOpenProject,
}: UseGlobalShortcutsOptions) {
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      const isMeta = e.metaKey || e.ctrlKey;

      if (isMeta && e.key === 'k') {
        e.preventDefault();
        onOpenSearch?.();
      } else if (isMeta && e.key === 's') {
        e.preventDefault();
        onSave?.();
      } else if (isMeta && e.key === 'r') {
        e.preventDefault();
        onRefresh?.();
      } else if (isMeta && e.key === 'n') {
        e.preventDefault();
        onNewAgent?.();
      } else if (isMeta && e.key === 'o') {
        e.preventDefault();
        onOpenProject?.();
      }
    },
    [onOpenSearch, onSave, onRefresh, onNewAgent, onOpenProject]
  );

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);
}
