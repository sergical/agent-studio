// Runs after both Vite builds. Fills every built HTML page with its prerendered markup and
// meta tags, writes the markdown copy of each docs page and llms.txt, then removes the
// server build, which the site does not serve.
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const packageRoot = fileURLToPath(new URL("..", import.meta.url));
const distDir = join(packageRoot, "dist");
const serverDir = join(packageRoot, "dist-ssr");

const { SITE_ROUTES, renderLlmsText, renderSitePage } = await import(
  pathToFileURL(join(serverDir, "site-prerender.js")).href
);

for (const route of SITE_ROUTES) {
  const htmlPath = join(distDir, route.file);
  const template = await readFile(htmlPath, "utf8");
  await writeFile(htmlPath, renderSitePage(route.file, template));

  if (route.markdownFile) {
    const markdownPath = join(distDir, route.markdownFile);
    await mkdir(dirname(markdownPath), { recursive: true });
    await writeFile(markdownPath, route.markdown);
  }
}

await writeFile(join(distDir, "llms.txt"), renderLlmsText());
await rm(serverDir, { recursive: true, force: true });

process.stdout.write(`Prerendered ${SITE_ROUTES.length} pages into dist\n`);
