// ============================================================================
// Skill Studio - Server tests
// Covers the proxy path/query building and the never-log-the-key guard, with
// a stubbed `fetch` - no network involved.
// ============================================================================

import { afterEach, describe, expect, it, vi } from "vitest";
import { proxyGet, requireApiKey, upstreamUrl } from "./server";

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
  afterEach(() => {
    vi.unstubAllGlobals();
  });

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
