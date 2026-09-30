import * as stylex from "@stylexjs/stylex";
import { AppWindow, Plug, Terminal, type LucideIcon } from "lucide-react";

import { siteTokens } from "../SiteTheme.stylex";
import { CLI_DOCS_URL, DOWNLOAD_URL, MCP_DOCS_URL } from "../site-links";
import { homeSectionStyles as section } from "./HomeSection.stylex";

interface UsageCard {
  id: string;
  title: string;
  icon: LucideIcon;
  text: string;
  command?: string;
  link: { href: string; label: string };
}

const cards: ReadonlyArray<UsageCard> = [
  {
    id: "app",
    title: "App",
    icon: AppWindow,
    text: "Browse, fix and park skills on your Mac.",
    link: { href: DOWNLOAD_URL, label: "Download for macOS" },
  },
  {
    id: "cli",
    title: "CLI",
    icon: Terminal,
    text: "Check every skill from your terminal.",
    command: "npx skill-studio diagnose",
    link: { href: CLI_DOCS_URL, label: "CLI docs" },
  },
  {
    id: "mcp",
    title: "MCP",
    icon: Plug,
    text: "Let your agent tidy its own skills.",
    command: "claude mcp add skill-studio -- npx -y skill-studio mcp",
    link: { href: MCP_DOCS_URL, label: "MCP docs" },
  },
];

export function UsageSection() {
  return (
    <>
      <h2 {...stylex.props(section.title)}>Use it your way.</h2>
      <div {...stylex.props(styles.grid)}>
        {cards.map(({ id, title, icon: Icon, text, command, link }) => (
          <article key={id} {...stylex.props(styles.card)}>
            <h3 {...stylex.props(styles.cardTitle)}>
              <Icon aria-hidden="true" size={18} {...stylex.props(styles.cardIcon)} />
              {title}
            </h3>
            <p {...stylex.props(styles.cardText)}>{text}</p>
            {command && (
              <pre {...stylex.props(styles.command)}>
                <code {...stylex.props(section.code, styles.commandText)}>{command}</code>
              </pre>
            )}
            <a href={link.href} {...stylex.props(section.textLink, styles.cardLink)}>
              {link.label}
              <span aria-hidden="true">→</span>
            </a>
          </article>
        ))}
      </div>
    </>
  );
}

const styles = stylex.create({
  grid: {
    display: "grid",
    gap: 18,
    gridTemplateColumns: "repeat(3, minmax(0, 1fr))",
    "@media (max-width: 900px)": { gridTemplateColumns: "1fr" },
  },
  card: {
    backgroundColor: siteTokens.surface,
    borderColor: siteTokens.border,
    borderRadius: 16,
    borderStyle: "solid",
    borderWidth: 1,
    display: "flex",
    flexDirection: "column",
    gap: 14,
    minWidth: 0,
    padding: "24px 24px 12px",
    "@media (max-width: 600px)": { padding: "20px 20px 8px" },
  },
  cardTitle: {
    alignItems: "center",
    display: "flex",
    fontSize: 17,
    fontWeight: 640,
    gap: 10,
    letterSpacing: "-.02em",
    margin: 0,
  },
  cardIcon: { color: siteTokens.accent, flexShrink: 0 },
  cardText: {
    color: siteTokens.muted,
    fontSize: 15,
    lineHeight: 1.55,
    margin: 0,
  },
  command: {
    backgroundColor: siteTokens.background,
    borderColor: siteTokens.border,
    borderRadius: 10,
    borderStyle: "solid",
    borderWidth: 1,
    margin: 0,
    padding: "12px 14px",
    whiteSpace: "pre-wrap",
  },
  commandText: {
    color: siteTokens.text,
    fontSize: 13,
    lineHeight: 1.5,
    overflowWrap: "anywhere",
    // One click selects the whole command, so it copies in one go.
    userSelect: "all",
  },
  cardLink: { marginTop: "auto" },
});
