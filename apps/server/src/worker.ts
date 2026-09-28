// ============================================================================
// Skill Studio - Worker
// The Cloudflare Workers entry point for the skills.sh proxy: this URL is
// public (unlike the Node dev server, which only ever binds 127.0.0.1), so it
// wires the Workers Rate Limiting binding and the Cache API into
// `createSkillsProxyApp` before every request reaches skills.sh.
// ============================================================================

import { createSkillsProxyApp, type RateLimiter, type ResponseCache } from "./skills-proxy-app";

/** The subset of Workers' `Fetcher.env` this proxy reads: the skills.sh key
 * (set once with `wrangler secret put SKILLS_SH_API_KEY`) and the rate limit
 * binding declared in `wrangler.jsonc`. */
interface Env {
  SKILLS_SH_API_KEY: string;
  RATE_LIMITER: RateLimiter;
  /** The deployed commit, injected by `.github/workflows/deploy-server.yml`
   * with `wrangler deploy --var SKILL_STUDIO_SERVER_VERSION:$GITHUB_SHA`. */
  SKILL_STUDIO_SERVER_VERSION?: string;
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

export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Response | Promise<Response> {
    const app = createSkillsProxyApp({
      apiKey: env.SKILLS_SH_API_KEY,
      limiter: env.RATE_LIMITER,
      cache: caches.default,
      waitUntil: (promise) => ctx.waitUntil(promise),
      version: env.SKILL_STUDIO_SERVER_VERSION,
    });
    return app.fetch(request);
  },
};
