// ============================================================================
// Skill Studio - Skills Proxy App
// The runtime-neutral Hono app: proxies skills.sh's authenticated /api/v1
// surface so the desktop app never needs its own key (skills.sh keys aren't
// per-account). `src/server.ts` (Node, `@hono/node-server`) and
// `src/worker.ts` (Cloudflare Workers) both build this app and differ only in
// how they start listening and which `limiter`/`cache` they inject - the
// Worker's is public, so it rate-limits and edge-caches; the Node dev server
// passes neither and keeps today's unrestricted local behaviour.
// ============================================================================

import { Hono } from "hono";
import type { ContentfulStatusCode } from "hono/utils/http-status";

const UPSTREAM_BASE = "https://skills.sh/api/v1";

/** One proxied GET's outcome: the upstream's own status and JSON body when
 * it responded at all (any status, not just 2xx), or a synthetic `{ error }`
 * body when the request to skills.sh itself couldn't be made. */
interface ProxyResult {
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
export async function proxyGet(
  apiKey: string,
  path: string,
  search: string,
  fetchImpl: typeof fetch = fetch,
): Promise<ProxyResult> {
  let response: Response;
  try {
    response = await fetchImpl(upstreamUrl(path, search), {
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
export function isSafeRawPathSegment(segment: string): boolean {
  try {
    decodeURIComponent(segment);
  } catch {
    return false;
  }
  return decodesToSafePathSegment(segment);
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

/** A per-caller rate limiter, matching the shape of a Workers Rate Limiting
 * binding (`env.RATE_LIMITER`) closely enough that tests can fake it without
 * Miniflare. */
export interface RateLimiter {
  limit(options: { key: string }): Promise<{ success: boolean }>;
}

/** A response cache, matching the shape of the Workers Cache API
 * (`caches.default`) closely enough that tests can fake it in-memory. */
export interface ResponseCache {
  match(request: Request): Promise<Response | undefined>;
  put(request: Request, response: Response): Promise<void>;
}

interface CreateSkillsProxyAppOptions {
  apiKey: string;
  /** Defaults to the global `fetch` - overridable so tests never hit the network. */
  fetch?: typeof fetch;
  /** Only set on the public Worker entry; the Node dev server leaves this unset. */
  limiter?: RateLimiter;
  /** Only set on the public Worker entry; the Node dev server leaves this unset. */
  cache?: ResponseCache;
}

const RATE_LIMIT_WINDOW_SECONDS = 60;

/** Edge-cache lifetime per route family, in seconds - list/search results
 * churn faster than a single skill's detail page. */
function cacheTtlSecondsFor(path: string): number {
  return path === "/api/v1/skills" || path === "/api/v1/skills/search" ? 300 : 3600;
}

/** Builds the Hono app for `apiKey` - split out from each runtime's entry so
 * tests can exercise routes without starting a real listener, and so the
 * Node and Worker entries share one implementation. */
export function createSkillsProxyApp({
  apiKey,
  fetch: fetchImpl = fetch,
  limiter,
  cache,
}: CreateSkillsProxyAppOptions): Hono {
  const app = new Hono();

  app.use("*", async (c, next) => {
    const start = Date.now();
    await next();
    const ms = Date.now() - start;
    process.stdout.write(`${c.req.method} ${c.req.path} ${c.res.status} ${ms}ms\n`);
  });

  app.get("/health", (c) => c.json({ ok: true }));

  // Only the public Worker entry passes `limiter`/`cache`; the Node dev
  // server's routes fall straight through to `next()` on both.
  app.use("/api/v1/*", async (c, next) => {
    if (!limiter) return next();
    const key = c.req.header("CF-Connecting-IP") ?? "unknown";
    const { success } = await limiter.limit({ key });
    if (!success) {
      return c.json({ error: "Too many requests" }, 429, {
        "Retry-After": String(RATE_LIMIT_WINDOW_SECONDS),
      });
    }
    return next();
  });

  app.use("/api/v1/*", async (c, next) => {
    if (!cache) return next();
    // The cache key is the full request URL - GET-only, so a plain `Request`
    // built from it is enough; no method/body/headers to vary on.
    const cacheKey = new Request(c.req.url);
    const cached = await cache.match(cacheKey);
    if (cached) {
      c.res = cached.clone();
      return;
    }
    await next();
    if (c.res.status === 200) {
      const ttl = cacheTtlSecondsFor(c.req.path);
      const cacheable = new Response(c.res.body, c.res);
      cacheable.headers.set("Cache-Control", `public, max-age=${ttl}`);
      await cache.put(cacheKey, cacheable);
    }
  });

  app.get("/api/v1/skills", async (c) => {
    const { status, body } = await proxyGet(
      apiKey,
      "/skills",
      new URL(c.req.url).search,
      fetchImpl,
    );
    // SAFETY: `status` is skills.sh's own response status, always a valid
    // HTTP status code - Hono's `ContentfulStatusCode` union just doesn't
    // widen back to `number`.
    return c.json(body, status as ContentfulStatusCode);
  });

  app.get("/api/v1/skills/search", async (c) => {
    const { status, body } = await proxyGet(
      apiKey,
      "/skills/search",
      new URL(c.req.url).search,
      fetchImpl,
    );
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
      fetchImpl,
    );
    // SAFETY: see the /api/v1/skills handler above.
    return c.json(body, status as ContentfulStatusCode);
  });

  app.get("/api/v1/*", (c) => c.json({ error: "Invalid skill detail path" }, 400));

  return app;
}
