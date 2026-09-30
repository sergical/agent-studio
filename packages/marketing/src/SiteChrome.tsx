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
        {showThemeToggle && <ThemeToggle />}
      </div>
    </header>
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
    <footer {...stylex.props(siteLayout.container, styles.footer)}>
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
