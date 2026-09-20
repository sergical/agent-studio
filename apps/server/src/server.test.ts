// ============================================================================
// Skill Studio - Server tests
// Covers the Node entry's own concerns: the API key guard and the raw
// request target validation that runs before Hono's tolerant URL
// normalization - see skills-proxy-app.test.ts for the shared app's routes.
// ============================================================================

import { afterEach, describe, expect, it, vi } from "vitest";
import { createNodeRequestHandler, isAllowedRawRequestTarget, requireApiKey } from "./server";

afterEach(() => {
  vi.unstubAllGlobals();
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
