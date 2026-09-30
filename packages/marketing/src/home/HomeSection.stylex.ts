import * as stylex from "@stylexjs/stylex";

import { siteTokens } from "../SiteTheme.stylex";

export const homeSectionStyles = stylex.create({
  section: {
    paddingTop: 124,
    "@media (max-width: 600px)": { paddingTop: 86 },
  },
  title: {
    fontSize: 34,
    fontWeight: 640,
    lineHeight: 1.08,
    letterSpacing: "-.05em",
    margin: "0 0 36px",
    textWrap: "balance",
    "@media (max-width: 760px)": { fontSize: 32 },
  },
  code: {
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
    fontSize: ".92em",
  },
  textLink: {
    alignItems: "center",
    color: siteTokens.text,
    display: "inline-flex",
    fontSize: 14,
    fontWeight: 600,
    gap: 6,
    minHeight: 44,
    textDecoration: "none",
    transition: "color 150ms ease-out",
    ":hover": { color: siteTokens.accent },
  },
});
