import { clsx } from 'clsx';

interface SectionHeaderProps {
  title: string;
  count?: number;
  className?: string;
}

export function SectionHeader({ title, count, className }: SectionHeaderProps) {
  return (
    <div className={clsx(
      'flex items-center gap-2 px-3 py-2',
      className
    )}>
      <span className="text-[11px] font-semibold uppercase tracking-wider text-[var(--color-text-quaternary)]">
        {title}
      </span>
      {count !== undefined && (
        <span className="text-[10px] font-medium text-[var(--color-text-quaternary)] bg-[var(--color-bg-tertiary)] px-1.5 py-0.5 rounded">
          {count}
        </span>
      )}
    </div>
  );
}
