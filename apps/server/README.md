# @skill-studio/server

Proxies skills.sh's authenticated `/api/v1` surface for the desktop app,
since skills.sh keys aren't per-account and the app can't ship one.

## Run

```bash
npm run dev:server   # from the repo root
```

The key lives in the repo-root `.env` as `SKILLS_SH_API_KEY` (not committed).
The server refuses to start without it. `PORT` defaults to `8787`, bound to
`127.0.0.1` only.

## Routes

- `GET /health` -> `{ ok: true }`, no upstream call
- `GET /api/v1/skills`, `GET /api/v1/skills/search`, `GET /api/v1/skills/:owner/:repo/:slug`
  -> proxied to `https://skills.sh/api/v1`, query string passed through verbatim

## Deploy to Cloudflare Workers

The same app also runs as a public Cloudflare Worker (`src/worker.ts`), for
release builds of the desktop app that don't have a local server to talk to
(see `SKILL_STUDIO_SERVER_URL` in the root `apps/desktop` release build). It's
hosted at `https://api.useskillstudio.com`.

The shortest path, from `apps/server`:

```bash
npx wrangler login
npx wrangler secret put SKILLS_SH_API_KEY   # paste the real skills.sh key when prompted
npm run deploy -w @skill-studio/server
```

The optional second path is `.github/workflows/deploy-server.yml`, which
redeploys on demand (`workflow_dispatch`) using `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID` repo secrets - it never sees the skills.sh key.

The repo variable `SKILL_STUDIO_SERVER_URL` must be `https://api.useskillstudio.com`
for release builds. Set it only after the first deploy answers on `/health`.

### Abuse control

The Worker is public, so it adds two things the Node dev server doesn't need:

- **Rate limit**: 60 requests per 60 seconds per caller IP
  (`CF-Connecting-IP`), via the Workers Rate Limiting binding. A refused
  request gets `429` with `Retry-After: 60`.
- **Edge cache**: successful (`200`) responses only, keyed by the full
  request URL - 300 seconds for the list/search routes, 3600 seconds for a
  skill's detail route. `/health` is never rate limited or cached.
