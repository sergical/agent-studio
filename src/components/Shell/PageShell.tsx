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
 * `<section class="page-shell">` with a header row (title, optional
 * subtitle, actions on the right) and the content below. The only source of
 * page padding - views must not add their own.
 */
export function PageShell({
  title,
  subtitle,
  actions,
  children,
  width = "default",
}: PageShellProps) {
  return (
    <section className={`page-shell ${width === "narrow" ? "narrow" : ""}`}>
      <header className="page-shell-header">
        <div className="page-shell-heading">
          <h1>{title}</h1>
          {subtitle && <p className="page-shell-subtitle">{subtitle}</p>}
        </div>
        {actions && <div className="page-shell-actions">{actions}</div>}
      </header>
      {children}
    </section>
  );
}
