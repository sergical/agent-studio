// ============================================================================
// Skill Studio - 404 page
// Served by the Worker for any path that has no file in dist
// ============================================================================

import * as stylex from "@stylexjs/stylex";

import { getPaletteTheme } from "../PaletteThemes.stylex";
import { SiteFooter, SiteHeader, siteLayout } from "../SiteChrome";
import { DOCS_URL } from "../site-links";
import { notFoundStyles as styles } from "./docs-page-styles";

export function NotFoundPage() {
  return (
    <div id="top" {...stylex.props(getPaletteTheme("violet"), siteLayout.page)}>
      <SiteHeader page="other" showThemeToggle />
      <main {...stylex.props(siteLayout.container, styles.main)}>
        <p {...stylex.props(styles.code)}>404</p>
        <h1 {...stylex.props(styles.title)}>Page not found</h1>
        <p {...stylex.props(styles.text)}>This page does not exist. It may have moved.</p>
        <div {...stylex.props(styles.links)}>
          <a href="/" {...stylex.props(styles.link)}>
            Go to the homepage
          </a>
          <a href={DOCS_URL} {...stylex.props(styles.link)}>
            Read the docs
          </a>
        </div>
      </main>
      <SiteFooter page="other" />
    </div>
  );
}
