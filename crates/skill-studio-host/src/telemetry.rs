//! Sentry crash-report client (PR1, unit 6.4): one event per Rust panic,
//! carrying the code location, a redacted stack trace, and OS/CPU/`surface`
//! context - gated end-to-end by a live [`Consent`] flag. See `docs/spec-headless-performance-observability.md`.

use std::panic::PanicHookInfo;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, SystemTime};

use sentry::protocol::{Context, Event, Map, Span, SpanStatus, TraceContext, Transaction};
use sentry::transports::ReqwestHttpTransportOptions;
use sentry::{ClientInitGuard, ClientOptions, Envelope, Hub, Level, Transport, TransportFactory};

use skill_studio_core::ports::{NoopTelemetry, OpOutcome, OpRecord, Telemetry};

/// Run-time fallback for the compile-time DSN; see [`resolve_dsn`].
const DSN_ENV_VAR: &str = "SKILL_STUDIO_SENTRY_DSN";

/// Flush budget for [`shutdown`] - also `ClientOptions::shutdown_timeout`,
/// so a slow client-side flush attempt and the caller's own patience agree.
pub const SHUTDOWN_FLUSH: Duration = Duration::from_secs(2);

/// One tag value per binary - the only thing that tells two otherwise
/// identical panics apart in Sentry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The Tauri desktop app.
    Desktop,
    /// The `skill-studio` CLI. Not wired up by this PR.
    Cli,
    /// The stdio MCP server. Not wired up by this PR.
    Mcp,
}

impl Surface {
    /// The `tags["surface"]` value this surface sends.
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Desktop => "desktop",
            Surface::Cli => "cli",
            Surface::Mcp => "mcp",
        }
    }
}

/// Live consent flag shared by the transport gate and the callers that flip
/// it (Settings' switch, the first-run screen). Cloning shares the flag -
/// every clone reads and writes the same underlying `AtomicBool`.
#[derive(Clone)]
pub struct Consent(Arc<AtomicBool>);

impl Consent {
    /// Builds a flag starting at `enabled`.
    pub fn new(enabled: bool) -> Self {
        Self(Arc::new(AtomicBool::new(enabled)))
    }

    /// The current value.
    pub fn enabled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Flips the flag. Takes effect on the next envelope [`ConsentTransport`]
    /// is asked to send.
    pub fn set(&self, enabled: bool) {
        self.0.store(enabled, Ordering::Relaxed);
    }
}

/// Keeps the Sentry client alive. Dropping it without [`shutdown`] still
/// closes the client - `sentry::ClientInitGuard`'s own `Drop` does that -
/// but without the caller's own flush budget or return value.
pub struct TelemetryGuard(ClientInitGuard);

/// Starts Sentry when a DSN is available (compile-time wins, then
/// [`DSN_ENV_VAR`] at run time), installs the panic hook, and returns the
/// guard. Returns `None` - and installs nothing - when no DSN is available
/// or the available one doesn't parse, which is every build until
/// `SKILL_STUDIO_SENTRY_DSN` is set to a valid DSN. Never panics:
/// `resolve_dsn` fails closed rather than falling back or panicking.
/// `sentry::init` runs before [`install_panic_hook`]: the process hub is
/// owned by whichever thread first touches a hub, so a panic on another
/// thread between the two calls would otherwise bind the client to the
/// wrong hub.
pub fn init(
    surface: Surface,
    app_version: &'static str,
    consent: Consent,
) -> Option<TelemetryGuard> {
    let dsn = resolve_dsn(
        option_env!("SKILL_STUDIO_SENTRY_DSN"),
        std::env::var(DSN_ENV_VAR).ok(),
    )?;
    let guard = sentry::init(client_options(dsn, surface, app_version, consent));
    install_panic_hook(surface);
    Some(TelemetryGuard(guard))
}

/// Flushes queued envelopes for at most [`SHUTDOWN_FLUSH`] and closes the
/// client. Returns what the transport reported - `true` only if the queue
/// fully drained in time. Takes the guard by value, not `&TelemetryGuard`,
/// so the caller cannot reuse a client that is already shutting down.
#[allow(clippy::needless_pass_by_value)]
pub fn shutdown(guard: TelemetryGuard) -> bool {
    guard.0.close(Some(SHUTDOWN_FLUSH))
}

/// The compile-time value wins; the run-time variable is the fallback so a
/// developer can point a local build at a test project without a rebuild.
/// An empty string counts as absent either way - an unset repo variable in
/// CI expands to `""`, not an omitted `env:` entry. A value that doesn't
/// parse as a DSN - from either source - returns `None` rather than falling
/// back to the other source: a malformed compile-time DSN must disable
/// telemetry, not silently let a run-time value (or `sentry::apply_defaults`
/// reading `SENTRY_DSN` from the environment) take over.
fn resolve_dsn(build: Option<&str>, process_env: Option<String>) -> Option<sentry::types::Dsn> {
    fn non_empty(value: String) -> Option<String> {
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    }
    let raw = build
        .map(str::to_string)
        .and_then(non_empty)
        .or_else(|| process_env.and_then(non_empty))?;
    raw.parse().ok()
}

/// Builds the options `init` hands to `sentry::init`. Kept apart from `init`
/// so a test can inspect the values without starting a real client.
fn client_options(
    dsn: sentry::types::Dsn,
    surface: Surface,
    app_version: &'static str,
    consent: Consent,
) -> ClientOptions {
    // `ClientOptions` is `#[non_exhaustive]`, so a struct-literal
    // (even with `..Default::default()`) doesn't compile outside its own
    // crate - build the default and mutate the fields this module cares
    // about instead.
    let mut options = ClientOptions::default();
    options.dsn = Some(dsn);
    options.release = Some(format!("skill-studio@{app_version}").into());
    options.environment = Some(
        if cfg!(debug_assertions) {
            "development"
        } else {
            "production"
        }
        .into(),
    );
    options.server_name = Some("skill-studio".into());
    options.send_default_pii = false;
    options.attach_stacktrace = true;
    options.max_breadcrumbs = 0;
    options.shutdown_timeout = SHUTDOWN_FLUSH;
    options.before_send = Some(Arc::new(move |event| Some(redact_event(event, surface))));
    options.transport = Some(Arc::new(ConsentTransportFactory { consent }));
    options.sample_rate(1.0)
}

/// `before_send`'s body, factored out so a test can call it directly. A
/// safety net behind the panic hook: every *event* passes through here.
/// Raw envelopes (`Client::send_envelope`, the path transactions will use)
/// skip `before_send`, so their fields are shaped by their builder instead.
fn redact_event(mut event: Event<'static>, surface: Surface) -> Event<'static> {
    event.server_name = None;
    event.user = None;
    event.request = None;
    event.breadcrumbs = Default::default();
    event.extra = Map::new();
    event
        .contexts
        .retain(|key, _| matches!(key.as_str(), "os" | "device" | "rust" | "runtime"));
    // `sentry-backtrace` fills `abs_path` with the full build-machine path
    // (e.g. `/Users/someone/.cargo/registry/...`); `filename` already holds
    // just the basename. Strip `abs_path` from every frame - function
    // names, line numbers, and basenames stay, so the trace is still useful
    // without naming the machine it was built on.
    for thread in &mut event.threads.values {
        if let Some(stacktrace) = thread.stacktrace.as_mut() {
            for frame in &mut stacktrace.frames {
                frame.abs_path = None;
            }
        }
    }
    for exception in &mut event.exception.values {
        if let Some(stacktrace) = exception.stacktrace.as_mut() {
            for frame in &mut stacktrace.frames {
                frame.abs_path = None;
            }
        }
    }
    event
        .tags
        .insert("surface".to_string(), surface.as_str().to_string());
    event
}

/// Wraps the real transport so [`Transport::send_envelope`] forwards only
/// while `consent.enabled()` is true; otherwise it drops the envelope. This
/// is the single gate for everything the client could send - events now,
/// transactions in PR2 - because both paths funnel through one client whose
/// `transport` is always a `ConsentTransport`.
struct ConsentTransport {
    inner: Arc<dyn Transport>,
    consent: Consent,
}

impl Transport for ConsentTransport {
    fn send_envelope(&self, envelope: Envelope) {
        if self.consent.enabled() {
            self.inner.send_envelope(envelope);
        }
    }

    fn flush(&self, timeout: Duration) -> bool {
        self.inner.flush(timeout)
    }

    fn shutdown(&self, timeout: Duration) -> bool {
        self.inner.shutdown(timeout)
    }
}

struct ConsentTransportFactory {
    consent: Consent,
}

impl TransportFactory for ConsentTransportFactory {
    /// Bounds the HTTP client to [`SHUTDOWN_FLUSH`]: `sentry`'s own reqwest
    /// client has no timeout, and `TransportThread::drop` joins the worker,
    /// which waits for any in-flight request - so an unbounded client could
    /// hang quit on a stalled network. A client that fails to build (a
    /// misconfigured TLS backend) falls back to the unbounded default
    /// rather than losing the transport entirely.
    fn create_transport_with_options(
        &self,
        options: sentry::TransportOptions,
    ) -> Arc<dyn Transport> {
        let client = reqwest::Client::builder().timeout(SHUTDOWN_FLUSH).build();
        let opts = ReqwestHttpTransportOptions::from(options);
        let inner: Arc<dyn Transport> = match client {
            Ok(client) => Arc::new(opts.with_client(client).build()),
            Err(_) => Arc::new(opts.build()),
        };
        Arc::new(ConsentTransport {
            inner,
            consent: self.consent.clone(),
        })
    }
}

/// `panic_event`'s message: "panicked at <file>:<line>:<column>" for a known
/// location, or a fixed sentence for none. This module's privacy promise -
/// only the place in the code where it crashed, never the panic payload
/// text - rests on `panic_event` never reading `info.payload()`, with
/// `redact_event`'s stripping as a second line of defense.
pub fn message_from_location(location: Option<&std::panic::Location<'_>>) -> String {
    match location {
        Some(location) => format!(
            "panicked at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        ),
        None => "panicked at an unknown location".to_string(),
    }
}

/// Builds the event a panic reports: the location only, `Fatal`, tagged with
/// `surface`. No `exception`, no `extra` - `info.payload()` (whatever the
/// panicking code passed to `panic!()`, which can quote a path or a skill
/// name) is never read, so it cannot reach this event by construction.
fn panic_event(info: &PanicHookInfo<'_>, surface: Surface) -> Event<'static> {
    let mut tags = Map::new();
    tags.insert("surface".to_string(), surface.as_str().to_string());
    Event {
        message: Some(message_from_location(info.location())),
        level: Level::Fatal,
        tags,
        ..Default::default()
    }
}

/// The hook body `install_panic_hook` installs: reports `panic_event`
/// through `sentry::capture_event` (a no-op with no client bound), then
/// flushes the bound client for at most [`SHUTDOWN_FLUSH`] before running
/// `next`, so a panic still prints to stderr exactly as it did without this
/// unit when `next` is the hook that was previously installed. The flush
/// matters because `capture_event` only queues the envelope on the
/// transport thread - a main-thread panic on macOS never reaches
/// `RunEvent::Exit` (`tao` resumes the unwind before `LoopDestroyed`), so
/// without it the crash report is lost. Mirrors `sentry_panic::panic_handler`.
/// Factored out from `install_panic_hook` so a test can build and install a
/// fresh hook directly: `install_panic_hook`'s `Once` guards its one
/// production caller (`init`) and only ever fires once, so a second test
/// wanting its own hook swapped in and out around its own panic can't go
/// through `install_panic_hook` - the `Once` would already be spent by
/// whichever hook test ran first.
fn panic_hook(
    surface: Surface,
    next: Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync>,
) -> Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync> {
    Box::new(move |info| {
        sentry::capture_event(panic_event(info, surface));
        if let Some(client) = sentry::Hub::current().client() {
            client.flush(Some(SHUTDOWN_FLUSH));
        }
        next(info);
    })
}

static PANIC_HOOK_INSTALLED: Once = Once::new();

/// Installs [`panic_hook`] wrapping whatever hook was previously in place.
/// Installs at most once per process - a second call (there is only one
/// caller, `init`) is a no-op rather than double-wrapping the hook.
fn install_panic_hook(surface: Surface) {
    PANIC_HOOK_INSTALLED.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(panic_hook(surface, default_hook));
    });
}

/// One [`OpRecord`]/desktop command, turned into a Sentry transaction by
/// hand: `record`/`record_command` bypass `before_send` entirely (it only
/// runs for `Event`s), so every field a shipped transaction needs is set
/// here instead of relying on the SDK's defaults.
pub struct SentryTelemetry {
    surface: Surface,
    release: String,
    environment: &'static str,
}

impl SentryTelemetry {
    fn environment() -> &'static str {
        if cfg!(debug_assertions) {
            "development"
        } else {
            "production"
        }
    }
}

/// `Ok` -> `SpanStatus::Ok`, `Err` -> `SpanStatus::InternalError` - shared by
/// a transaction's root trace context and (for `record`) nothing else: step
/// spans always report `Ok`, since a step that ran at all completed.
fn outcome_status(ok: bool) -> SpanStatus {
    if ok {
        SpanStatus::Ok
    } else {
        SpanStatus::InternalError
    }
}

/// `surface`/`outcome`[/`error_code`] - the only tags [`SentryTelemetry::record`]
/// and [`record_command`] ever attach, so a transaction can carry no more
/// than these three typed values, never free text.
fn outcome_tags(surface: Surface, ok: bool, error_code: Option<&str>) -> Map<String, String> {
    let mut tags = Map::new();
    tags.insert("surface".to_string(), surface.as_str().to_string());
    tags.insert(
        "outcome".to_string(),
        (if ok { "ok" } else { "error" }).to_string(),
    );
    if let Some(code) = error_code {
        tags.insert("error_code".to_string(), code.to_string());
    }
    tags
}

impl Telemetry for SentryTelemetry {
    fn record(&self, record: OpRecord) {
        let ok = matches!(record.outcome, OpOutcome::Ok);
        let elapsed = Duration::from_millis(record.timing.elapsed_ms);
        let timestamp = SystemTime::now();
        let start_timestamp = timestamp - elapsed;
        let op = format!("skill.{}", record.timing.op);
        let root_span_id = sentry::protocol::SpanId::default();
        let trace_id = sentry::protocol::TraceId::default();

        let mut spans = Vec::with_capacity(record.timing.steps.len());
        let mut offset = Duration::ZERO;
        for step in &record.timing.steps {
            let step_elapsed = Duration::from_millis(step.elapsed_ms);
            let step_start = start_timestamp + offset;
            spans.push(Span {
                span_id: sentry::protocol::SpanId::default(),
                trace_id,
                parent_span_id: Some(root_span_id),
                op: Some("skill.step".to_string()),
                description: Some(step.name.clone()),
                start_timestamp: step_start,
                timestamp: Some(step_start + step_elapsed),
                status: Some(SpanStatus::Ok),
                ..Default::default()
            });
            offset += step_elapsed;
        }

        let error_code = match &record.outcome {
            OpOutcome::Err { code } => Some(code.as_str()),
            OpOutcome::Ok => None,
        };
        let tags = outcome_tags(self.surface, ok, error_code);

        let mut contexts = Map::new();
        contexts.insert(
            "trace".to_string(),
            Context::Trace(Box::new(TraceContext {
                trace_id,
                span_id: root_span_id,
                op: Some(op.clone()),
                status: Some(outcome_status(ok)),
                ..Default::default()
            })),
        );

        let transaction = Transaction {
            name: Some(op),
            release: Some(self.release.clone().into()),
            environment: Some(self.environment.into()),
            platform: "native".into(),
            timestamp: Some(timestamp),
            start_timestamp,
            spans,
            contexts,
            tags,
            server_name: None,
            user: None,
            ..Default::default()
        };

        if let Some(client) = Hub::current().client() {
            let mut envelope = Envelope::new();
            envelope.add_item(transaction);
            client.send_envelope(envelope);
        }
    }
}

/// `SentryTelemetry` when `init` bound a client, [`NoopTelemetry`] otherwise
/// - the same "no DSN, nothing happens" fallback [`init`] uses.
pub fn port(surface: Surface, app_version: &str) -> Arc<dyn Telemetry> {
    if Hub::current().client().is_some() {
        Arc::new(SentryTelemetry {
            surface,
            release: format!("skill-studio@{app_version}"),
            environment: SentryTelemetry::environment(),
        })
    } else {
        Arc::new(NoopTelemetry)
    }
}

/// One transaction per desktop Tauri command, parallel to [`SentryTelemetry::record`]
/// but without spans: `command`'s error text is free text a `CommandOutcome`
/// can carry a path in, so only `ok`/`error` and the command name itself are
/// ever tagged - never the error text.
pub fn record_command(surface: Surface, command: &'static str, elapsed_ms: u64, ok: bool) {
    let Some(client) = Hub::current().client() else {
        return;
    };
    let elapsed = Duration::from_millis(elapsed_ms);
    let timestamp = SystemTime::now();
    let start_timestamp = timestamp - elapsed;
    let name = format!("command.{command}");
    let tags = outcome_tags(surface, ok, None);

    let mut contexts = Map::new();
    contexts.insert(
        "trace".to_string(),
        Context::Trace(Box::new(TraceContext {
            op: Some("command".to_string()),
            status: Some(outcome_status(ok)),
            ..Default::default()
        })),
    );

    let release = client.options().release.clone();
    let environment = client.options().environment.clone();
    let transaction = Transaction {
        name: Some(name),
        release,
        environment,
        platform: "native".into(),
        timestamp: Some(timestamp),
        start_timestamp,
        contexts,
        tags,
        server_name: None,
        user: None,
        ..Default::default()
    };

    let mut envelope = Envelope::new();
    envelope.add_item(transaction);
    client.send_envelope(envelope);
}

/// The one key this reads out of `<home>/.agents/skill-studio.json` -
/// `#[serde(default)]` so a missing key, not just a missing file, still
/// resolves to `false` rather than failing to deserialize.
#[derive(serde::Deserialize, Default)]
struct TelemetryRegistry {
    #[serde(default)]
    error_reporting_enabled: bool,
}

/// Reads only `error_reporting_enabled` from `<home>/.agents/skill-studio.json`.
/// Missing file, malformed file, or missing key all resolve to `false` -
/// consent defaults closed, never open.
pub fn consent_from_registry(home: &Path) -> bool {
    let path = home.join(".agents").join("skill-studio.json");
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<TelemetryRegistry>(&contents)
        .map(|registry| registry.error_reporting_enabled)
        .unwrap_or(false)
}

/// `SKILL_STUDIO_TELEMETRY=0|1` (also `"false"`/`"true"`) overrides the
/// registry value; any other string, or no variable at all, leaves the
/// registry's value untouched.
// `env_override` takes ownership, not a borrow, to match every call site
// (`std::env::var(..).ok()`) and the CLI/MCP/desktop's shared call shape.
#[allow(clippy::needless_pass_by_value)]
pub fn resolve_consent(env_override: Option<String>, registry: bool) -> bool {
    match env_override.as_deref() {
        Some("1" | "true") => true,
        Some("0" | "false") => false,
        _ => registry,
    }
}

#[cfg(all(test, feature = "telemetry"))]
mod tests {
    use super::*;
    use sentry::protocol::EnvelopeItem;
    use skill_studio_core::identity::CorrelationId;
    use skill_studio_core::ops::Operation;
    use skill_studio_core::timing::{OpTiming, StepTiming};

    fn transaction_from(envelope: &Envelope) -> Transaction<'static> {
        let mut transactions = envelope.items().filter_map(|item| match item {
            EnvelopeItem::Transaction(t) => Some((**t).clone()),
            _ => None,
        });
        let transaction = transactions
            .next()
            .expect("envelope must hold one transaction item");
        assert!(
            transactions.next().is_none(),
            "envelope must hold exactly one transaction item"
        );
        transaction
    }

    fn three_step_record(op: &str) -> OpRecord {
        OpRecord {
            operation: Operation::Scan,
            correlation_id: CorrelationId("c-telemetry".into()),
            outcome: OpOutcome::Ok,
            timing: OpTiming {
                op: op.to_string(),
                elapsed_ms: 60,
                steps: vec![
                    StepTiming {
                        name: "read_roots".into(),
                        elapsed_ms: 10,
                    },
                    StepTiming {
                        name: "read_skills".into(),
                        elapsed_ms: 20,
                    },
                    StepTiming {
                        name: "build_inventory".into(),
                        elapsed_ms: 30,
                    },
                ],
            },
        }
    }

    fn telemetry() -> SentryTelemetry {
        SentryTelemetry {
            surface: Surface::Desktop,
            release: "skill-studio@9.9.9".to_string(),
            environment: "development",
        }
    }

    /// guards: a step's start offset must equal the sum of the steps before
    /// it, not (say) always start at the root's own start - the three spans
    /// otherwise overlap or drift from the timings they represent.
    #[test]
    fn an_op_record_with_three_steps_becomes_one_transaction_with_three_child_spans() {
        let record = three_step_record("scan");
        let envelopes = sentry::test::with_captured_envelopes(|| {
            telemetry().record(record.clone());
        });
        assert_eq!(envelopes.len(), 1);
        let transaction = transaction_from(&envelopes[0]);

        assert_eq!(transaction.name.as_deref(), Some("skill.scan"));
        assert_eq!(transaction.release.as_deref(), Some("skill-studio@9.9.9"));
        assert_eq!(transaction.environment.as_deref(), Some("development"));
        assert!(transaction.server_name.is_none());
        assert!(transaction.user.is_none());
        assert_eq!(
            transaction.tags.get("surface").map(String::as_str),
            Some("desktop")
        );
        assert_eq!(
            transaction.tags.get("outcome").map(String::as_str),
            Some("ok")
        );
        assert_eq!(transaction.tags.len(), 2, "no tag beyond surface/outcome");
        assert!(matches!(
            transaction.contexts.get("trace"),
            Some(Context::Trace(_))
        ));

        assert_eq!(transaction.spans.len(), 3);
        let root_span_id = transaction
            .contexts
            .get("trace")
            .and_then(|context| match context {
                Context::Trace(trace) => Some(trace.span_id),
                _ => None,
            })
            .expect("trace context missing");
        let expected_offsets_ms = [0u64, 10, 30];
        let expected_durations_ms = [10u64, 20, 30];
        for (i, span) in transaction.spans.iter().enumerate() {
            assert_eq!(span.parent_span_id, Some(root_span_id));
            assert_eq!(span.op.as_deref(), Some("skill.step"));
            let start_offset = span
                .start_timestamp
                .duration_since(transaction.start_timestamp)
                .expect("span starts after the root");
            assert_eq!(start_offset.as_millis() as u64, expected_offsets_ms[i]);
            let duration = span
                .timestamp
                .expect("span timestamp")
                .duration_since(span.start_timestamp)
                .expect("span ends after it starts");
            assert_eq!(duration.as_millis() as u64, expected_durations_ms[i]);
        }
        assert_eq!(
            transaction.spans[0].description.as_deref(),
            Some("read_roots")
        );
        assert_eq!(
            transaction.spans[2].description.as_deref(),
            Some("build_inventory")
        );
    }

    /// guards: a failed op losing its error code tag, or reporting the trace
    /// context status as `ok` despite the failure.
    #[test]
    fn a_failed_op_record_carries_the_error_code_tag_and_error_status() {
        let record = OpRecord {
            operation: Operation::Install,
            correlation_id: CorrelationId("c-fail".into()),
            outcome: OpOutcome::Err {
                code: "invalid_request".to_string(),
            },
            timing: OpTiming {
                op: "install".to_string(),
                elapsed_ms: 5,
                steps: Vec::new(),
            },
        };
        let envelopes = sentry::test::with_captured_envelopes(|| {
            telemetry().record(record);
        });
        let transaction = transaction_from(&envelopes[0]);

        assert_eq!(
            transaction.tags.get("outcome").map(String::as_str),
            Some("error")
        );
        assert_eq!(
            transaction.tags.get("error_code").map(String::as_str),
            Some("invalid_request")
        );
        let trace_status = transaction
            .contexts
            .get("trace")
            .and_then(|context| match context {
                Context::Trace(trace) => Some(trace.status),
                _ => None,
            })
            .expect("trace context missing");
        assert_eq!(trace_status, Some(SpanStatus::InternalError));
    }

    /// guards: `record`'s typed-fields-only shape - the type itself, not a
    /// redaction pass, is what keeps a path or a skill name out, since no
    /// field on `OpRecord` can carry one.
    #[test]
    fn a_home_path_and_a_skill_name_never_appear_in_a_serialized_transaction() {
        let record = three_step_record("scan");
        let envelopes = sentry::test::with_captured_envelopes(|| {
            telemetry().record(record);
        });
        let mut bytes = Vec::new();
        envelopes[0].to_writer(&mut bytes).expect("serialize");
        let serialized = String::from_utf8_lossy(&bytes);

        assert!(!serialized.contains("/Users/"));
        assert!(!serialized.contains("sentinel-skill-name"));
    }

    /// guards: `consent_from_registry` reading any key besides the switch,
    /// or a present-but-false switch resolving to `true`.
    #[test]
    fn consent_from_registry_reads_only_the_switch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agents_dir = dir.path().join(".agents");
        std::fs::create_dir_all(&agents_dir).expect("mkdir");
        let registry_path = agents_dir.join("skill-studio.json");

        assert!(
            !consent_from_registry(dir.path()),
            "a missing registry file must resolve to false"
        );

        std::fs::write(&registry_path, "not json").expect("write");
        assert!(
            !consent_from_registry(dir.path()),
            "a malformed registry file must resolve to false"
        );

        std::fs::write(&registry_path, r#"{"skills_sh_api_key": "x"}"#).expect("write");
        assert!(
            !consent_from_registry(dir.path()),
            "an absent switch key must resolve to false"
        );

        std::fs::write(
            &registry_path,
            r#"{"skills_sh_api_key": "x", "error_reporting_enabled": true}"#,
        )
        .expect("write");
        assert!(
            consent_from_registry(dir.path()),
            "a true switch alongside other keys must resolve to true"
        );
    }

    /// guards: the env override losing to the registry, or an unrecognized
    /// override string silently disabling telemetry instead of falling
    /// back to the registry.
    #[test]
    fn the_env_override_wins_over_the_registry() {
        let cases: &[(Option<&str>, bool, bool)] = &[
            (Some("0"), true, false),
            (Some("1"), false, true),
            (Some("false"), true, false),
            (Some("true"), false, true),
            (None, true, true),
            (None, false, false),
        ];
        for (env, registry, expected) in cases {
            let actual = resolve_consent(env.map(str::to_string), *registry);
            assert_eq!(
                actual, *expected,
                "resolve_consent({env:?}, {registry}) should be {expected}"
            );
        }
    }
    use std::sync::Mutex;

    // Panic hooks are process-global; this test binary's other tests must
    // not install or restore one while `a_real_panic_produces_one_event...`
    // is mid-swap.
    static HOOK_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// guards: a real panic reaching the installed hook, through the real
    /// `ContextIntegration`/`AttachStacktraceIntegration`/`redact_event`
    /// pipeline (not the bare-default client `with_captured_events` builds),
    /// must produce exactly one Sentry event carrying only the code
    /// location, OS context, and a redacted stack trace - never the panic
    /// payload text, which here quotes a skill name and a home path, and
    /// never the build-machine's absolute paths.
    #[test]
    fn a_real_panic_produces_one_event_with_the_location_only() {
        let _guard = HOOK_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_hook = std::panic::take_hook();
        install_panic_hook(Surface::Desktop);

        // This panic is the test fixture, not a mistake: it stands in for a
        // real panic payload that could quote a path or a skill name, which
        // `a_real_panic_produces_one_event_with_the_location_only` asserts
        // never reaches the captured event.
        #[allow(clippy::panic)]
        fn panic_with_a_sensitive_message() {
            panic!("secret /Users/someone/.claude/skills/my-skill");
        }
        let panic_line = line!() - 2;

        let dsn: sentry::types::Dsn = "https://examplePublicKey@o0.ingest.sentry.io/0"
            .parse()
            .expect("placeholder dsn parses");
        let mut opts = client_options(dsn, Surface::Desktop, "0.0.0", Consent::new(true));
        opts.integrations.push(Arc::new(
            sentry::integrations::contexts::ContextIntegration::default(),
        ));
        opts.integrations.push(Arc::new(
            sentry::integrations::backtrace::AttachStacktraceIntegration,
        ));

        let events = sentry::test::with_captured_events_options(
            || {
                let _ = std::panic::catch_unwind(panic_with_a_sensitive_message);
            },
            opts,
        );
        std::panic::set_hook(previous_hook);

        assert_eq!(
            events.len(),
            1,
            "expected exactly one captured event, got {events:?}"
        );
        let event = &events[0];
        assert_eq!(event.level, Level::Fatal);
        let message = event.message.clone().unwrap_or_default();
        let expected_prefix = format!("panicked at {}:{panic_line}:", file!());
        assert!(
            message.starts_with(&expected_prefix),
            "message {message:?} did not start with {expected_prefix:?}"
        );

        assert!(event.server_name.is_none(), "server_name must be redacted");
        assert!(event.user.is_none(), "user must be redacted");
        assert!(event.request.is_none(), "request must be redacted");
        assert!(event.breadcrumbs.is_empty(), "breadcrumbs must be redacted");
        assert!(event.extra.is_empty(), "extra must be redacted");

        let context_keys: std::collections::BTreeSet<&str> =
            event.contexts.keys().map(String::as_str).collect();
        let allowed: std::collections::BTreeSet<&str> =
            ["os", "device", "rust"].into_iter().collect();
        assert!(
            context_keys.is_subset(&allowed),
            "unexpected context keys {context_keys:?}"
        );
        assert!(context_keys.contains("os"), "expected an os context");

        let mut saw_function = false;
        let mut saw_stacktrace = false;
        for thread in &event.threads.values {
            if let Some(stacktrace) = &thread.stacktrace {
                saw_stacktrace = true;
                for frame in &stacktrace.frames {
                    assert!(
                        frame.abs_path.is_none(),
                        "a frame kept its absolute path: {frame:?}"
                    );
                    if frame.function.is_some() {
                        saw_function = true;
                    }
                }
            }
        }
        assert!(saw_stacktrace, "expected at least one thread stacktrace");
        assert!(
            saw_function,
            "expected at least one frame with a function name"
        );

        let serialized = serde_json::to_string(event).expect("serialize event");
        assert!(
            !serialized.contains("my-skill"),
            "a skill name leaked into the event: {serialized}"
        );
        if let Ok(home) = std::env::var("HOME") {
            assert!(
                !serialized.contains(&home),
                "the home path leaked into the event: {serialized}"
            );
        }
    }

    /// guards: `install_panic_hook` forgetting to flush - `capture_event`
    /// only queues the envelope, so a main-thread panic that never reaches
    /// `RunEvent::Exit` (tao resumes the unwind before `LoopDestroyed`)
    /// would otherwise lose the crash report. Binds a client whose
    /// transport is a fake recording each `send_envelope` and `flush` call
    /// in order directly on a hub for this test thread with `Hub::run` -
    /// `with_captured_events(_options)` replaces the transport with its own
    /// `TestTransport`, which is exactly the substitution this test needs to
    /// see through.
    #[test]
    fn a_panic_flushes_the_transport_before_the_hook_returns() {
        #[derive(Debug, PartialEq)]
        enum RecordedCall {
            Envelope,
            Flush(Duration),
        }

        #[derive(Default)]
        struct RecordingTransport {
            calls: Mutex<Vec<RecordedCall>>,
        }
        impl Transport for RecordingTransport {
            fn send_envelope(&self, _envelope: Envelope) {
                self.calls
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(RecordedCall::Envelope);
            }
            fn flush(&self, timeout: Duration) -> bool {
                self.calls
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(RecordedCall::Flush(timeout));
                true
            }
        }

        struct FixedTransportFactory(Arc<dyn Transport>);
        impl TransportFactory for FixedTransportFactory {
            fn create_transport_with_options(
                &self,
                _options: sentry::TransportOptions,
            ) -> Arc<dyn Transport> {
                self.0.clone()
            }
        }

        let _guard = HOOK_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let dsn: sentry::types::Dsn = "https://examplePublicKey@o0.ingest.sentry.io/0"
            .parse()
            .expect("placeholder dsn parses");
        let recording = Arc::new(RecordingTransport::default());
        let mut options = client_options(dsn, Surface::Desktop, "0.0.0", Consent::new(true));
        options.transport = Some(Arc::new(FixedTransportFactory(recording.clone())));
        let hub = Arc::new(sentry::Hub::new(
            Some(Arc::new(sentry::Client::with_options(options))),
            Arc::new(sentry::Scope::default()),
        ));

        // Builds via `panic_hook` directly rather than `install_panic_hook`:
        // that function's `Once` guards its one production caller (`init`)
        // and is process-global, so it would only ever fire for whichever
        // of this test and `a_real_panic_produces_one_event_with_the_location_only`
        // runs first, leaving the second test's hook uninstalled.
        let next_hook = std::panic::take_hook();
        std::panic::set_hook(panic_hook(Surface::Desktop, next_hook));

        #[allow(clippy::panic)]
        fn panic_fixture() {
            panic!("a panic the hook must flush before returning");
        }

        sentry::Hub::run(hub, || {
            let _ = std::panic::catch_unwind(panic_fixture);
        });
        // Discard our hook rather than an explicitly saved "previous" one:
        // `next_hook` was moved into `panic_hook` above, and dropping
        // whatever `take_hook` returns here resets to the same default
        // hook `next_hook` was under this lock.
        let _ = std::panic::take_hook();

        let calls = recording
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            *calls,
            vec![RecordedCall::Envelope, RecordedCall::Flush(SHUTDOWN_FLUSH)],
            "expected exactly one envelope then one flush with SHUTDOWN_FLUSH, got {calls:?}"
        );
    }

    /// guards: `redact_event` failing to strip a hostname, user, request,
    /// breadcrumbs, or a frame's absolute path - the safety net this
    /// function exists to be.
    #[test]
    fn before_send_strips_hostname_user_request_and_breadcrumbs() {
        let mut event = Event {
            server_name: Some("alices-mac.local".into()),
            user: Some(sentry::User {
                email: Some("alice@example.com".to_string()),
                ..Default::default()
            }),
            request: Some(sentry::protocol::Request {
                url: Some("https://example.com".parse().expect("url")),
                ..Default::default()
            }),
            ..Default::default()
        };
        event.breadcrumbs.values.push(sentry::Breadcrumb::default());
        event.extra.insert(
            "note".to_string(),
            serde_json::Value::String("secret".to_string()),
        );
        event.threads.values.push(sentry::protocol::Thread {
            stacktrace: Some(sentry::protocol::Stacktrace {
                frames: vec![sentry::protocol::Frame {
                    abs_path: Some("/Users/someone/.cargo/registry/x/lib.rs".into()),
                    filename: Some("lib.rs".into()),
                    function: Some("skill_studio_core::ops::install".into()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        });

        let redacted = redact_event(event, Surface::Desktop);

        assert!(redacted.server_name.is_none());
        assert!(redacted.user.is_none());
        assert!(redacted.request.is_none());
        assert!(redacted.breadcrumbs.is_empty());
        assert!(redacted.extra.is_empty());
        assert_eq!(
            redacted.tags.get("surface").map(String::as_str),
            Some("desktop")
        );
        let frame = &redacted.threads.values[0]
            .stacktrace
            .as_ref()
            .expect("stacktrace")
            .frames[0];
        assert!(frame.abs_path.is_none(), "abs_path must be stripped");
        assert_eq!(frame.filename.as_deref(), Some("lib.rs"));
        assert_eq!(
            frame.function.as_deref(),
            Some("skill_studio_core::ops::install")
        );
    }

    /// guards: `ConsentTransport` forwarding while off, or dropping while
    /// on - the single gate every envelope the client could send passes
    /// through.
    #[test]
    fn the_consent_transport_drops_envelopes_while_off_and_forwards_them_when_on() {
        #[derive(Default)]
        struct CountingTransport {
            sent: Mutex<usize>,
        }
        impl Transport for CountingTransport {
            fn send_envelope(&self, _envelope: Envelope) {
                *self
                    .sent
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            }
        }

        let inner = Arc::new(CountingTransport::default());
        let consent = Consent::new(false);
        let transport = ConsentTransport {
            inner: inner.clone(),
            consent: consent.clone(),
        };

        transport.send_envelope(Envelope::new());
        assert_eq!(
            *inner.sent.lock().expect("recorder"),
            0,
            "an envelope was forwarded while consent was off"
        );

        consent.set(true);
        transport.send_envelope(Envelope::new());
        assert_eq!(
            *inner.sent.lock().expect("recorder"),
            1,
            "no envelope was forwarded once consent was on"
        );
    }

    /// guards: `shutdown` failing to pass `SHUTDOWN_FLUSH` down to the
    /// transport, or swallowing the transport's own report of whether the
    /// queue drained. `options.shutdown_timeout` is set far above
    /// `SHUTDOWN_FLUSH` so this test cannot pass by accident: were `shutdown`
    /// to pass `None` (falling back to `ClientInitGuard::drop`'s own
    /// `close(None)`, which reads `shutdown_timeout`) the recorded timeout
    /// would be 30s, not `SHUTDOWN_FLUSH`. Only the *first* `flush`/`shutdown`
    /// call is recorded, because `close` always calls `flush` once itself and
    /// then `ClientInitGuard`'s `Drop` calls `close(None)` again on top of
    /// that - recording the last call would see the drop's `None`, not
    /// `shutdown`'s own request.
    #[test]
    fn shutdown_asks_the_transport_to_flush_within_two_seconds() {
        #[derive(Default)]
        struct RecordingTransport {
            first_timeout: Mutex<Option<Duration>>,
        }
        impl Transport for RecordingTransport {
            fn send_envelope(&self, _envelope: Envelope) {}
            fn flush(&self, timeout: Duration) -> bool {
                let mut seen = self
                    .first_timeout
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if seen.is_none() {
                    *seen = Some(timeout);
                }
                true
            }
            fn shutdown(&self, timeout: Duration) -> bool {
                self.flush(timeout)
            }
        }

        struct FixedTransportFactory(Arc<dyn Transport>);
        impl TransportFactory for FixedTransportFactory {
            fn create_transport_with_options(
                &self,
                _options: sentry::TransportOptions,
            ) -> Arc<dyn Transport> {
                self.0.clone()
            }
        }

        let recording = Arc::new(RecordingTransport::default());
        let mut options = ClientOptions::default();
        options.dsn = "https://examplePublicKey@o0.ingest.sentry.io/0"
            .parse()
            .ok();
        options.shutdown_timeout = Duration::from_secs(30);
        options.transport = Some(Arc::new(FixedTransportFactory(recording.clone())));
        let guard = TelemetryGuard(sentry::init(options));

        let result = shutdown(guard);

        assert!(result, "the fake transport reports a completed flush");
        assert_eq!(
            *recording.first_timeout.lock().expect("recorder"),
            Some(SHUTDOWN_FLUSH),
            "shutdown did not ask the transport to flush within SHUTDOWN_FLUSH"
        );
    }

    /// guards: the compile-time DSN losing to the run-time one, an empty
    /// string (an unset repo variable's `env:` expansion) counting as
    /// present, or a malformed DSN falling back to the other source instead
    /// of disabling telemetry.
    #[test]
    fn resolve_dsn_prefers_the_build_value_and_treats_empty_as_absent() {
        const VALID: &str = "https://examplePublicKey@o0.ingest.sentry.io/0";
        let cases: &[(Option<&str>, Option<&str>, bool)] = &[
            (Some(VALID), Some("not-a-dsn"), true),
            (None, Some(VALID), true),
            (Some(""), Some(VALID), true),
            (None, Some(""), false),
            (None, None, false),
            (Some("not-a-dsn"), None, false),
            (Some("not-a-dsn"), Some(VALID), false),
        ];
        for (build, env, expect_some) in cases {
            let actual = resolve_dsn(*build, env.map(str::to_string));
            assert_eq!(
                actual.is_some(),
                *expect_some,
                "resolve_dsn({build:?}, {env:?}) should resolve to a dsn: {expect_some}"
            );
        }
    }

    /// guards: `client_options` growing a default that carries PII (a real
    /// hostname, an unbounded breadcrumb trail) or losing the release
    /// string the version passed in.
    #[test]
    fn client_options_never_carry_pii_defaults() {
        let dsn: sentry::types::Dsn = "https://examplePublicKey@o0.ingest.sentry.io/0"
            .parse()
            .expect("placeholder dsn parses");
        let options = client_options(dsn, Surface::Desktop, "9.9.9", Consent::new(false));

        assert!(!options.send_default_pii);
        assert_eq!(options.server_name.as_deref(), Some("skill-studio"));
        assert_eq!(options.max_breadcrumbs, 0);
        assert_eq!(options.release.as_deref(), Some("skill-studio@9.9.9"));
    }

    /// guards: the message format growing text beyond the code location -
    /// the only thing the welcome screen says a crash report carries - or a
    /// panic with no location failing to produce a fixed message.
    #[test]
    fn a_crash_report_message_carries_only_the_code_location_or_names_the_extra_text() {
        let location = std::panic::Location::caller();
        let message = message_from_location(Some(location));
        assert_eq!(
            message,
            format!(
                "panicked at {}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            ),
            "the message carries more than the code location"
        );
        assert_eq!(
            message_from_location(None),
            "panicked at an unknown location",
            "a panic with no location must still produce a fixed message"
        );
    }
}
