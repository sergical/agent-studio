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

/** The subset of Workers' `Fetcher.env` this proxy reads: the skills.sh key
 * (set once with `wrangler secret put SKILLS_SH_API_KEY`), the rate limit
 * binding declared in `wrangler.jsonc`, and the optional Sentry DSN (`wrangler
 * secret put SENTRY_DSN`) - absent, `withSentry` is a no-op. */
interface Env {
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

// `SENTRY_DSN` is an optional Worker secret (`wrangler secret put SENTRY_DSN`)
// - absent, the SDK never initializes and every call below is a no-op.
export default Sentry.withSentry(
  (env: Env) => ({
    dsn: env.SENTRY_DSN,
    environment: "production",
    tracesSampleRate: 0,
    beforeSend: scrubSentryEvent,
  }),
  handler,
);
