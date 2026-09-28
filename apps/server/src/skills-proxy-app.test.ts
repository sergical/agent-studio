// ============================================================================
// Skill Studio - Skills Proxy App tests
// Covers the runtime-neutral app factory: proxy path/query building, unsafe
// path rejection, and the rate-limit/cache middleware the public Worker
// entry injects - all with plain fakes, no Miniflare and no network.
// ============================================================================

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createSkillsProxyApp,
  proxyGet,
  upstreamUrl,
  type RateLimiter,
  type ResponseCache,
} from "./skills-proxy-app";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("upstreamUrl", () => {
  it("appends the path and search string verbatim to the skills.sh base", () => {
    expect(upstreamUrl("/skills/search", "?q=foo&limit=10")).toBe(
      "https://skills.sh/api/v1/skills/search?q=foo&limit=10",
    );
  });

  it("tolerates an empty search string", () => {
    expect(upstreamUrl("/skills", "")).toBe("https://skills.sh/api/v1/skills");
  });
});

describe("proxyGet", () => {
  it("sends the bearer token and relays the upstream's status and body", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ data: [] }), { status: 200 }));

    const result = await proxyGet("sk-test", "/skills", "?page=0", fetchMock);

    expect(fetchMock).toHaveBeenCalledWith("https://skills.sh/api/v1/skills?page=0", {
      headers: { Authorization: "Bearer sk-test" },
    });
    expect(result).toEqual({ status: 200, body: { data: [] } });
  });

  it("relays a non-2xx upstream response as-is", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ error: "unauthorized" }), { status: 401 }));

    const result = await proxyGet("sk-bad", "/skills", "", fetchMock);

    expect(result).toEqual({ status: 401, body: { error: "unauthorized" } });
  });

  it("maps a failed fetch to a synthetic error body instead of throwing", async () => {
    const fetchMock = vi.fn().mockRejectedValue(new Error("getaddrinfo ENOTFOUND skills.sh"));

    const result = await proxyGet("sk-test", "/skills", "", fetchMock);

    expect(result.status).toBe(502);
    expect(result.body).toEqual({ error: "getaddrinfo ENOTFOUND skills.sh" });
  });
});

describe("GET /api/v1/skills/:owner/:repo/:slug", () => {
  it.each([
    "/api/v1/skills/%2E%2E/repo/slug",
    "/api/v1/skills/owner/repo%2Fescape/slug",
    "/api/v1/skills/owner/repo%252Fescape/slug",
    "/api/v1/skills/owner/repo/%2E%2E",
    "/api/v1/skills/owner/repo/%252E%252E",
    "/api/v1/skills/owner/repo/skill%5Cname",
    "/api/v1/skills/owner/repo/skill%255Cname",
    "/api/v1/skills/owner/repo/%E0%A4%A",
  ])("rejects unsafe encoded segments before fetch: %s", async (path) => {
    const fetchMock = vi.fn();

    const response = await createSkillsProxyApp({
      apiKey: "sk-secret",
      fetch: fetchMock,
    }).request(`http://localhost${path}`);

    expect(response.status).toBe(400);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("encodes decoded safe segments once and drops the query, since the detail route takes none", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ skill: "ok" }), { status: 200 }));

    const response = await createSkillsProxyApp({ apiKey: "sk-secret", fetch: fetchMock }).request(
      "http://localhost/api/v1/skills/org%20name/repo%2Btools/skill%25%40v1?ref=a%2Fb",
    );

    expect(response.status).toBe(200);
    expect(fetchMock).toHaveBeenCalledWith(
      "https://skills.sh/api/v1/skills/org%20name/repo%2Btools/skill%25%40v1",
      { headers: { Authorization: "Bearer sk-secret" } },
    );
  });
});

/** An in-memory `RateLimiter` fake: `refuseAfter` requests succeed, then
 * every later `limit()` call for any key refuses - enough to prove the
 * middleware wiring without a real Workers Rate Limiting binding. */
function fakeLimiter(refuseAfter: number): RateLimiter & { calls: number } {
  const state = { calls: 0 };
  return {
    get calls() {
      return state.calls;
    },
    async limit() {
      state.calls += 1;
      return { success: state.calls <= refuseAfter };
    },
  };
}

/** An in-memory `ResponseCache` fake keyed by request URL, matching just
 * enough of the Workers Cache API (`caches.default`) for these tests. The
 * real Cache API's `put` reads the response body to completion - mirrored
 * here (instead of `response.clone()`) so a production bug that hands the
 * live response's body to both the caller and the cache fails the same way
 * it would against a real Worker: with a "body already used" error. */
function fakeCache(): ResponseCache & { size: number } {
  const store = new Map<string, { body: ArrayBuffer; status: number; headers: Headers }>();
  return {
    get size() {
      return store.size;
    },
    async match(request) {
      const cached = store.get(request.url);
      if (!cached) return undefined;
      return new Response(cached.body, { status: cached.status, headers: cached.headers });
    },
    async put(request, response) {
      const body = await response.arrayBuffer();
      store.set(request.url, { body, status: response.status, headers: response.headers });
    },
  };
}

describe("rate limit middleware", () => {
  it("returns 429 with Retry-After when the limiter refuses, without calling upstream", async () => {
    const fetchMock = vi.fn();
    const limiter = fakeLimiter(0);

    const response = await createSkillsProxyApp({
      apiKey: "sk-secret",
      fetch: fetchMock,
      limiter,
    }).request("http://localhost/api/v1/skills");

    expect(response.status).toBe(429);
    expect(response.headers.get("Retry-After")).toBe("60");
    expect(await response.json()).toEqual({ error: "Too many requests" });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("does not rate limit /health, answering even when the limiter would refuse", async () => {
    const limiter = fakeLimiter(0);

    const response = await createSkillsProxyApp({ apiKey: "sk-secret", limiter }).request(
      "http://localhost/health",
    );

    expect(response.status).toBe(200);
    expect(limiter.calls).toBe(0);
  });
});

describe("aggregate upstream budget middleware", () => {
  it("returns 429 with Retry-After when the shared budget is spent, without calling upstream", async () => {
    const fetchMock = vi.fn();
    const budget = fakeLimiter(0);

    const response = await createSkillsProxyApp({
      apiKey: "sk-secret",
      fetch: fetchMock,
      budget,
    }).request("http://localhost/api/v1/skills");

    expect(response.status).toBe(429);
    expect(response.headers.get("Retry-After")).toBe("60");
    expect(await response.json()).toEqual({ error: "Too many requests" });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("spends the budget only on a cache miss, leaving cached reads free", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ data: ["one"] }), { status: 200 }));
    const budget = fakeLimiter(1);
    const cache = fakeCache();
    const app = createSkillsProxyApp({ apiKey: "sk-secret", fetch: fetchMock, budget, cache });

    await app.request("http://localhost/api/v1/skills?page=0");
    const second = await app.request("http://localhost/api/v1/skills?page=0");

    expect(await second.json()).toEqual({ data: ["one"] });
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(budget.calls).toBe(1);
  });

  it("does not cache a 429 from the budget either", async () => {
    const fetchMock = vi.fn();
    const budget = fakeLimiter(0);
    const cache = fakeCache();

    const response = await createSkillsProxyApp({
      apiKey: "sk-secret",
      fetch: fetchMock,
      budget,
      cache,
    }).request("http://localhost/api/v1/skills");

    expect(response.status).toBe(429);
    expect(cache.size).toBe(0);
  });

  it("does not consult the budget for /health, answering even when it would refuse", async () => {
    const budget = fakeLimiter(0);

    const response = await createSkillsProxyApp({ apiKey: "sk-secret", budget }).request(
      "http://localhost/health",
    );

    expect(response.status).toBe(200);
    expect(budget.calls).toBe(0);
  });

  it("leaves the Node dev server unbudgeted when no budget is injected", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ data: [] }), { status: 200 }));
    const app = createSkillsProxyApp({ apiKey: "sk-secret", fetch: fetchMock });

    for (let i = 0; i < 3; i += 1) {
      const response = await app.request(`http://localhost/api/v1/skills?page=${i}`);
      expect(response.status).toBe(200);
    }
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });
});

describe("edge cache middleware", () => {
  it("serves a second identical request from the cache with no second upstream call", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ data: ["one"] }), { status: 200 }));
    const cache = fakeCache();
    const app = createSkillsProxyApp({ apiKey: "sk-secret", fetch: fetchMock, cache });

    const first = await app.request("http://localhost/api/v1/skills?view=all-time&page=0");
    const second = await app.request("http://localhost/api/v1/skills?view=all-time&page=0");

    expect(first.status).toBe(200);
    expect(await first.json()).toEqual({ data: ["one"] });
    expect(await second.json()).toEqual({ data: ["one"] });
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("passes the rate limit check before ever consulting the cache", async () => {
    const fetchMock = vi.fn();
    const limiter = fakeLimiter(0);
    const cache = fakeCache();

    const response = await createSkillsProxyApp({
      apiKey: "sk-secret",
      fetch: fetchMock,
      limiter,
      cache,
    }).request("http://localhost/api/v1/skills");

    expect(response.status).toBe(429);
    expect(cache.size).toBe(0);
  });

  it("relays an upstream 404 without caching it", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ error: "not found" }), { status: 404 }));
    const cache = fakeCache();
    const app = createSkillsProxyApp({ apiKey: "sk-secret", fetch: fetchMock, cache });

    const response = await app.request("http://localhost/api/v1/skills/owner/repo/missing");

    expect(response.status).toBe(404);
    expect(cache.size).toBe(0);
    await app.request("http://localhost/api/v1/skills/owner/repo/missing");
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("relays an upstream 500 without caching it", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ error: "boom" }), { status: 500 }));
    const cache = fakeCache();
    const app = createSkillsProxyApp({ apiKey: "sk-secret", fetch: fetchMock, cache });

    const response = await app.request("http://localhost/api/v1/skills");

    expect(response.status).toBe(500);
    expect(cache.size).toBe(0);
  });

  it("drops a query param outside the route's allowlist before it reaches upstream", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ data: [] }), { status: 200 }));

    await createSkillsProxyApp({ apiKey: "sk-secret", fetch: fetchMock }).request(
      "http://localhost/api/v1/skills?page=0&x=cache-buster",
    );

    expect(fetchMock).toHaveBeenCalledWith(
      "https://skills.sh/api/v1/skills?page=0",
      expect.anything(),
    );
  });

  it("treats an unrelated extra param as the same cache entry, making one upstream call", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ data: ["one"] }), { status: 200 }));
    const cache = fakeCache();
    const app = createSkillsProxyApp({ apiKey: "sk-secret", fetch: fetchMock, cache });

    await app.request("http://localhost/api/v1/skills?page=0");
    await app.request("http://localhost/api/v1/skills?page=0&x=cache-buster-1");
    await app.request("http://localhost/api/v1/skills?page=0&x=cache-buster-2");

    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("treats the same params in a different order as the same cache entry, making one upstream call", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ data: ["one"] }), { status: 200 }));
    const cache = fakeCache();
    const app = createSkillsProxyApp({ apiKey: "sk-secret", fetch: fetchMock, cache });

    await app.request("http://localhost/api/v1/skills?view=all-time&page=0&per_page=20");
    await app.request("http://localhost/api/v1/skills?per_page=20&page=0&view=all-time");

    expect(fetchMock).toHaveBeenCalledOnce();
  });
});

describe("non-GET requests to /api/v1/*", () => {
  it("rejects a write method with 405, without consulting the cache or calling upstream", async () => {
    const fetchMock = vi.fn();
    const cache = fakeCache();
    const app = createSkillsProxyApp({ apiKey: "sk-secret", fetch: fetchMock, cache });
    await app.request("http://localhost/api/v1/skills?page=0");

    const response = await app.request("http://localhost/api/v1/skills?page=99", {
      method: "POST",
    });

    expect(response.status).toBe(405);
    expect(fetchMock).toHaveBeenCalledOnce();
  });
});
