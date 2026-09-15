import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const cwd = fileURLToPath(new URL("../", import.meta.url));
const env = { NODE_ENV: "test" };
const script = `
import assert from "node:assert/strict";
import { createNodeRequestHandler } from "./dist/server.js";
const handle = createNodeRequestHandler("fixture-only");
const health = await handle(new Request("http://fixture/health"), {incoming: {url: "/health"}});
assert.equal(health.status, 200);
assert.deepEqual(await health.json(), {ok: true});
const path = "/api/v1/skills/%252e%252e/repo/name";
const rejected = await handle(new Request("http://fixture" + path), {incoming: {url: path}});
assert.equal(rejected.status, 400);
`;
const smoke = spawnSync(
  process.execPath,
  ["--import", "./dist/instrument.js", "--input-type=module", "-e", script],
  { cwd, env, encoding: "utf8", timeout: 10000 },
);
assert.equal(smoke.status, 0, smoke.stderr);
const refused = spawnSync(
  process.execPath,
  ["--import", "./dist/instrument.js", "./dist/server.js"],
  { cwd, env, encoding: "utf8", timeout: 10000 },
);
assert.equal(refused.status, 1, refused.stderr);
assert.match(refused.stderr, /SKILLS_SH_API_KEY is not set/);
for (const name of ["instrument", "server"]) {
  const js = readFileSync(resolve(cwd, "dist", name + ".js"), "utf8");
  assert(!js.includes("sourceMappingURL="));
  const map = JSON.parse(readFileSync(resolve(cwd, "dist", name + ".js.map"), "utf8"));
  assert.equal(map.version, 3);
  assert(map.sources.length > 0);
  assert.equal(map.sources.length, map.sourcesContent.length);
}
process.stdout.write(
  "Compiled API: health 200; traversal 400; missing-key startup refused; external maps valid.\n",
);
