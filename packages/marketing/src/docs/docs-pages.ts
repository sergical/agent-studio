// ============================================================================
// Skill Studio - Docs pages
// One entry per docs page: its URL, nav label, meta text and markdown source
// ============================================================================

import cliMarkdown from "./content/cli.md?raw";
import getStartedMarkdown from "./content/get-started.md?raw";
import mcpMarkdown from "./content/mcp.md?raw";

export interface DocsPageEntry {
  slug: "get-started" | "cli" | "mcp";
  /** The page URL, with a trailing slash. */
  path: string;
  /** Where the prerender writes the plain markdown copy, relative to dist. */
  markdownPath: string;
  navLabel: string;
  title: string;
  description: string;
  markdown: string;
}

export const DOCS_PAGES: DocsPageEntry[] = [
  {
    slug: "get-started",
    path: "/docs/",
    markdownPath: "docs/index.md",
    navLabel: "Get started",
    title: "Get started with Skill Studio",
    description:
      "Install Skill Studio, then find, fix and park your agent skills. Every change can be undone.",
    markdown: getStartedMarkdown,
  },
  {
    slug: "cli",
    path: "/docs/cli/",
    markdownPath: "docs/cli.md",
    navLabel: "CLI",
    title: "Skill Studio CLI",
    description:
      "Check, fix, park and update agent skills from the terminal with npx skill-studio.",
    markdown: cliMarkdown,
  },
  {
    slug: "mcp",
    path: "/docs/mcp/",
    markdownPath: "docs/mcp.md",
    navLabel: "MCP server",
    title: "Skill Studio MCP server",
    description:
      "Connect Skill Studio to Claude Code, Codex, Cursor or OpenCode, so your agent can tidy its own skills.",
    markdown: mcpMarkdown,
  },
];
