// ============================================================================
// PageShell - The slim header bar (breadcrumb, title, actions) and optional
// toolbar row every view sits under, plus the scrolling content area. The
// sidebar holds places; a page's own filter bar (if any) is the `toolbar`.
// ============================================================================

import type { ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { Button } from "@skill-studio/ui";

interface PageShellProps {
  title: string;
  /** Parent crumb, shown before the title as "Skills ›". Clicking it runs `onClick`. */
  parent?: { label: string; onClick: () => void };
  subtitle?: string;
  actions?: ReactNode;
  /** Thin row under the header: filters, tabs. Controls inside are 28px tall. */
  toolbar?: ReactNode;
  children: ReactNode;
  /** "narrow" (720px) for a detail-style page, e.g. a pack's detail view. */
  width?: "default" | "narrow";
}

/**
 * The first word in the header: a top-level page's title, or the parent crumb on a child page.
 * One disabled-or-enabled button for both, so "Skills" doesn't move when it becomes a link.
 */
function RootCrumb({ label, onClick }: { label: string; onClick?: () => void }) {
  return (
    <Button
      variant="ghost"
      size="xs"
      disabled={!onClick}
      // -7px = the 6px hover padding plus the button's 1px transparent border, so the text starts
      // at the header's own padding edge.
      className="-ml-[7px] h-6 min-w-0 rounded-sm px-1.5 text-small text-text-tertiary hover:text-text-primary disabled:text-text-primary disabled:opacity-100"
      onClick={onClick}
    >
      <span className="truncate">{label}</span>
    </Button>
  );
}

/**
 * A `<section>` with a one-line header (parent crumb, title, actions on the
 * right), an optional toolbar row, and a scrolling content area below.
 */
export function PageShell({
  title,
  parent,
  subtitle,
  actions,
  toolbar,
  children,
  width = "default",
}: PageShellProps) {
  return (
    <section className="flex min-h-0 flex-1 flex-col">
      {/* With a toolbar, header and toolbar read as one block: only the toolbar draws the line. The
          header keeps a transparent border so its content centres the same with or without one. */}
      <header
        className={`flex h-10 shrink-0 items-center justify-between gap-3 border-b px-4 ${toolbar ? "border-transparent" : "border-border-subtle"}`}
      >
        <div className="flex min-w-0 items-center gap-1.5 text-small">
          {parent ? (
            <>
              <RootCrumb label={parent.label} onClick={parent.onClick} />
              <ChevronRight size={12} className="shrink-0 text-text-quaternary" />
              <h1 className="m-0 truncate text-small font-medium text-text-primary">{title}</h1>
            </>
          ) : (
            <h1 className="m-0 flex min-w-0">
              <RootCrumb label={title} />
            </h1>
          )}
          {subtitle && (
            <span className="min-w-0 truncate text-small text-text-tertiary">· {subtitle}</span>
          )}
        </div>
        {actions && <div className="flex shrink-0 items-center gap-1.5">{actions}</div>}
      </header>
      {toolbar && (
        <div className="flex h-10 shrink-0 items-center gap-2 border-b border-border-subtle px-4 [--control-height:28px]">
          {toolbar}
        </div>
      )}
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div
          className={`mx-auto flex w-full flex-col gap-5 px-6 pt-5 pb-7 ${width === "narrow" ? "max-w-180" : "max-w-300"}`}
        >
          {children}
        </div>
      </div>
    </section>
  );
}
