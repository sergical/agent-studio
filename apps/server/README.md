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

## API telemetry

The Node entry point preloads `src/instrument.ts` before the server module. The
supported npm command includes this preload. A separate runtime command must
preserve the order: `node --import tsx --import ./src/instrument.ts src/server.ts`
from this package directory. Local dev servers still run through portless in a
managed terminal pane, as required by the repository instructions.

| Variable                    | Meaning                                                         |
| --------------------------- | --------------------------------------------------------------- |
| `SENTRY_DSN`                | Enables telemetry; absent or blank leaves the SDK uninitialized |
| `SENTRY_RELEASE`            | Deployed release identifier; required for release verification  |
| `SENTRY_ENVIRONMENT`        | Deployment environment; defaults to `development`               |
| `SENTRY_TRACES_SAMPLE_RATE` | Number from 0 through 1; defaults to `0.1`                      |

The pinned SDKs are `@sentry/hono` and `@sentry/node` 10.73.0. The integration
follows the [official Node Hono setup](https://github.com/getsentry/sentry-javascript/blob/develop/packages/hono/README.md).
The API creates one isolated request trace around raw-target validation and
routing, with child spans for upstream work. Failed upstream fetches and Hono
handler errors produce error events. Request logs, request counts, and request
duration distributions share trace context. Trace sampling does not sample
logs or metrics: each request produces one Sentry log and two request metrics.
Stdout request records are best effort. While stdout needs drain or is destroyed,
the API skips new stdout records and increments `api.request.stdout_dropped` in
Sentry with the same bounded method/route/status labels. It resumes stdout logging
after drain. This bounds additional request-log buffering; it does not make a
synchronous stdout sink nonblocking or establish a total SDK memory budget.
The drop counter is exported only when telemetry is enabled. The
`src/request-telemetry.test.ts` fixture verifies this backpressure behavior.

Export hooks retain fixed method/route labels, HTTP status, timings, trace IDs,
and configured release/environment metadata. They discard request data, headers,
query text, exception messages, user data, source context, arbitrary log messages,
and arbitrary metric names. Exception frames retain only known API source file
names and line/column numbers, with paths rewritten to `app:///src/`; external
frames lose their filenames. This conservative policy reduces error detail.
It is not proof that deployed source maps resolve those paths.

Automatic HTTP instrumentation, outgoing trace propagation, console capture,
breadcrumbs, and profiling are disabled. Hono's default error console output is
replaced with the same generic 500 response; the SDK middleware captures the
error once. Existing upstream response bodies remain unchanged. SIGINT/SIGTERM
stop accepting connections and allow two seconds for active connections. The
runtime then aborts upstream fetch/body reads, destroys remaining client
connections, waits up to two seconds for handlers, and requests a two-second SDK
flush. Repeated signals share the same shutdown. Complete shutdown exits zero;
unsettled handlers or failed flush exit one. A referenced 6.5-second watchdog
exits one if the transport does not honor its deadline. This bound assumes the
Node event loop can run; it cannot interrupt synchronous blocking code.

Unix-socket child-process fixtures verify idle, header-pending, body-pending,
repeated-signal, disabled-telemetry, ignored-abort, and stalled-transport cases.
Cancelled upstream work is recorded as a 503 request, with no error event.
Header/body cancellation prevents false success telemetry during shutdown.
These tests use the production runtime and handler factories but fixture
upstream and telemetry transports. They do not prove remote flush delivery or
the behavior of a deployment supervisor.

Local acceptance uses an in-memory Sentry transport. The
`src/api-telemetry.test.ts` fixture proves envelope shape, correlation,
concurrent request isolation, and fixture sanitization without sending events.
Production ingestion, source maps, alert routing, distributed trace propagation,
and deployment shutdown remain release checks.

## Production build

```sh
npm run build --workspace apps/server
npm run test:production-build --workspace apps/server
npm run start --workspace apps/server
```

Build uses the pinned esbuild version to produce Node ESM entry points in `dist`.
Package dependencies stay external; deploy the package with its production
`node_modules`, not just the two JavaScript files. The start command preloads
`dist/instrument.js` before `dist/server.js`. It does not require tsx or load the
repository `.env`; the deployment supplies `SKILLS_SH_API_KEY`, `PORT` and the
Sentry variables above. The existing loopback-only bind remains unchanged: this
command does not expose the service publicly or configure a reverse proxy.

External `.js.map` files contain source content and have no sourceMappingURL in
the JavaScript. Retain them privately for upload and release evidence. Do not
serve them. The telemetry filter preserves only exact `server.js` and
`instrument.js` bundle names under `app:///dist/`, with line/column data, so the
upload must match those artifact names and the configured release. Upload and
production symbolication have not been verified.

`test:production-build` requires a completed build. It runs child Node processes
with an isolated test environment, checks compiled health and traversal rejection,
checks missing-key startup refusal, and validates both maps. It opens no listener
and sends no external requests. The separate runtime tests exercise shutdown
through temporary Unix sockets. Neither test establishes deployed Sentry receipt,
production dependency packaging or supervisor behavior.

## Remote verification — 2026-09-15

The reviewed code at `36f8e29841bf5a090572f899934ffe44d5080e92` was rebuilt and
exercised through its compiled `createNodeRequestHandler` with instrumentation
preloaded. No listener or deployment was started. Two synthetic requests produced
health 200 and a stubbed upstream failure 502. The SDK completed its bounded flush
and the process exited (0.54 s command wall time; peak memory was not measured).

Destination: `sergtech/skill-studio-api`, environment `verification`, release
`skill-studio-api@36f8e298-remote-20260915`. Sampling was 1 for this two-request
check only. No real upstream key or request data was used. Account inspection used
Executor's personal Sentry MCP connection. Read-back confirmed:

- [Error SKILL-STUDIO-API-1](https://sergtech.sentry.io/issues/SKILL-STUDIO-API-1),
  event `9c4fb9c786d64ff895fd9578db91f9b7`, at 16:02:18 UTC.
- Two `api.request.completed` logs linked to the request traces.
- Three spans for the failed request: server, Hono middleware and upstream.
- Two `api.request.count` samples of 1 and two `api.request.duration` samples,
  0.928 ms and 17.368 ms. These fixture timings are not a performance baseline.

[Failed-request trace](https://sergtech.sentry.io/explore/traces/trace/6c8400856f354457baba4ee3035dd2c1)
and [health trace](https://sergtech.sentry.io/explore/traces/trace/860e94c87c8144b196643f14af16247b)
provide the correlation identifiers for logs and metrics. Search each dataset in
the API project for the verification timestamp; the error also carries the release
and environment above. These links remain subject to Sentry retention; no ongoing
test export was enabled and custom retention has not been verified.

The first check found `OTHER unmatched` on the error and middleware span. The
route correction sets allowlisted labels before the handler runs and keeps them
isolated per request. A second compiled synthetic failure returned 502 and flushed
successfully (0.43 s; peak memory not measured). Executor read-back confirmed:

- Release `skill-studio-api@route-fix-20260915`, environment `verification`.
- Event `51dc8902c35d4ef984b69062e7ae5fdb` at 16:13:45 UTC in the same issue.
- [Corrected trace](https://sergtech.sentry.io/explore/traces/trace/0664e2c229ec4208801fad15c1064d38):
  error, server and middleware use `GET /api/v1/skills/search`; upstream uses
  `skills.upstream`. Returned summaries exclude synthetic private markers.

Focused verification: 14 telemetry tests passed in 0.869 s, including concurrent
failures completed in reverse order. A subsequent erased generic type annotation
fixed test compilation; the server build/typecheck, scoped lint/format and
whitespace checks passed. The second remote check used that compiled candidate.
No listener remained and no raw payload files were retained.

The subsequent scope-attribute review found that Sentry 10.73 merges scope data
after `beforeSendLog` and `beforeSendMetric`. The API now validates serialized
method, route and status labels immediately before transport and removes all
other log/metric attributes. The fixture injects private current/isolation scope
attributes and an invalid inherited status. Fifteen focused tests passed in
0.821 s, including existing request trace correlation, followed by typecheck,
scoped lint/format and whitespace checks. Peak memory was not measured; no
browser, remote export or persistent capture ran for this correction.

Acceptance remains incomplete:

- First-party frames remain `app:///dist/server.js`; source-map resolution is not
  verified. The issue's code location points to a different repository, so release
  and repository mapping also need correction.
- Synthetic query/header markers are absent from the returned event summary,
  but the server added geographic context. Full payload redaction and server-side
  privacy configuration remain unverified; this is not a complete redaction pass.
- Production deployment, alert delivery, and retention configuration remain open.

The first read-back hit an internal connector error. A read-only retry succeeded;
no duplicate test requests were sent. Metric search succeeded with the metrics
dataset's default fields after the natural-language query was rejected. No raw
telemetry payload files were retained locally.
