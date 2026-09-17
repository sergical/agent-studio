import { build } from "esbuild";
import { fileURLToPath } from "node:url";

await build({
  absWorkingDir: fileURLToPath(new URL("./", import.meta.url)),
  entryPoints: ["src/instrument.ts", "src/server.ts"],
  outdir: "dist",
  platform: "node",
  target: "node22",
  format: "esm",
  bundle: true,
  packages: "external",
  sourcemap: "external",
  sourcesContent: true,
});
