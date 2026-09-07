// ============================================================================
// Skill Studio - Server tests
// Covers the proxy path/query building and the never-log-the-key guard, with
// a stubbed `fetch` - no network involved.
// ============================================================================

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createApp,
  createNodeRequestHandler,
  isAllowedRawRequestTarget,
  proxyGet,
  requireApiKey,
  upstreamUrl,
} from "./server";

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
    vi.stubGlobal("fetch", fetchMock);

    const result = await proxyGet("sk-test", "/skills", "?page=0");

    expect(fetchMock).toHaveBeenCalledWith("https://skills.sh/api/v1/skills?page=0", {
      headers: { Authorization: "Bearer sk-test" },
    });
    expect(result).toEqual({ status: 200, body: { data: [] } });
  });

  it("relays a non-2xx upstream response as-is", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ error: "unauthorized" }), { status: 401 }));
    vi.stubGlobal("fetch", fetchMock);

    const result = await proxyGet("sk-bad", "/skills", "");

    expect(result).toEqual({ status: 401, body: { error: "unauthorized" } });
  });

  it("maps a failed fetch to a synthetic error body instead of throwing", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("getaddrinfo ENOTFOUND skills.sh")));

    const result = await proxyGet("sk-test", "/skills", "");

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
    vi.stubGlobal("fetch", fetchMock);

    const response = await createApp("sk-secret").request(`http://localhost${path}`);

    expect(response.status).toBe(400);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("encodes decoded safe segments once and forwards the query", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ skill: "ok" }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    const response = await createApp("sk-secret").request(
      "http://localhost/api/v1/skills/org%20name/repo%2Btools/skill%25%40v1?ref=a%2Fb",
    );

    expect(response.status).toBe(200);
    expect(fetchMock).toHaveBeenCalledWith(
      "https://skills.sh/api/v1/skills/org%20name/repo%2Btools/skill%25%40v1?ref=a%2Fb",
      { headers: { Authorization: "Bearer sk-secret" } },
    );
  });
});

describe("raw Node request target validation", () => {
  it.each([
    "/api/v1/skills/x/../search",
    "/api/v1/skills/x/%2e%2e/search",
    "/api/v1/skills/x/%252e%252e/search",
    "/api/v1/skills/owner/repo%2Fescape/slug",
    "/api/v1/skills/owner/repo%252Fescape/slug",
    "/api/v1/skills/owner/repo/skill%5Cname",
    "/api/v1/skills/owner/repo/skill%255Cname",
    "/api/v1/skills/owner/repo/%E0%A4%A",
    "/api/v1/skills/search?q=%E0%A4%A",
  ])("rejects an unsafe raw path: %s", (rawTarget) => {
    expect(isAllowedRawRequestTarget(rawTarget)).toBe(false);
  });

  it.each([
    ["/api/v1/skills/x/../search", "/api/v1/skills/search"],
    ["/api/v1/skills/x/%2e%2e/search", "/api/v1/skills/search"],
    ["/api/v1/skills/x/%252e%252e/search", "/api/v1/skills/search"],
    ["/api/v1/skills/search/%2e%2e", "/api/v1/skills"],
  ])(
    "rejects %s even when the Fetch Request has normalized to %s",
    async (rawTarget, normalizedPath) => {
      const fetchMock = vi.fn();
      vi.stubGlobal("fetch", fetchMock);
      const handler = createNodeRequestHandler("sk-secret");
      const normalizedRequest = new Request(`http://localhost${normalizedPath}`);
      expect(new URL(normalizedRequest.url).pathname).toBe(normalizedPath);

      const response = await handler(normalizedRequest, { incoming: { url: rawTarget } });

      expect(response.status).toBe(400);
      expect(await response.text()).not.toContain("sk-secret");
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  it.each([
    "/api/v1/skills/owner/repo%2Fescape/slug",
    "/api/v1/skills/owner/repo%252Fescape/slug",
    "/api/v1/skills/owner/repo/skill%5Cname",
    "/api/v1/skills/owner/repo/skill%255Cname",
    "/api/v1/skills/owner/repo/%E0%A4%A",
    "/api/v1/skills/search?q=%E0%A4%A",
  ])("rejects %s at the production seam before bearer fetch", async (rawTarget) => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const handler = createNodeRequestHandler("sk-secret");
    const response = await handler(new Request("http://localhost/api/v1/skills"), {
      incoming: { url: rawTarget },
    });

    expect(response.status).toBe(400);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each([
    "/api/v1/skills?view=all-time&page=0",
    "/api/v1/skills/search?q=a%2Fb&limit=10",
    "/api/v1/skills/org%20name/repo%2Btools/skill%25%40v1?ref=a%2Fb",
  ])("allows an exact supported route: %s", async (rawTarget) => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ data: [] }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    const handler = createNodeRequestHandler("sk-secret");

    const response = await handler(new Request(`http://localhost${rawTarget}`), {
      incoming: { url: rawTarget },
    });

    expect(response.status).toBe(200);
    expect(fetchMock).toHaveBeenCalledOnce();
  });
});

describe("requireApiKey", () => {
  it("returns the trimmed key when present", () => {
    expect(requireApiKey({ SKILLS_SH_API_KEY: "  sk-test  " })).toBe("sk-test");
  });

  it("throws a clear message, and never the key itself, when missing", () => {
    expect(() => requireApiKey({})).toThrow(/SKILLS_SH_API_KEY is not set/);
  });

  it("throws for a blank key", () => {
    expect(() => requireApiKey({ SKILLS_SH_API_KEY: "   " })).toThrow(
      /SKILLS_SH_API_KEY is not set/,
    );
  });
});
