// ============================================================================
// Skill Studio - Server
// Proxies skills.sh's authenticated /api/v1 surface so the desktop app never
// needs its own key: skills.sh API keys aren't per-account, so the desktop
// app can't ship one. This server holds the one key (SKILLS_SH_API_KEY, from
// the repo-root .env) and forwards discovery requests on the desktop app's
// behalf.
// ============================================================================

import { serve } from "@hono/node-server";
import { Hono } from "hono";
import type { ContentfulStatusCode } from "hono/utils/http-status";

const UPSTREAM_BASE = "https://skills.sh/api/v1";
const DEFAULT_PORT = 8787;
const HOST = "127.0.0.1";

interface RawNodeRequestEnvironment {
  incoming: {
    url?: string;
  };
}

/** One proxied GET's outcome: the upstream's own status and JSON body when
 * it responded at all (any status, not just 2xx), or a synthetic `{ error }`
 * body when the request to skills.sh itself couldn't be made. */
export interface ProxyResult {
  status: number;
  body: unknown;
}

/** The exact skills.sh URL for `path` (e.g. `"/skills/search"`) and a
 * verbatim `search` string (e.g. `"?q=foo&limit=10"`, or `""`) - exported so
 * tests can check the URL a stubbed `fetch` was called with. */
export function upstreamUrl(path: string, search: string): string {
  return `${UPSTREAM_BASE}${path}${search}`;
}

/** Proxies one GET request to skills.sh with `apiKey` as a bearer token,
 * relaying the upstream's status and JSON body verbatim - a non-2xx upstream
 * response is still relayed as-is. Only a failure to reach skills.sh at all
 * (network error, DNS, etc.) maps to a `{ error }` body. */
export async function proxyGet(apiKey: string, path: string, search: string): Promise<ProxyResult> {
  let response: Response;
  try {
    response = await fetch(upstreamUrl(path, search), {
      headers: { Authorization: `Bearer ${apiKey}` },
    });
  } catch (e) {
    return {
      status: 502,
      body: { error: e instanceof Error ? e.message : "Failed to reach skills.sh" },
    };
  }
  const body = await response
    .json()
    .catch(() => ({ error: "skills.sh returned a non-JSON response" }));
  return { status: response.status, body };
}

/** Reads and validates `SKILLS_SH_API_KEY` - the one thing this server
 * refuses to start without. Never logged. */
export function requireApiKey(env: NodeJS.ProcessEnv): string {
  const key = env.SKILLS_SH_API_KEY?.trim();
  if (!key) {
    throw new Error(
      "SKILLS_SH_API_KEY is not set. Add it to the repo-root .env, then run `npm run dev:server`.",
    );
  }
  return key;
}

/** True only when no decoding layer turns a path segment into traversal or a separator. */
function decodesToSafePathSegment(segment: string): boolean {
  let decodedLayer = segment;
  while (true) {
    if (
      decodedLayer.length === 0 ||
      decodedLayer === "." ||
      decodedLayer === ".." ||
      decodedLayer.includes("/") ||
      decodedLayer.includes("\\")
    ) {
      return false;
    }
    const nextLayer = decodedLayer.replace(/%([0-9a-f]{2})/gi, (_escape, hex: string) =>
      String.fromCharCode(Number.parseInt(hex, 16)),
    );
    if (nextLayer === decodedLayer) return true;
    decodedLayer = nextLayer;
  }
}

/** True only when a raw path segment has valid percent encoding and stays safe when decoded. */
function isSafeRawPathSegment(segment: string): boolean {
  try {
    decodeURIComponent(segment);
  } catch {
    return false;
  }
  return decodesToSafePathSegment(segment);
}

/** Validates the unnormalized Node request target before Hono can route a normalized URL. */
export function isAllowedRawRequestTarget(rawTarget: string | undefined): boolean {
  if (!rawTarget) return false;

  const queryStart = rawTarget.indexOf("?");
  const rawPath = queryStart === -1 ? rawTarget : rawTarget.slice(0, queryStart);
  const rawQuery = queryStart === -1 ? "" : rawTarget.slice(queryStart + 1);
  try {
    decodeURIComponent(rawQuery);
  } catch {
    return false;
  }
  const segments = rawPath.split("/");
  if (segments[0] !== "" || segments.slice(1).some((segment) => !isSafeRawPathSegment(segment))) {
    return false;
  }

  const isApiV1 = segments[1] === "api" && segments[2] === "v1";
  if (!isApiV1) return true;

  const isSkillsRoute = segments[3] === "skills";
  const isListRoute = isSkillsRoute && segments.length === 4;
  const isSearchRoute = isSkillsRoute && segments.length === 5 && segments[4] === "search";
  const isDetailRoute = isSkillsRoute && segments.length === 7;
  return isListRoute || isSearchRoute || isDetailRoute;
}

/** Detects malformed first-pass percent encoding before Hono's tolerant parameter decoding hides it. */
function hasMalformedSkillDetailEncoding(url: string): boolean {
  const encodedSegments = new URL(url).pathname.split("/").slice(4);
  if (encodedSegments.length !== 3) return true;
  return encodedSegments.some((segment) => {
    try {
      decodeURIComponent(segment);
      return false;
    } catch {
      return true;
    }
  });
}

/** Builds the Hono app for `apiKey` - split out from `main` so tests can
 * exercise routes without starting a real listener. */
export function createApp(apiKey: string): Hono {
  const app = new Hono();

  app.use("*", async (c, next) => {
    const start = Date.now();
    await next();
    const ms = Date.now() - start;
    process.stdout.write(`${c.req.method} ${c.req.path} ${c.res.status} ${ms}ms\n`);
  });

  app.get("/health", (c) => c.json({ ok: true }));

  app.get("/api/v1/skills", async (c) => {
    const { status, body } = await proxyGet(apiKey, "/skills", new URL(c.req.url).search);
    // SAFETY: `status` is skills.sh's own response status, always a valid
    // HTTP status code - Hono's `ContentfulStatusCode` union just doesn't
    // widen back to `number`.
    return c.json(body, status as ContentfulStatusCode);
  });

  app.get("/api/v1/skills/search", async (c) => {
    const { status, body } = await proxyGet(apiKey, "/skills/search", new URL(c.req.url).search);
    // SAFETY: see the /api/v1/skills handler above.
    return c.json(body, status as ContentfulStatusCode);
  });

  app.get("/api/v1/skills/:owner/:repo/:slug", async (c) => {
    const { owner, repo, slug } = c.req.param();
    const segments = [owner, repo, slug];
    if (hasMalformedSkillDetailEncoding(c.req.url) || !segments.every(decodesToSafePathSegment)) {
      return c.json({ error: "Invalid skill detail path" }, 400);
    }
    const { status, body } = await proxyGet(
      apiKey,
      `/skills/${segments.map((segment) => encodeURIComponent(segment)).join("/")}`,
      new URL(c.req.url).search,
    );
    // SAFETY: see the /api/v1/skills handler above.
    return c.json(body, status as ContentfulStatusCode);
  });

  app.get("/api/v1/*", (c) => c.json({ error: "Invalid skill detail path" }, 400));

  return app;
}

/** Creates the production Node fetch seam that rejects unsafe raw targets before Hono routing. */
export function createNodeRequestHandler(apiKey: string) {
  const app = createApp(apiKey);
  return (request: Request, env: RawNodeRequestEnvironment): Response | Promise<Response> => {
    if (!isAllowedRawRequestTarget(env.incoming.url)) {
      return Response.json({ error: "Invalid request path" }, { status: 400 });
    }
    return app.fetch(request, env);
  };
}

function main() {
  const apiKey = requireApiKey(process.env);
  const port = Number(process.env.PORT) || DEFAULT_PORT;
  serve({ fetch: createNodeRequestHandler(apiKey), port, hostname: HOST }, (info) => {
    process.stdout.write(`Skill Studio server listening on http://${HOST}:${info.port}\n`);
  });
}

// Only start the server when this file is run directly (`tsx src/server.ts`),
// not when a test imports its exports.
if (import.meta.url === `file://${process.argv[1]}`) {
  main();
}
