import { clsx } from 'clsx';
import { Command, ArrowUp, ArrowDown, CornerDownLeft } from 'lucide-react';

interface KeyboardHintProps {
  keys: string[];
  label?: string;
  className?: string;
}

const keyIcons: Record<string, React.ReactNode> = {
  '⌘': <Command className="w-2.5 h-2.5" />,
  '↑': <ArrowUp className="w-2.5 h-2.5" />,
  '↓': <ArrowDown className="w-2.5 h-2.5" />,
  '↵': <CornerDownLeft className="w-2.5 h-2.5" />,
};

export function KeyboardHint({ keys, label, className }: KeyboardHintProps) {
  return (
    <div className={clsx('flex items-center gap-2', className)}>
      <div className="flex items-center gap-0.5">
        {keys.map((key, i) => (
          <kbd
            key={i}
            className="flex items-center justify-center min-w-[18px] h-[18px] px-1 text-[10px] font-medium text-[var(--color-text-quaternary)] bg-[var(--color-bg-tertiary)] rounded border border-[var(--color-border-subtle)]"
          >
            {keyIcons[key] || key}
          </kbd>
        ))}
      </div>
      {label && (
        <span className="text-[11px] text-[var(--color-text-quaternary)]">
          {label}
        </span>
      )}
    </div>
  );
}

export function KeyboardHints() {
  return (
    <div className="flex items-center gap-4 px-4 py-2 border-t border-[var(--color-border)]">
      <KeyboardHint keys={['↑', '↓']} label="navigate" />
      <KeyboardHint keys={['↵']} label="select" />
      <KeyboardHint keys={['esc']} label="close" />
      <KeyboardHint keys={['⌘', 'K']} label="search" />
    </div>
  );
}
