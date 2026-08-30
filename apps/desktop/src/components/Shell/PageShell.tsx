// ============================================================================
// PageShell - Consistent header (title, optional subtitle, actions) and
// padding for every main view. The sidebar holds places; a page's own filter
// bar (if any) lives just below this header, inside `children`.
// ============================================================================

import type { ReactNode } from "react";

interface PageShellProps {
  title: string;
  subtitle?: string;
  actions?: ReactNode;
  children: ReactNode;
  /** "narrow" (720px) for a detail-style page, e.g. a pack's detail view. */
  width?: "default" | "narrow";
}

/**
 * A `<section>` with a header row (title, optional subtitle, actions on the
 * right) and the content below. The only source of page padding - views
 * must not add their own.
 */
export function PageShell({
  title,
  subtitle,
  actions,
  children,
  width = "default",
}: PageShellProps) {
  return (
    <section
      className={`mx-auto flex w-full flex-col gap-6 px-8 py-7 ${width === "narrow" ? "max-w-180" : "max-w-300"}`}
    >
      <header className="flex items-start justify-between gap-4">
        <div className="flex min-w-0 flex-col gap-1">
          <h1 className="m-0 text-title font-semibold text-wrap-balance text-text-primary">
            {title}
          </h1>
          {subtitle && (
            <p className="m-0 text-body text-wrap-pretty text-text-secondary">{subtitle}</p>
          )}
        </div>
        {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
      </header>
      {children}
    </section>
  );
}
