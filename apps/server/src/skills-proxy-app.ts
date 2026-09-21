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

/** Why a proxy result carried a body the proxy wrote itself rather than one
 * skills.sh produced: the network call to skills.sh failed, or skills.sh
 * replied with something that isn't JSON. `null` when skills.sh itself
 * answered, so the request log can tell the proxy's own synthetic 502 apart
 * from a 502 skills.sh really sent. */
type ProxyFailure = "upstream_unreachable" | "upstream_non_json";

/** One proxied GET's outcome: the upstream's own status and JSON body when
 * it responded at all (any status, not just 2xx), or a synthetic `{ error }`
 * body when the request to skills.sh itself couldn't be made. `failure`
 * names which synthetic case it was, for the caller's request log. */
interface ProxyResult {
  status: number;
  body: unknown;
  failure: ProxyFailure | null;
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
      failure: "upstream_unreachable",
    };
  }
  try {
    const body = await response.json();
    return { status: response.status, body, failure: null };
  } catch {
    return {
      status: response.status,
      body: { error: "skills.sh returned a non-JSON response" },
      failure: "upstream_non_json",
    };
  }
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
  /** Schedules work past the response, e.g. Workers' `ExecutionContext.waitUntil` -
   * when absent, the cache write is awaited inline instead. */
  waitUntil?: (promise: Promise<unknown>) => void;
}

const RATE_LIMIT_WINDOW_SECONDS = 60;

/** Edge-cache lifetime per route family, in seconds - list/search results
 * churn faster than a single skill's detail page. */
function cacheTtlSecondsFor(path: string): number {
  return path === "/api/v1/skills" || path === "/api/v1/skills/search" ? 300 : 3600;
}

/** The only query params each route forwards upstream and keys the cache on,
 * sorted for a stable order - anything else (an unrelated param, or the same
 * params in a different order) is dropped so it can't fragment the cache or
 * drain a caller's rate-limit quota with cache-busting variations. The skill
 * detail route takes no query params at all. */
const ALLOWED_QUERY_PARAMS = {
  "/api/v1/skills": ["page", "per_page", "view"],
  "/api/v1/skills/search": ["limit", "q"],
} satisfies Record<string, readonly string[]>;

/** Rebuilds `url`'s query string using only `path`'s allowed params, in
 * sorted order - used for both the upstream request and the cache key so the
 * two always agree. */
function normalizedSearch(path: string, url: string): string {
  const allowed = ALLOWED_QUERY_PARAMS[path] ?? [];
  const params = new URL(url).searchParams;
  const kept = new URLSearchParams();
  for (const key of allowed) {
    const value = params.get(key);
    if (value !== null) kept.set(key, value);
  }
  const search = kept.toString();
  return search ? `?${search}` : "";
}

/** Per-request state the completion log reads back off Hono's context. Each
 * field is set by the middleware or handler that learns it, so one finished
 * request can carry the cache outcome, the rate-limit verdict, and what
 * skills.sh answered - none of which the request line alone exposed. */
interface ProxyLogVariables {
  cacheOutcome: "hit" | "miss" | "bypass";
  rateLimited: boolean;
  upstreamStatus: number | null;
  upstreamFailure: ProxyFailure | null;
}

/** The one event emitted per request, once its response is decided. Field
 * names are stable so Workers Logs and `wrangler tail` can query them
 * (e.g. `status = 502 AND upstream_failure = "upstream_unreachable"`), and the
 * Node dev server's stdout shows the same shape. */
export interface RequestLogEvent {
  event: "http_request";
  request_id: string;
  method: string;
  path: string;
  status: number;
  duration_ms: number;
  cache: "hit" | "miss" | "bypass";
  rate_limited: boolean;
  upstream_status: number | null;
  upstream_failure: ProxyFailure | null;
  error: string | null;
}

/** Writes one JSON line to stdout - the runtime-neutral channel this app
 * already logged to, which both the Node dev server and Workers capture. */
function emitRequestLog(event: RequestLogEvent): void {
  process.stdout.write(`${JSON.stringify(event)}\n`);
}

/** Builds the Hono app for `apiKey` - split out from each runtime's entry so
 * tests can exercise routes without starting a real listener, and so the
 * Node and Worker entries share one implementation. */
export function createSkillsProxyApp({
  apiKey,
  fetch: fetchImpl = fetch,
  limiter,
  cache,
  waitUntil,
}: CreateSkillsProxyAppOptions): Hono<{ Variables: ProxyLogVariables }> {
  const app = new Hono<{ Variables: ProxyLogVariables }>();

  // Outermost middleware: seeds the per-request log fields, then emits exactly
  // one event once the response is decided. Alongside method/path/status/
  // duration it carries a request id and the fields the old request line never
  // exposed - cache outcome, the rate-limit verdict, which status skills.sh
  // answered with, and the message of any handler error.
  app.use("*", async (c, next) => {
    const requestId = crypto.randomUUID();
    c.set("cacheOutcome", "bypass");
    c.set("rateLimited", false);
    c.set("upstreamStatus", null);
    c.set("upstreamFailure", null);
    const start = Date.now();

    let status = 500;
    let error: string | null = null;
    try {
      await next();
      status = c.res.status;
      // Hono's own error handling turns a thrown handler/middleware error into
      // a 500 response (and records the cause on `c.error`) before `next()`
      // resolves, so this is the only place the crash's message is still
      // reachable - it never surfaces as a rejection here.
      error = c.error?.message ?? null;
    } catch (e) {
      // Only a non-`Error` throw escapes Hono's handler; keep the request
      // logged before it propagates.
      error = e instanceof Error ? e.message : "unhandled error";
      throw e;
    } finally {
      emitRequestLog({
        event: "http_request",
        request_id: requestId,
        method: c.req.method,
        path: c.req.path,
        status,
        duration_ms: Date.now() - start,
        cache: c.get("cacheOutcome"),
        rate_limited: c.get("rateLimited"),
        upstream_status: c.get("upstreamStatus"),
        upstream_failure: c.get("upstreamFailure"),
        error,
      });
    }
  });

  app.get("/health", (c) => c.json({ ok: true }));

  // The rate limiter and cache both key on GET-only semantics (an idempotent,
  // side-effect-free request whose URL fully determines the response), so a
  // non-GET method is rejected here, before either middleware runs, rather
  // than falling through to them and to Hono's routing.
  app.use("/api/v1/*", async (c, next) => {
    if (c.req.method !== "GET") {
      return c.json({ error: "Method not allowed" }, 405);
    }
    return next();
  });

  // Only the public Worker entry passes `limiter`/`cache`; the Node dev
  // server's routes fall straight through to `next()` on both.
  app.use("/api/v1/*", async (c, next) => {
    if (!limiter) return next();
    const key = c.req.header("CF-Connecting-IP") ?? "unknown";
    const { success } = await limiter.limit({ key });
    if (!success) {
      c.set("rateLimited", true);
      return c.json({ error: "Too many requests" }, 429, {
        "Retry-After": String(RATE_LIMIT_WINDOW_SECONDS),
      });
    }
    return next();
  });

  app.use("/api/v1/*", async (c, next) => {
    if (!cache) return next();
    // The cache key is the request's origin/path plus its normalized query -
    // GET-only, so a plain `Request` built from it is enough; no
    // method/body/headers to vary on.
    const normalized = normalizedSearch(c.req.path, c.req.url);
    const cacheKey = new Request(`${new URL(c.req.url).origin}${c.req.path}${normalized}`);
    const cached = await cache.match(cacheKey);
    if (cached) {
      c.set("cacheOutcome", "hit");
      c.res = cached.clone();
      return;
    }
    c.set("cacheOutcome", "miss");
    await next();
    if (c.res.status === 200) {
      const ttl = cacheTtlSecondsFor(c.req.path);
      // `c.res.clone()` tees the body so the cache and the eventual caller
      // each get their own independent stream - handing both the same
      // stream (e.g. `new Response(c.res.body, c.res)`) means whichever
      // reads first (here, `cache.put`) leaves the other's body consumed.
      const cacheable = c.res.clone();
      cacheable.headers.set("Cache-Control", `public, max-age=${ttl}`);
      const putPromise = cache.put(cacheKey, cacheable);
      if (waitUntil) {
        waitUntil(putPromise);
      } else {
        await putPromise;
      }
    }
  });

  app.get("/api/v1/skills", async (c) => {
    const { status, body, failure } = await proxyGet(
      apiKey,
      "/skills",
      normalizedSearch(c.req.path, c.req.url),
      fetchImpl,
    );
    // `upstream_status` stays null when there was no upstream response at
    // all (skills.sh unreachable); otherwise it is skills.sh's own status,
    // whatever the proxy relays to the caller.
    c.set("upstreamStatus", failure === "upstream_unreachable" ? null : status);
    c.set("upstreamFailure", failure);
    // SAFETY: `status` is skills.sh's own response status, always a valid
    // HTTP status code - Hono's `ContentfulStatusCode` union just doesn't
    // widen back to `number`.
    return c.json(body, status as ContentfulStatusCode);
  });

  app.get("/api/v1/skills/search", async (c) => {
    const { status, body, failure } = await proxyGet(
      apiKey,
      "/skills/search",
      normalizedSearch(c.req.path, c.req.url),
      fetchImpl,
    );
    // See the /api/v1/skills handler for the `upstream_*` fields.
    c.set("upstreamStatus", failure === "upstream_unreachable" ? null : status);
    c.set("upstreamFailure", failure);
    // SAFETY: see the /api/v1/skills handler above.
    return c.json(body, status as ContentfulStatusCode);
  });

  app.get("/api/v1/skills/:owner/:repo/:slug", async (c) => {
    const { owner, repo, slug } = c.req.param();
    const segments = [owner, repo, slug];
    if (hasMalformedSkillDetailEncoding(c.req.url) || !segments.every(decodesToSafePathSegment)) {
      return c.json({ error: "Invalid skill detail path" }, 400);
    }
    const { status, body, failure } = await proxyGet(
      apiKey,
      `/skills/${segments.map((segment) => encodeURIComponent(segment)).join("/")}`,
      // The detail route takes no query params - not just for the cache key,
      // but forwarded to skills.sh too.
      normalizedSearch(c.req.path, c.req.url),
      fetchImpl,
    );
    // See the /api/v1/skills handler for the `upstream_*` fields.
    c.set("upstreamStatus", failure === "upstream_unreachable" ? null : status);
    c.set("upstreamFailure", failure);
    // SAFETY: see the /api/v1/skills handler above.
    return c.json(body, status as ContentfulStatusCode);
  });

  app.get("/api/v1/*", (c) => c.json({ error: "Invalid skill detail path" }, 400));

  return app;
}
