// ============================================================================
// Skill Studio - Docs page
// Prerendered to static HTML; the client adds only the copy and theme handlers
// ============================================================================

import * as stylex from "@stylexjs/stylex";

import { getPaletteTheme } from "../PaletteThemes.stylex";
import { SiteFooter, SiteHeader, siteLayout } from "../SiteChrome";
import { docsPageStyles as styles } from "./docs-page-styles";
import { renderDocsMarkdown } from "./docs-markdown";
import { DOCS_PAGES, type DocsPageEntry } from "./docs-pages";

interface DocsPageProps {
  page: DocsPageEntry;
}

export function DocsPage({ page }: DocsPageProps) {
  const article = stylex.props(styles.article);

  return (
    <div id="top" {...stylex.props(getPaletteTheme("violet"), siteLayout.page)}>
      <SiteHeader page="docs" showThemeToggle />
      <div {...stylex.props(siteLayout.container, styles.layout)}>
        <nav {...stylex.props(styles.nav)} aria-label="Docs">
          <span {...stylex.props(styles.navHeading)}>Docs</span>
          {DOCS_PAGES.map((entry) => (
            <a
              key={entry.slug}
              href={entry.path}
              aria-current={entry.slug === page.slug ? "page" : undefined}
              {...stylex.props(styles.navLink, entry.slug === page.slug && styles.navLinkCurrent)}
            >
              {entry.navLabel}
            </a>
          ))}
        </nav>
        <main>
          <article
            className={`docs-prose ${article.className ?? ""}`}
            style={article.style}
            dangerouslySetInnerHTML={{ __html: renderDocsMarkdown(page.markdown) }}
          />
          <a href={`/${page.markdownPath}`} {...stylex.props(styles.markdownLink)}>
            View this page as Markdown
          </a>
        </main>
      </div>
      <SiteFooter page="docs" />
    </div>
  );
}
