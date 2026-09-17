# Marketing telemetry acceptance

This PR adds opt-in React error hooks, sanitized page-load and interaction traces,
and a bootstrap log and counter. A blank DSN omits the telemetry module from the
build. Blank sampling values use 0.1; explicit zero disables trace sampling.

Configuration: `VITE_SENTRY_DSN`, `VITE_SENTRY_RELEASE`,
`VITE_SENTRY_ENVIRONMENT`, and `VITE_SENTRY_TRACES_SAMPLE_RATE` are build-time
values. The intended Sentry project is `sergtech/skill-studio-marketing`.
`build:observability` writes private hidden source maps to `dist-observability`.
Do not serve these maps; upload matching artifacts through the release process.
No project key or production telemetry is enabled by this PR.

## Local evidence — September 15, 2026

Marketing typecheck, scoped lint/format, and all eight unit tests passed. Tests
use in-memory transports and took 0.728 s. Disabled and enabled builds passed;
the fully sampled enabled fixture build took 0.665 s. Peak process-tree memory
was not measured. The preflight showed 44% free memory and 70 GiB free disk.

A compiled Chrome check used the actual page with every request intercepted by
`tools/verification/marketing-browser-fixture.mjs`. The fixture supplied static
build assets and accepted fake Sentry requests without external network traffic.
The enabled page rendered, and clicking Skills changed the product preview's
heading from Home to Skills. One injected global error, one page-load transaction,
one log container, one metric container and one interaction span were captured.
Payloads omitted the injected private query, fragment, error text and absolute
file path. Page-load measurements and click INP were present. These observations
are functional evidence, not a performance benchmark or native desktop test.

The disabled page rendered with zero telemetry requests and no telemetry chunk.
Three enabled JavaScript files had valid hidden source maps. The verifier found
the expected fixture DSN, release, instrumentation sources and SDK sources.
Browser sessions and fixture processes were closed. Raw payloads are disposable;
retain this summary, the reproduction tools, and the selected render below.

![Compiled marketing page with telemetry enabled](marketing-observability.png)

## Reproduce the bounded checks

Run the package typecheck and tests with one worker. Build once with a blank DSN
and once with `https://public@example.invalid/1`, a fixture release, environment
`test`, and trace sample rate `1`. Run the source-map verifier with those same
values. CI performs these builds and retains only JUnit for three days.

For browser acceptance, start an isolated agent-browser session on `about:blank`,
get its CDP URL, and run the fixture tool in a managed terminal with arguments
`<cdp-url> <absolute-build-directory> <temporary-capture-json>`. Wait for
`FIXTURE_READY`, then open `https://marketing-fixture.invalid/`. All requests on
that page are intercepted. Check rendering and a preview navigation action;
use a deliberate global exception containing a private marker to check redaction.
Background the page to flush interaction timing. Inspect only captured payloads
for marker removal. Repeat against the disabled build, then close the browser,
stop the fixture process, and delete captures and temporary builds.

## Review corrections

Independent review found that Sentry 10.73 merges scope attributes after log and
metric filters and derives standalone interaction envelope headers before the
span filter. A `beforeEnvelope` integration now clears serialized log/metric
attributes and fixes the header transaction to `marketing.page`. Tests use both
current and isolation scope markers and an actual SDK standalone span named
with a private DOM label. Trace identifiers remain correlated.

Caught and recoverable React errors now export `handled: true`; uncaught errors
remain unhandled. The three hook cases are checked separately. CI also scans the
disabled build for telemetry chunk and SDK markers, alongside enabled map checks.

Eleven tests passed in 0.754 s after the final source edit, with typecheck,
scoped lint/format and whitespace checks. Disabled and enabled builds took
0.437 s and 0.603 s of Vite time. Their map/exclusion verifier passed before the
final erased type annotation and function declaration adjustment; final CI
checks the committed build. Preflight: 46% memory free and 66 GiB disk free.
Peak memory was not collected. No browser, listener or remote export ran for
these corrections. The fresh cleanup pass was a no-op.

## Open release checks

Remote Sentry receipt, source-map upload/symbolication, alert routing, retention
settings, deployment, a real React render-error path, CLS collection, and
controlled telemetry overhead remain unverified. The bootstrap counter records
that rendering was scheduled; it is not a completed-paint measurement.

Source SHA-256 `src/main.tsx`: `8dc60e67cbaa2e97ebb5869c9732ebfc1f93978c502fccf64d43055e7a03d7d7`.

Source SHA-256 `src/marketing-instrument.ts`: `27de93d0addb67ea7ea4fbd6a750dfbbed2f8ea5534b20c909a01510adb4fea2`.

Source SHA-256 `src/marketing-telemetry.ts`: `5dea9dec4b6254092d93e7f9cb0aa1fdb300d004c536c80181f045e58f863a62`.
