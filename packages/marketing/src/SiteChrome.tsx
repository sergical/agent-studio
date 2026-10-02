// ============================================================================
// Skill Studio - Site chrome
// Header and footer shared by the homepage, the docs and the 404 page
// ============================================================================

import * as stylex from "@stylexjs/stylex";
import { Moon, Sun } from "lucide-react";

import { LogoLockup } from "./MarketingBrand";
import { DOCS_URL, GITHUB_REPOSITORY_URL } from "./site-links";
import { siteTokens } from "./SiteTheme.stylex";

type SitePage = "home" | "docs" | "other";

interface SiteHeaderProps {
  page: SitePage;
  /** The homepage has its own toggle inside the product demo. */
  showThemeToggle?: boolean;
}

export function SiteHeader({ page, showThemeToggle = false }: SiteHeaderProps) {
  return (
    <header {...stylex.props(siteLayout.container, styles.header)}>
      <LogoLockup href={page === "home" ? "#top" : "/"} compact />
      <div {...stylex.props(styles.headerEnd)}>
        <nav {...stylex.props(styles.nav)} aria-label="Main navigation">
          <a href="/#how-it-works" {...stylex.props(styles.navLink)}>
            How it works
          </a>
          <a
            href={DOCS_URL}
            aria-current={page === "docs" ? "page" : undefined}
            {...stylex.props(styles.navLink, page === "docs" && styles.navLinkCurrent)}
          >
            Docs
          </a>
          <a href="/#download" {...stylex.props(styles.navLink)}>
            Download
          </a>
        </nav>
        <a
          href={GITHUB_REPOSITORY_URL}
          target="_blank"
          rel="noreferrer"
          aria-label="Skill Studio on GitHub"
          {...stylex.props(styles.iconLink)}
        >
          <GitHubMark />
        </a>
        {showThemeToggle && <ThemeToggle />}
      </div>
    </header>
  );
}

// lucide-react dropped its brand icons, so the GitHub mark is drawn here.
function GitHubMark() {
  return (
    <svg aria-hidden="true" viewBox="0 0 16 16" width="18" height="18" fill="currentColor">
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8Z" />
    </svg>
  );
}

// The page markup is prerendered without knowing the theme, so both icons ship and
// marketing-site.css shows the one that matches <html data-site-theme>.
function ThemeToggle() {
  return (
    <button
      type="button"
      data-theme-toggle=""
      aria-label="Switch between light and dark theme"
      {...stylex.props(styles.themeToggle)}
    >
      <Moon aria-hidden="true" size={16} data-theme-icon="dark" />
      <Sun aria-hidden="true" size={16} data-theme-icon="light" />
    </button>
  );
}

interface SiteFooterProps {
  page: SitePage;
}

export function SiteFooter({ page }: SiteFooterProps) {
  return (
    <footer
      {...stylex.props(siteLayout.container, styles.footer, page === "home" && styles.footerHome)}
    >
      <LogoLockup href={page === "home" ? "#top" : "/"} compact />
      <nav {...stylex.props(styles.footerLinks)} aria-label="Footer navigation">
        <a href={DOCS_URL} {...stylex.props(styles.footerLink)}>
          Docs
        </a>
        <a
          href={GITHUB_REPOSITORY_URL}
          target="_blank"
          rel="noreferrer"
          {...stylex.props(styles.footerLink)}
        >
          Source
        </a>
        <a href="#top" {...stylex.props(styles.footerLink)}>
          Back to top
        </a>
      </nav>
    </footer>
  );
}

export const siteLayout = stylex.create({
  page: {
    backgroundColor: siteTokens.background,
    color: siteTokens.text,
    minHeight: "100vh",
    overflow: "clip",
  },
  container: {
    boxSizing: "border-box",
    marginInline: "auto",
    maxWidth: 1280,
    paddingInline: 28,
    width: "100%",
    "@media (max-width: 600px)": { paddingInline: 20 },
  },
});

const styles = stylex.create({
  header: {
    alignItems: "center",
    display: "flex",
    justifyContent: "space-between",
    paddingBlock: 22,
    "@media (max-width: 700px)": { paddingBlock: 16 },
  },
  headerEnd: { alignItems: "center", display: "flex", gap: 26 },
  nav: { display: "flex", gap: 26, "@media (max-width: 700px)": { display: "none" } },
  navLink: {
    color: siteTokens.muted,
    fontSize: 14,
    textDecoration: "none",
    transition: "color 150ms ease-out",
    ":hover": { color: siteTokens.text },
  },
  navLinkCurrent: { color: siteTokens.text },
  iconLink: {
    alignItems: "center",
    borderRadius: 10,
    color: siteTokens.muted,
    display: "inline-flex",
    height: 40,
    justifyContent: "center",
    marginInline: -11,
    transition: "color 150ms ease-out",
    width: 40,
    ":hover": { color: siteTokens.text },
  },
  themeToggle: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderColor: siteTokens.border,
    borderRadius: 10,
    borderStyle: "solid",
    borderWidth: 1,
    color: siteTokens.muted,
    display: "inline-flex",
    height: 40,
    justifyContent: "center",
    padding: 0,
    transition: "color 150ms ease-out, border-color 150ms ease-out, transform 150ms ease-out",
    width: 40,
    ":hover": { borderColor: siteTokens.muted, color: siteTokens.text },
    ":active": { transform: "scale(.94)" },
  },
  footer: {
    alignItems: "center",
    borderTopColor: siteTokens.border,
    borderTopStyle: "solid",
    borderTopWidth: 1,
    display: "flex",
    justifyContent: "space-between",
    paddingBlock: 28,
    paddingBottom: "max(28px, env(safe-area-inset-bottom))",
  },
  // On the homepage the footer sits in the closing section, which already separates it.
  footerHome: { borderTopWidth: 0, paddingBottom: 28 },
  footerLinks: { alignItems: "center", display: "flex", gap: 22 },
  footerLink: {
    color: siteTokens.muted,
    fontSize: 12,
    minHeight: 44,
    alignItems: "center",
    display: "inline-flex",
    textDecoration: "none",
    transition: "color 150ms ease-out",
    ":hover": { color: siteTokens.text },
  },
});
