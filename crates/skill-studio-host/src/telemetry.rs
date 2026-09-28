//! Sentry crash-report client (PR1, unit 6.4): one event per Rust panic,
//! carrying only the code location, the app version, OS/CPU context, and a
//! `surface` tag - gated end-to-end by a live [`Consent`] flag so nothing
//! leaves the machine while the user's switch is off. See
//! `docs/spec-headless-performance-observability.md`.

use std::panic::PanicHookInfo;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

use sentry::protocol::{Event, Map};
use sentry::transports::ReqwestHttpTransportOptions;
use sentry::{ClientInitGuard, ClientOptions, Envelope, Level, Transport, TransportFactory};

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
/// guard. Returns `None` - and installs nothing - when no DSN is available,
/// which is every build until `SKILL_STUDIO_SENTRY_DSN` is set. Never
/// panics: `resolve_dsn` and the DSN parse both fail closed.
pub fn init(
    surface: Surface,
    app_version: &'static str,
    consent: Consent,
) -> Option<TelemetryGuard> {
    let dsn = resolve_dsn(
        option_env!("SKILL_STUDIO_SENTRY_DSN"),
        std::env::var(DSN_ENV_VAR).ok(),
    )?;
    install_panic_hook(surface);
    let guard = sentry::init(client_options(&dsn, surface, app_version, consent));
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
/// CI expands to `""`, not an omitted `env:` entry.
fn resolve_dsn(build: Option<&str>, process_env: Option<String>) -> Option<String> {
    fn non_empty(value: String) -> Option<String> {
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    }
    build
        .map(str::to_string)
        .and_then(non_empty)
        .or_else(|| process_env.and_then(non_empty))
}

/// Builds the options `init` hands to `sentry::init`. Kept apart from `init`
/// so a test can inspect the values without starting a real client.
fn client_options(
    dsn: &str,
    surface: Surface,
    app_version: &'static str,
    consent: Consent,
) -> ClientOptions {
    // `ClientOptions` is `#[non_exhaustive]`, so a struct-literal
    // (even with `..Default::default()`) doesn't compile outside its own
    // crate - build the default and mutate the fields this module cares
    // about instead.
    let mut options = ClientOptions::default();
    // A malformed DSN (only reachable through the run-time fallback - the
    // compile-time one is set once, by the release workflow) fails closed
    // to a disabled client rather than panicking `init`.
    options.dsn = dsn.parse().ok();
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

/// `before_send`'s body, factored out so a test can call it directly rather
/// than reaching in through a closure. A safety net: the panic hook already
/// builds a clean event, but this is the one gate every event - including
/// PR2's transactions - passes through before it leaves the process.
fn redact_event(mut event: Event<'static>, surface: Surface) -> Event<'static> {
    event.server_name = None;
    event.user = None;
    event.request = None;
    event.breadcrumbs = Default::default();
    event.extra = Map::new();
    event
        .contexts
        .retain(|key, _| matches!(key.as_str(), "os" | "device" | "rust" | "runtime"));
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
    fn create_transport_with_options(
        &self,
        options: sentry::TransportOptions,
    ) -> Arc<dyn Transport> {
        let inner: Arc<dyn Transport> =
            Arc::new(ReqwestHttpTransportOptions::from(options).build());
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

static PANIC_HOOK_INSTALLED: Once = Once::new();

/// Installs a panic hook that reports `panic_event` through
/// `sentry::capture_event` (a no-op with no client bound) before running the
/// previous hook, so a panic still prints to stderr exactly as it did
/// without this unit. Installs at most once per process - a second call
/// (there is only one caller, `init`) is a no-op rather than double-wrapping
/// the hook.
fn install_panic_hook(surface: Surface) {
    PANIC_HOOK_INSTALLED.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            sentry::capture_event(panic_event(info, surface));
            default_hook(info);
        }));
    });
}

#[cfg(all(test, feature = "telemetry"))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Panic hooks are process-global; this test binary's other tests must
    // not install or restore one while `a_real_panic_produces_one_event...`
    // is mid-swap.
    static HOOK_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// guards: a real panic reaching the installed hook must produce
    /// exactly one Sentry event carrying only the code location - never the
    /// panic payload text, which here quotes a skill name and a home path.
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

        let events = sentry::test::with_captured_events(|| {
            let _ = std::panic::catch_unwind(panic_with_a_sensitive_message);
        });
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
        let serialized = serde_json::to_string(event).expect("serialize event");
        assert!(
            !serialized.contains("my-skill"),
            "a skill name leaked into the event: {serialized}"
        );
        assert!(
            !serialized.contains("/Users/"),
            "a home path leaked into the event: {serialized}"
        );
    }

    /// guards: `redact_event` failing to strip a hostname, user, request, or
    /// breadcrumbs an integration attached upstream of it - the safety net
    /// this function exists to be.
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
    /// queue drained.
    #[test]
    fn shutdown_asks_the_transport_to_flush_within_two_seconds() {
        #[derive(Default)]
        struct RecordingTransport {
            seen_timeout: Mutex<Option<Duration>>,
        }
        impl Transport for RecordingTransport {
            fn send_envelope(&self, _envelope: Envelope) {}
            fn flush(&self, timeout: Duration) -> bool {
                *self
                    .seen_timeout
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(timeout);
                false
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
        options.shutdown_timeout = SHUTDOWN_FLUSH;
        options.transport = Some(Arc::new(FixedTransportFactory(recording.clone())));
        let guard = TelemetryGuard(sentry::init(options));

        let result = shutdown(guard);

        assert!(!result, "the fake transport reports an incomplete flush");
        assert_eq!(
            *recording.seen_timeout.lock().expect("recorder"),
            Some(SHUTDOWN_FLUSH),
            "shutdown did not ask the transport to flush within SHUTDOWN_FLUSH"
        );
    }

    /// guards: the compile-time DSN losing to the run-time one, or an empty
    /// string (an unset repo variable's `env:` expansion) counting as
    /// present.
    #[test]
    fn resolve_dsn_prefers_the_build_value_and_treats_empty_as_absent() {
        let cases: &[(Option<&str>, Option<&str>, Option<&str>)] = &[
            (Some("build-dsn"), Some("env-dsn"), Some("build-dsn")),
            (None, Some("env-dsn"), Some("env-dsn")),
            (Some(""), Some("env-dsn"), Some("env-dsn")),
            (None, Some(""), None),
            (None, None, None),
        ];
        for (build, env, expected) in cases {
            let actual = resolve_dsn(*build, env.map(str::to_string));
            assert_eq!(
                actual.as_deref(),
                *expected,
                "resolve_dsn({build:?}, {env:?}) should be {expected:?}"
            );
        }
    }

    /// guards: `client_options` growing a default that carries PII (a real
    /// hostname, an unbounded breadcrumb trail) or losing the release
    /// string the version passed in.
    #[test]
    fn client_options_never_carry_pii_defaults() {
        let options = client_options(
            "https://examplePublicKey@o0.ingest.sentry.io/0",
            Surface::Desktop,
            "9.9.9",
            Consent::new(false),
        );

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
