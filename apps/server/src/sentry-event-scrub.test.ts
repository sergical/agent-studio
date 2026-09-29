// ============================================================================
// Skill Studio - Sentry Event Scrub tests
// Covers the Worker's Sentry `beforeSend` hook: it must never let a caller's
// IP, headers, cookies, query string, or user data leave the Worker.
// ============================================================================

import { describe, expect, it } from "vitest";

import { scrubSentryEvent } from "./sentry-event-scrub";

import type { ErrorEvent } from "@sentry/cloudflare";

describe("scrubSentryEvent", () => {
  it("a scrubbed event keeps no header, cookie, query string, or user", () => {
    // SAFETY: only the fields `scrubSentryEvent` reads and deletes matter to this test;
    // the full `ErrorEvent` shape adds nothing here.
    const event = {
      type: undefined,
      user: { id: "caller-1" },
      request: {
        method: "GET",
        url: "https://api.example/skills/search?q=my%20secret",
        headers: { "cf-connecting-ip": "203.0.113.7" },
        cookies: { session: "abc" },
        query_string: "q=my%20secret",
        data: { some: "body" },
      },
    } as ErrorEvent;

    const scrubbed = scrubSentryEvent(event);

    expect(scrubbed.user).toBeUndefined();
    expect(scrubbed.request?.headers).toBeUndefined();
    expect(scrubbed.request?.cookies).toBeUndefined();
    expect(scrubbed.request?.query_string).toBeUndefined();
    expect(scrubbed.request?.data).toBeUndefined();
    expect(scrubbed.request?.url).toBe("https://api.example/skills/search");
    expect(scrubbed.request?.method).toBe("GET");
  });

  it("an event without a request passes through unchanged apart from user", () => {
    // SAFETY: see the previous test - only `user` and the absent `request` matter here.
    const event = { type: undefined, user: { id: "caller-1" } } as ErrorEvent;

    const scrubbed = scrubSentryEvent(event);

    expect(scrubbed.user).toBeUndefined();
    expect(scrubbed.request).toBeUndefined();
  });

  it("a fetch breadcrumb carrying a query string is removed from the event", () => {
    // SAFETY: only `breadcrumbs` matters to this test.
    const event = {
      type: undefined,
      breadcrumbs: [
        { category: "fetch", data: { url: "https://skills.sh/api/v1/skills/search?q=secret" } },
      ],
    } as ErrorEvent;

    const scrubbed = scrubSentryEvent(event);

    expect(scrubbed.breadcrumbs).toBeUndefined();
  });

  it("the culture context (timezone) is removed while other contexts stay", () => {
    // SAFETY: only `contexts` matters to this test.
    const event = {
      type: undefined,
      contexts: { culture: { timezone: "America/New_York" }, runtime: { name: "workerd" } },
    } as ErrorEvent;

    const scrubbed = scrubSentryEvent(event);

    expect(scrubbed.contexts?.culture).toBeUndefined();
    expect(scrubbed.contexts?.runtime).toEqual({ name: "workerd" });
  });

  it("an exception message loses its query string but keeps the rest of the text", () => {
    // SAFETY: only `exception.values` matters to this test.
    const event = {
      type: undefined,
      exception: {
        values: [{ value: "fetch failed for https://skills.sh/api/v1/skills/search?q=secret" }],
      },
    } as ErrorEvent;

    const scrubbed = scrubSentryEvent(event);

    expect(scrubbed.exception?.values?.[0]?.value).toBe(
      "fetch failed for https://skills.sh/api/v1/skills/search",
    );
  });
});
