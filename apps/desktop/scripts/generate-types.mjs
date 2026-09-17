// ============================================================================
// Skill Studio - generate-types
// Runs the `schema` Rust binary (apps/desktop/src-tauri/src/bin/schema.rs),
// which emits a JSON Schema for every Tauri wire type via `schemars`, then
// turns it into TypeScript with json-schema-to-typescript. Writes
// packages/lib/src/skill-types.generated.ts, which packages/lib/src/skill-types.ts
// re-exports from - see that file's header for why this replaced the old
// 733-line hand-written version.
//
// `npm run types:generate` (writes the file) and `npm run types:check` (fails
// on drift - see apps/desktop/package.json and the root `check` script) both
// call this script; `--check` selects the latter.
// ============================================================================
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync, existsSync, mkdtempSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import path from "node:path";
import { compile } from "json-schema-to-typescript";

const desktopDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const workspaceRoot = path.dirname(path.dirname(desktopDir));
const outputPath = path.join(workspaceRoot, "packages/lib/src/skill-types.generated.ts");

const schemaJson = execFileSync(
  "cargo",
  ["run", "--quiet", "-p", "skill-studio", "--bin", "schema"],
  { cwd: workspaceRoot, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
);
const schema = JSON.parse(schemaJson);

const banner = `// ============================================================================
// GENERATED FILE - do not edit by hand.
// Produced by \`npm run types:generate\` (apps/desktop/scripts/generate-types.mjs)
// from the Rust wire types in apps/desktop/src-tauri/src/skills/*.rs, via the
// \`schema\` binary and json-schema-to-typescript. Re-run that command after
// changing a #[derive(JsonSchema)] struct or enum; \`npm run check\` fails if
// this file drifts from the Rust source of truth.
// ============================================================================

`;

let ts = await compile(schema, "WireTypes", {
  bannerComment: "",
  additionalProperties: false,
  style: { semi: true },
});

// `WireTypes` exists only to give schemars one root that reaches every wire
// type (see schema.rs) - it is never constructed, so its own generated
// interface is dropped from the checked-in output.
ts = ts.replace(/export interface WireTypes \{[\s\S]*?\n\}\n\n?/, "");

// json-schema-to-typescript's own formatting doesn't match oxfmt's, so run
// the result through oxfmt (in a scratch file - oxfmt only formats files on
// disk) before writing or diffing it, or every fresh generation would fail
// `npm run format:check`.
const scratchDir = mkdtempSync(path.join(tmpdir(), "skill-types-generate-"));
const scratchPath = path.join(scratchDir, "skill-types.generated.ts");
writeFileSync(scratchPath, banner + ts);
execFileSync("npx", ["oxfmt", "--write", scratchPath], { cwd: workspaceRoot });
const finalContent = readFileSync(scratchPath, "utf8");
rmSync(scratchDir, { recursive: true, force: true });

if (process.argv.includes("--check")) {
  if (!existsSync(outputPath)) {
    process.stderr.write(`${outputPath} does not exist - run \`npm run types:generate\`.\n`);
    process.exit(1);
  }
  const current = readFileSync(outputPath, "utf8");
  if (current !== finalContent) {
    process.stderr.write(
      `${outputPath} is out of date with the Rust wire types - run \`npm run types:generate\` and commit the result.\n`,
    );
    process.exit(1);
  }
  process.stdout.write("skill-types.generated.ts is up to date.\n");
} else {
  writeFileSync(outputPath, finalContent);
  process.stdout.write(`Wrote ${outputPath}\n`);
}
