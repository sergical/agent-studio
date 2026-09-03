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
