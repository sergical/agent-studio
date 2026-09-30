// ============================================================================
// Skill Studio - Docs markdown
// Turns a docs page's markdown into HTML at prerender time
// ============================================================================

import { Marked } from "marked";

function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function headingId(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
}

const docsMarked = new Marked({
  gfm: true,
  renderer: {
    // The button is plain markup because the docs ship no React on the client;
    // docs-main.ts handles every copy button with one delegated click listener.
    code({ text, lang }) {
      const language = lang ? ` class="language-${escapeHtml(lang)}"` : "";
      return `<div class="docs-code"><pre><code${language}>${escapeHtml(text)}</code></pre><button type="button" class="docs-copy" data-copy=""><span data-copy-label="" aria-live="polite">Copy</span></button></div>\n`;
    },
    heading({ tokens, depth, text }) {
      const content = this.parser.parseInline(tokens);
      if (depth === 1) return `<h1>${content}</h1>\n`;
      return `<h${depth} id="${headingId(text)}">${content}</h${depth}>\n`;
    },
  },
});

export function renderDocsMarkdown(markdown: string): string {
  return docsMarked.parse(markdown, { async: false });
}
