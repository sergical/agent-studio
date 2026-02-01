// ============================================================================
// SkillSearchBar - Search input for skills.sh
// ============================================================================

import { useCallback, useEffect, useRef } from 'react';
import { Search, X, Loader } from 'lucide-react';

interface SkillSearchBarProps {
  value: string;
  onChange: (value: string) => void;
  onSearch: (query: string) => void;
  isLoading: boolean;
  placeholder?: string;
}

export function SkillSearchBar({
  value,
  onChange,
  onSearch,
  isLoading,
  placeholder = 'Search skills.sh...',
}: SkillSearchBarProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Debounced search
  useEffect(() => {
    if (debounceRef.current) {
      clearTimeout(debounceRef.current);
    }

    debounceRef.current = setTimeout(() => {
      onSearch(value);
    }, 300);

    return () => {
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
      }
    };
  }, [value, onSearch]);

  const handleClear = useCallback(() => {
    onChange('');
    inputRef.current?.focus();
  }, [onChange]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      handleClear();
    }
  }, [handleClear]);

  return (
    <div className="skill-search-bar">
      <div className="skill-search-icon">
        {isLoading ? (
          <Loader size={16} className="skill-search-spinner" />
        ) : (
          <Search size={16} />
        )}
      </div>
      <input
        ref={inputRef}
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={placeholder}
        className="skill-search-input"
      />
      {value && (
        <button
          className="skill-search-clear"
          onClick={handleClear}
          title="Clear search"
        >
          <X size={14} />
        </button>
      )}
    </div>
  );
}
