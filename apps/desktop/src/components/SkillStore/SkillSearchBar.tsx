// ============================================================================
// SkillSearchBar - Search input for skills.sh
// ============================================================================

import { useEffect, useEffectEvent, useRef } from "react";
import { Search, X, Loader } from "lucide-react";
import { Button, Input } from "@skill-studio/ui";

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

  // Reads the latest `onSearch` without resetting the debounce timer below
  // when the caller passes a new `onSearch` identity without `value` itself
  // having changed.
  const onDebouncedSearch = useEffectEvent((query: string) => {
    onSearch(query);
  });

  // Debounced search
  useEffect(() => {
    if (debounceRef.current) {
      clearTimeout(debounceRef.current);
    }

    debounceRef.current = setTimeout(() => {
      onDebouncedSearch(value);
    }, 300);

    return () => {
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
      }
    };
  }, [value]);

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
        className="h-(--control-height) pr-8.5 pl-8.5"
      />
      {value && (
        <Button
          variant="ghost"
          size="icon-xs"
          className="absolute right-2 rounded-full bg-bg-tertiary text-text-tertiary"
          onClick={handleClear}
          title="Clear search"
        >
          <X size={14} />
        </Button>
      )}
    </div>
  );
}
