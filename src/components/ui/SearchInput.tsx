import { forwardRef } from 'react';
import { Search, Command } from 'lucide-react';
import { clsx } from 'clsx';

interface SearchInputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  onClear?: () => void;
}

export const SearchInput = forwardRef<HTMLInputElement, SearchInputProps>(
  ({ className, value, onClear, ...props }, ref) => {
    return (
      <div className="relative">
        <div className="absolute left-3 top-1/2 -translate-y-1/2 pointer-events-none">
          <Search className="w-4 h-4 text-[var(--color-text-tertiary)]" />
        </div>
        <input
          ref={ref}
          type="text"
          value={value}
          className={clsx(
            'w-full h-10 pl-10 pr-16 bg-[var(--color-bg-secondary)] text-[var(--color-text-primary)]',
            'rounded-lg border border-[var(--color-border)]',
            'placeholder:text-[var(--color-text-quaternary)]',
            'focus:outline-none focus:border-[var(--color-border-focus)] focus:bg-[var(--color-bg-tertiary)]',
            'transition-all duration-150',
            className
          )}
          {...props}
        />
        <div className="absolute right-3 top-1/2 -translate-y-1/2 flex items-center gap-1 pointer-events-none">
          <kbd className="flex items-center gap-0.5 px-1.5 py-0.5 text-[10px] font-medium text-[var(--color-text-quaternary)] bg-[var(--color-bg-tertiary)] rounded border border-[var(--color-border-subtle)]">
            <Command className="w-2.5 h-2.5" />K
          </kbd>
        </div>
      </div>
    );
  }
);

SearchInput.displayName = 'SearchInput';
