// ============================================================================
// SkillSearchBar - Search input for skills.sh
// ============================================================================

import { useEffect, useRef } from "react";
import { Search, X, Loader } from "lucide-react";
import { Input } from "@skill-studio/ui";

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
  placeholder = "Search skills.sh…",
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

  const handleClear = () => {
    onChange("");
    inputRef.current?.focus();
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      handleClear();
    }
  };

  return (
    <div className="relative flex max-w-[400px] flex-1 items-center">
      <div className="pointer-events-none absolute left-3 flex items-center text-text-tertiary">
        {isLoading ? <Loader size={16} className="animate-spin" /> : <Search size={16} />}
      </div>
      <Input
        ref={inputRef}
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={placeholder}
        className="h-(--control-height) rounded-sm border-border bg-bg-primary pr-8.5 pl-8.5 text-body text-text-primary placeholder:text-text-tertiary focus-visible:border-border-focus focus-visible:ring-0"
      />
      {value && (
        <button
          className="absolute right-2 flex size-5 items-center justify-center rounded-full border-0 bg-bg-tertiary text-text-tertiary transition-colors hover:bg-bg-hover hover:text-text-primary"
          onClick={handleClear}
          title="Clear search"
        >
          <X size={14} />
        </button>
      )}
    </div>
  );
}
