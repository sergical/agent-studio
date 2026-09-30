// ============================================================================
// Skill Studio - Docs and 404 page styles
// Kept apart from the page components so docs-main.ts can pull these StyleX
// rules into the client CSS without bundling React
// ============================================================================

import * as stylex from "@stylexjs/stylex";

import { siteTokens } from "../SiteTheme.stylex";

export const docsPageStyles = stylex.create({
  layout: {
    display: "grid",
    gap: 56,
    gridTemplateColumns: "200px minmax(0, 1fr)",
    paddingBlock: "32px 96px",
    "@media (max-width: 860px)": {
      gap: 28,
      gridTemplateColumns: "minmax(0, 1fr)",
      paddingBlock: "8px 72px",
    },
  },
  nav: {
    alignSelf: "start",
    display: "flex",
    flexDirection: "column",
    gap: 2,
    position: "sticky",
    top: 24,
    "@media (max-width: 860px)": {
      borderBottomColor: siteTokens.border,
      borderBottomStyle: "solid",
      borderBottomWidth: 1,
      flexDirection: "row",
      gap: 4,
      marginInline: -4,
      overflowX: "auto",
      paddingBottom: 12,
      position: "static",
    },
  },
  navHeading: {
    color: siteTokens.muted,
    fontSize: 12,
    fontWeight: 600,
    letterSpacing: "0.04em",
    marginBottom: 8,
    paddingInline: 12,
    textTransform: "uppercase",
    "@media (max-width: 860px)": { display: "none" },
  },
  navLink: {
    borderRadius: 8,
    color: siteTokens.muted,
    flexShrink: 0,
    fontSize: 14,
    paddingBlock: 8,
    paddingInline: 12,
    textDecoration: "none",
    transition: "color 150ms ease-out, background-color 150ms ease-out",
    whiteSpace: "nowrap",
    ":hover": { color: siteTokens.text },
  },
  navLinkCurrent: {
    backgroundColor: siteTokens.surface,
    color: siteTokens.text,
  },
  // docs-prose.css is plain CSS and cannot read StyleX variables by name, so the article
  // re-exports the theme colours it needs as ordinary custom properties.
  article: {
    "--docs-accent": siteTokens.accent,
    "--docs-border": siteTokens.border,
    "--docs-muted": siteTokens.muted,
    "--docs-surface": siteTokens.surface,
    "--docs-text": siteTokens.text,
    maxWidth: 720,
    minWidth: 0,
  },
  markdownLink: {
    color: siteTokens.muted,
    display: "inline-block",
    fontSize: 13,
    marginTop: 48,
    textDecoration: "underline",
    textUnderlineOffset: 3,
    ":hover": { color: siteTokens.text },
  },
});

export const notFoundStyles = stylex.create({
  main: {
    display: "flex",
    flexDirection: "column",
    gap: 16,
    minHeight: "60vh",
    justifyContent: "center",
    paddingBlock: 96,
  },
  code: { color: siteTokens.muted, fontSize: 14, margin: 0 },
  title: {
    fontSize: "clamp(36px, 6vw, 56px)",
    fontWeight: 600,
    letterSpacing: "-0.03em",
    lineHeight: 1.05,
    margin: 0,
  },
  text: { color: siteTokens.muted, fontSize: 17, lineHeight: 1.6, margin: 0, maxWidth: 520 },
  links: { display: "flex", flexWrap: "wrap", gap: 12, marginTop: 12 },
  link: {
    alignItems: "center",
    borderColor: siteTokens.border,
    borderRadius: 10,
    borderStyle: "solid",
    borderWidth: 1,
    color: siteTokens.text,
    display: "inline-flex",
    fontSize: 14,
    minHeight: 44,
    paddingInline: 16,
    textDecoration: "none",
    transition: "border-color 150ms ease-out",
    ":hover": { borderColor: siteTokens.muted },
  },
});
