// ============================================================================
// Skill Studio - Worker
// The Cloudflare Workers entry point for the skills.sh proxy: this URL is
// public (unlike the Node dev server, which only ever binds 127.0.0.1), so it
// wires the Workers Rate Limiting binding and the Cache API into
// `createSkillsProxyApp` before every request reaches skills.sh.
// ============================================================================

import * as Sentry from "@sentry/cloudflare";

import { createSkillsProxyApp, type RateLimiter, type ResponseCache } from "./skills-proxy-app";
import { scrubSentryEvent } from "./sentry-event-scrub";

import type { CloudflareOptions } from "@sentry/cloudflare";

/** The subset of Workers' `Fetcher.env` this proxy reads: the skills.sh key
 * (set once with `wrangler secret put SKILLS_SH_API_KEY`), the rate limit
 * binding declared in `wrangler.jsonc`, and the optional Sentry DSN (`wrangler
 * secret put SENTRY_DSN`). */
export interface Env {
  SKILLS_SH_API_KEY: string;
  RATE_LIMITER: RateLimiter;
  SENTRY_DSN?: string;
}

/** The Workers fetch handler's third argument - its `waitUntil` schedules the
 * Cache API write past the response, so a cache write never adds to the
 * caller's latency. Named narrowly instead of pulling in
 * `@cloudflare/workers-types` for one method. */
interface ExecutionContext {
  waitUntil(promise: Promise<unknown>): void;
  passThroughOnException(): void;
}

// `caches.default` is a Workers-only global (the edge Cache API) with no
// Node equivalent, so it isn't part of this project's `lib: ["ES2020"]`
// tsconfig - declared narrowly here instead of pulling in the full
// `@cloudflare/workers-types` package just for one global.
declare const caches: { default: ResponseCache };

const handler = {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Response | Promise<Response> {
    const app = createSkillsProxyApp({
      apiKey: env.SKILLS_SH_API_KEY,
      limiter: env.RATE_LIMITER,
      cache: caches.default,
      waitUntil: (promise) => ctx.waitUntil(promise),
      reportServerError: (error, { kind }) =>
        Sentry.captureException(error, { tags: { error_kind: kind } }),
    });
    return app.fetch(request);
  },
};

/** Without `SENTRY_DSN` the SDK still initializes on every request but has no
 * transport, so it captures locally and sends nothing.
 *
 * Tracing stays off on purpose: no `tracesSampleRate` or `tracesSampler` is
 * set below. A caller can send a sampled `sentry-trace` header on any
 * request, and that alone forces a transaction regardless of this Worker's
 * own sampling config - transactions skip `beforeSend`, so a transaction is
 * the one event type this scrub can't clean. `tracePropagationTargets: []`
 * closes the other half: it stops the SDK from attaching `sentry-trace`/
 * `baggage` headers to the outgoing skills.sh requests. */
export function sentryOptionsFor(env: Env): CloudflareOptions {
  return {
    dsn: env.SENTRY_DSN,
    environment: "production",
    beforeSend: scrubSentryEvent,
    beforeSendTransaction: () => null,
    tracePropagationTargets: [],
  };
}

export default Sentry.withSentry(sentryOptionsFor, handler);
