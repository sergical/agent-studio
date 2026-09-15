# Skill Studio telemetry boundary

This crate sanitizes typed Sentry 0.49.2 envelopes and provides a bounded sender
with one sender worker and a Sentry transport adapter. It also supplies an
explicit opt-in SDK session and read-context layers for desktop Rust. Existing
CLI/MCP surface and operation values remain compatible; this extraction adds no
CLI/MCP implementation. It does not depend on the shared skill core. Importing
the crate does not initialize telemetry; callers invoke `session_from_environment`.
Application wiring and native acceptance are separate review units.

```rust
use skill_studio_telemetry::{sanitize_envelope, TelemetryEnvironment,
    TelemetryIdentity, TelemetrySurface};

let identity = TelemetryIdentity::new(
    TelemetrySurface::Cli, TelemetryEnvironment::Production, (1, 2, 3),
);
// An exporter receives raw_envelope from the SDK:
// let safe = sanitize_envelope(raw_envelope, &identity);
// Only safe envelopes may enter the export queue.
```

`sanitize_envelope(Envelope, &TelemetryIdentity) -> Option<SanitizedEnvelope>`
rebuilds headers and supported items. `None` means no supported items survived
or the final encoded envelope exceeded 256 KiB. The output wrapper owns the final encoded bytes and exposes `into_bytes()` for
transport use. `TelemetryExporter::try_enqueue` accepts this wrapper only. The
queue does not serialize the data again. Do not append raw scope data later.

Identity has fixed surface/environment enums and a numeric release tuple. It
cannot inherit private scope labels. An optional 12–64 character lowercase hex
build revision adds a release suffix. Prerelease identifiers are not supported.

| Input                                                                       | Retained output                                                                                                                                   |
| --------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Error/fatal event                                                           | Generic runtime-failure message, IDs, time, severity, sanitized trace context, trusted identity                                                   |
| Transaction                                                                 | Allowed read, scan/phase, repair-preview and history operations, IDs/times/status, allowed numeric data and outcome fields, sanitized child spans |
| Structured log                                                              | Fixed scan/phase, repair-preview, history and refresh lifecycle messages, trace ID/time, allowed fields and trusted identity                      |
| Counter                                                                     | Scan, repair-preview and history count names, counter type, unit absent, value exactly 1                                                          |
| Duration                                                                    | Scan, repair-preview and history duration names, distribution type, milliseconds, finite value from 0 through 86,400,000                          |
| Inventory count                                                             | `skill.scan.inventory_count`, gauge type, unit absent, integer value from 0 through u32::MAX                                                      |
| Raw envelope, attachment, session, client report, check-in, unknown variant | Dropped                                                                                                                                           |

Metric attributes accept only fixed outcome, extent, operation, and error-code
values plus trusted identity. They omit IDs and other unbounded labels. Logs and
span data additionally support selected/project/skill/ledger-only/discovery-issue
counts and duration. Values are checked before copying; arbitrary nested maps
under allowed keys are not cloned into output.

Only the first 64 envelope items and first 128 children per supported container
or transaction are examined. Excess items are dropped. These limits and the
encoded-size check bound retained output, not the SDK's original allocations,
formatting buffers, CPU, or total process memory. The Sentry serializer uses its
own per-item buffer. Sanitization and encoding run synchronously before queue admission. Their CPU
and allocation costs still need measurement. The sender bounds are below.

Error/fatal events tagged as cancelled, scope_busy, scope_deadline_exceeded, or
scope_changed are dropped. Normal callers must still avoid reporting expected
outcomes as errors. Raw exception text, native stacks, debug images, breadcrumbs,
user data, requests, and unrecognized context are omitted. This protects privacy
but does not satisfy native symbolication or meaningful crash grouping. Do not
enable production error export until a native frame/image policy is implemented
and verified.

Tests use hostile typed data and the actual SDK with an in-memory transport.
They prove four-signal trace correlation and retained read/scan nesting after
filtering. They do not prove remote receipt, IPC/worker propagation, panic
capture, symbolication, or runtime overhead.

```sh
cargo test --offline --locked --manifest-path crates/skill-studio-telemetry/Cargo.toml
cargo clippy --offline --locked --manifest-path crates/skill-studio-telemetry/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path crates/skill-studio-telemetry/Cargo.toml -- --check
```

## Bounded sender

```rust
TelemetryExporter::start(sink) -> std::io::Result<TelemetryExporter>
TelemetryExporter::try_enqueue(&self, SanitizedEnvelope) -> EnqueueOutcome
TelemetryExporter::flush(&self, Duration) -> FlushOutcome
TelemetryExporter::shutdown(&self, Duration) -> FlushOutcome
TelemetryExporter::stats(&self) -> ExportStats
```

The sink is a `FnMut(&[u8]) -> Result<(), ExportFailure> + Send + 'static`.
It runs only on the output worker.

The sender holds at most eight queued envelopes plus one in delivery. Each
encoded envelope is at most 256 KiB, so queued payload is at most 2 MiB. This
excludes the worker's active payload, producers, Vec spare capacity, SDK buffers,
and the thread stack. It is not a total RSS bound.

`try_enqueue` returns `Queued`, `Full`, or `Closed`. It uses short mutex/channel
critical sections; it does not wait for queue space or perform sink I/O. It is
not lock-free or a hard real-time API. Serialization has already occurred.

`shutdown(&self, budget)` closes admission, then lets the worker finish accepted
items. It returns `Drained`, `Failed`, or `TimedOut`. The requested wait is capped
at two seconds and uses one monotonic deadline per call. Full queues do not
require space for a shutdown message. Repeated calls can observe later drain
completion; each call has its own requested budget. The runtime must pass its
remaining overall budget, not repeatedly restart a full timeout.

A timed-out sink call is not forcibly interrupted. The worker is not joined; it
can finish later and will stop once its queue drains. Dropping the exporter
closes admission without waiting. A real sink must implement its own network
timeouts. Process exit ends remaining threads. No retry policy is implemented.

`stats()` reports accepted, delivered, failed, dropped-full, dropped-closed, and
worker-panic counts without recursively logging export failures. Delivered means
the supplied sink returned success; it does not prove a Sentry receipt. Stats
are individual atomic reads, not one atomic snapshot. During an active send the
fields can briefly describe different instants. A worker panic is a separate
count; pending accepted items are not reclassified as delivered or failed.
`Drained` covers accepted delivery only; inspect drop counters for lost records.
These counters are locally queryable, not yet exported as Sentry metrics.

Tests hold a sink while filling the queue, verify rejection and a short shutdown
timeout, release the sink and observe drain completion, count a failed delivery
without retry, and distinguish a worker panic from successful shutdown. These
are library tests, not HTTP or executable shutdown verification.

## Sentry and HTTP adapter

`SentryTransport` implements `sentry_core::Transport`. Its `send_envelope` first
runs the final sanitizer, then admits only sanitized bytes to the bounded sender.
It exposes sender stats plus a count of envelopes rejected by sanitization.
These are local counters, not SDK client reports or exported metrics.

`with_sink(identity, sink)` is available for fixtures and alternative sinks.
`http(&Dsn, identity)` supplies a reqwest 0.12.28 blocking sink with rustls TLS.
Client creation, requests, and client destruction occur on the output worker,
not in an async caller or scan thread. Reqwest can create its own internal
runtime thread; the earlier one-worker count describes the sender only.

The HTTP adapter accepts HTTPS or HTTP to literal loopback IP addresses. It
rejects DSNs containing a secret key. Plain remote HTTP, including named hosts
that might resolve to loopback, is rejected. Sentry auth uses the public DSN key
and a fixed client identifier. Bodies use `application/x-sentry-envelope` and
the DSN's envelope API path.

Redirects are disabled, ambient proxies are disabled, TLS certificate validation
is enabled, connection timeout is 500 ms, and request timeout is two seconds.
A 2xx status counts as delivered. Other statuses and network errors count as
failed. Response bodies and raw network error messages are neither logged nor
exported. No application retry, Retry-After/rate-limit handling, or durable spool
is implemented. These remain production policy work. Disabling ambient proxies
also means deployments that require a proxy need explicit future configuration.

SDK `flush` drains a snapshot of accepted envelopes without closing admission.
It uses the caller's budget capped at two seconds. New submissions remain
possible; later admissions do not extend that snapshot. SDK `shutdown` closes
admission and waits for worker completion. Both map only `Drained` to true.
An earlier delivery failure remains visible in later flush/shutdown results.
`SentrySession::close` caps the caller wait across SDK batchers and transport at
two seconds. CLI passes the remaining budget after local log shutdown. Desktop
uses this close on Tauri Exit. A timed-out close thread can finish later.

Tests exercise a real local HTTP request, request headers and sanitized body,
a redirect trap, a silent peer that reaches the request timeout, and two flushes
through an actual SDK client. Test listeners use temporary loopback ports and
are joined. The sandbox may require approval for these socket tests. No remote
Sentry project is contacted. TLS negotiation, DNS failure, production receipt,
rate limits, and whole-executable shutdown are not established by these tests.

## Extraction verification — September 15, 2026

The extraction preserves the integration production source and Cargo files.
The original 29 tests passed with the locked offline dependency graph, two build workers
and one test thread: 8.06 s wall time, 93,503,488 bytes maximum RSS for one process
(not process-tree peak), zero swaps. Strict all-target Clippy passed in 1.30 s;
formatting passed. The existing target cache was reused. Preflight showed 47%
free memory and 67 GiB free disk. Managed test and check processes exited.

A subsequent test-only review fix adds three direct `SentrySession::close` cases:
delivery success/failure, bounded stalled delivery and main-client replacement.
All three passed in 0.03 s after a 2.10 s build. Strict all-target Clippy passed
in 1.45 s and formatting passed. Peak memory was not collected for this focused
run. Production source remained unchanged; the earlier full-test result applies.

Tests use in-memory sinks and joined temporary loopback peers. They send no
production telemetry. CI repeats formatting, strict Clippy and tests with two
build workers, one test thread, a ten-minute deadline and three-day test-log
retention. Application wiring, native acceptance, remote receipt and the native
frame/image policy remain separate delivery requirements.
