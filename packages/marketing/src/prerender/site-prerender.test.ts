// ============================================================================
// Skill Studio - Site prerender tests
// Flow: the build fills each HTML template through renderSitePage.
// Failure: a page ships with the wrong canonical URL, a missing title, an
// empty body, or noindex on a real page.
// ============================================================================

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { DOCS_PAGES } from "../docs/docs-pages";
import { renderSitePage, SITE_ROUTES } from "./site-prerender";
import { PAGE_TEMPLATES } from "./site-templates";

const SITE_ORIGIN = "https://useskillstudio.com";

function renderRoute(file: string): string {
  const template = readFileSync(fileURLToPath(new URL(`../../${file}`, import.meta.url)), "utf8");
  return renderSitePage(file, template);
}

function rootMarkup(html: string): string {
  const match = /<div id="root"[^>]*>([\s\S]*)<\/div>\s*(?:<script|<\/body>)/.exec(html);
  return match?.[1] ?? "";
}

describe("renderSitePage", () => {
  it("has a prerender route for every template Vite builds, and no extra routes", () => {
    expect(SITE_ROUTES.map((route) => route.file).sort()).toEqual([...PAGE_TEMPLATES].sort());
  });

  it.each(SITE_ROUTES.map((route) => route.file))(
    "fills %s with a title and page markup",
    (file) => {
      const html = renderRoute(file);

      expect(html).toMatch(/<title>[^<]+<\/title>/);
      expect(html).not.toContain("<!--site-head-->");
      expect(html).not.toContain("<!--site-theme-->");
      expect(rootMarkup(html).trim()).not.toBe("");
    },
  );

  it.each(DOCS_PAGES)("points canonical and og:url of $path at that page", (page) => {
    const html = renderRoute(`${page.path.slice(1)}index.html`);
    const url = `${SITE_ORIGIN}${page.path}`;

    expect(html).toContain(`<title>${page.title}</title>`);
    expect(html).toContain(`<link rel="canonical" href="${url}" />`);
    expect(html).toContain(`<meta property="og:url" content="${url}" />`);
    expect(html).toContain(`href="${SITE_ORIGIN}/${page.markdownPath}"`);
  });

  it("marks only the 404 page as noindex", () => {
    for (const { file } of SITE_ROUTES) {
      const isNoindex = renderRoute(file).includes('<meta name="robots" content="noindex" />');
      expect(isNoindex, file).toBe(file === "404.html");
    }
  });

  it("keeps the home page canonical at the site root", () => {
    const html = renderRoute("index.html");

    expect(html).toContain(`<link rel="canonical" href="${SITE_ORIGIN}/" />`);
    expect(html).toContain(`<meta property="og:url" content="${SITE_ORIGIN}/" />`);
  });
});
