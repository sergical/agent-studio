// ============================================================================
// Skill Studio - Server
// The Node entry point (`npm run dev:server`) for the skills.sh proxy: builds
// `createSkillsProxyApp` with no rate limiter and no cache (local dev has no
// public abuse surface to guard against) and serves it with
// `@hono/node-server`. See `src/worker.ts` for the public Cloudflare Workers
// entry, and `src/skills-proxy-app.ts` for the shared app both build.
// ============================================================================

import { serve } from "@hono/node-server";

import { createSkillsProxyApp, isSafeRawPathSegment } from "./skills-proxy-app";

const DEFAULT_PORT = 8787;
const HOST = "127.0.0.1";

interface RawNodeRequestEnvironment {
  incoming: {
    url?: string;
  };
}

/** Reads and validates `SKILLS_SH_API_KEY` - the one thing this server
 * refuses to start without. Never logged. */
export function requireApiKey(env: NodeJS.ProcessEnv): string {
  const key = env.SKILLS_SH_API_KEY?.trim();
  if (!key) {
    throw new Error(
      "SKILLS_SH_API_KEY is not set. Add it to the repo-root .env, then run `npm run dev:server`.",
    );
  }
  return key;
}

/** Validates the unnormalized Node request target before Hono can route a normalized URL. */
export function isAllowedRawRequestTarget(rawTarget: string | undefined): boolean {
  if (!rawTarget) return false;

  const queryStart = rawTarget.indexOf("?");
  const rawPath = queryStart === -1 ? rawTarget : rawTarget.slice(0, queryStart);
  const rawQuery = queryStart === -1 ? "" : rawTarget.slice(queryStart + 1);
  try {
    decodeURIComponent(rawQuery);
  } catch {
    return false;
  }
  const segments = rawPath.split("/");
  if (segments[0] !== "" || segments.slice(1).some((segment) => !isSafeRawPathSegment(segment))) {
    return false;
  }

  const isApiV1 = segments[1] === "api" && segments[2] === "v1";
  if (!isApiV1) return true;

  const isSkillsRoute = segments[3] === "skills";
  const isListRoute = isSkillsRoute && segments.length === 4;
  const isSearchRoute = isSkillsRoute && segments.length === 5 && segments[4] === "search";
  const isDetailRoute = isSkillsRoute && segments.length === 7;
  return isListRoute || isSearchRoute || isDetailRoute;
}

/** Builds the Hono app for `apiKey`, with no rate limiter and no cache - kept
 * for tests that exercise routes directly without a real listener. */
export function createApp(apiKey: string) {
  return createSkillsProxyApp({ apiKey });
}

/** Creates the production Node fetch seam that rejects unsafe raw targets before Hono routing. */
export function createNodeRequestHandler(apiKey: string) {
  const app = createApp(apiKey);
  return (request: Request, env: RawNodeRequestEnvironment): Response | Promise<Response> => {
    if (!isAllowedRawRequestTarget(env.incoming.url)) {
      return Response.json({ error: "Invalid request path" }, { status: 400 });
    }
    return app.fetch(request, env);
  };
}

function main() {
  const apiKey = requireApiKey(process.env);
  const port = Number(process.env.PORT) || DEFAULT_PORT;
  serve({ fetch: createNodeRequestHandler(apiKey), port, hostname: HOST }, (info) => {
    process.stdout.write(`Skill Studio server listening on http://${HOST}:${info.port}\n`);
  });
}

// Only start the server when this file is run directly (`tsx src/server.ts`),
// not when a test imports its exports.
if (import.meta.url === `file://${process.argv[1]}`) {
  main();
}
