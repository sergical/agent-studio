// ============================================================================
// Skills Module - error_reporting
// Opt-in error reporting (unit 6.4): off by default, one switch in Settings
// (`error_reporting_enabled` in `~/.agents/skill-studio.json`, alongside the
// other settings in `skill_fork_registry`). When on, a panic is sanitized -
// see `skill_studio_core::report_sanitizer` - and queued on a
// `skill_studio_host::QueuedReportSink`; when off, the sink is never built,
// so a panic or a failed command makes no network call. `ReportingState` is
// the piece of Tauri-managed state the toggle command flips at runtime, so
// switching Settings takes effect without a restart.
// ============================================================================

use std::panic::PanicHookInfo;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use skill_studio_core::ports::ReportSink;
use skill_studio_core::report_sanitizer::{sanitize, RawException, RawReport, SensitiveContext};
use skill_studio_host::{HttpReportTransport, QueuedReportSink};
use tauri::Manager;

/// The switch and the sink it feeds, managed as Tauri state so
/// `set_error_reporting_enabled` can flip it live.
pub struct ReportingState {
    enabled: AtomicBool,
    sink: Arc<QueuedReportSink>,
}

impl ReportingState {
    /// Builds the state with the switch set to `enabled` (the persisted
    /// value, read at startup) and a sink built from
    /// `HttpReportTransport::from_env` - `None` when the endpoint's
    /// environment variable is unset, which is the normal case outside a
    /// release build. A `None` transport still builds a working sink: its
    /// `send` always fails, but `report`'s own gate on `enabled` means it is
    /// never called while the switch is off.
    pub fn new(enabled: bool) -> Self {
        let transport: Box<dyn skill_studio_host::ReportTransport> =
            match HttpReportTransport::from_env() {
                Some(transport) => Box::new(transport),
                None => Box::new(NoEndpointTransport),
            };
        Self::with_transport(enabled, transport)
    }

    /// Builds the state with an injected transport - the production `new`'s
    /// own path once it has picked one, and a test's path straight to a
    /// recording transport, so a test exercises the same gate `new` does
    /// rather than a parallel copy of it.
    pub fn with_transport(
        enabled: bool,
        transport: Box<dyn skill_studio_host::ReportTransport>,
    ) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            sink: Arc::new(QueuedReportSink::new(transport)),
        }
    }

    /// Flips the switch. Takes effect on the next failure; does not touch
    /// whatever is already queued.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Sanitizes and queues `raw` only when the switch is on. With the
    /// switch off this is a no-op that never reaches the sink - the
    /// property `reporting_off_makes_no_network_call` checks directly
    /// against a recording transport.
    pub fn maybe_report(&self, raw: &RawReport, sensitive: &SensitiveContext) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        self.sink.report(sanitize(raw, sensitive));
    }

    /// Sends every queued report now. Forwards to the sink; see
    /// `QueuedReportSink::flush`.
    pub fn flush(&self) -> Result<(), String> {
        self.sink.flush()
    }
}

/// A transport that refuses every send - stands in for `Option<Box<dyn
/// ReportTransport>>` so `ReportingState` always has a concrete sink to gate
/// on `enabled`, rather than branching on "is there a transport" at every
/// call site.
struct NoEndpointTransport;

impl skill_studio_host::ReportTransport for NoEndpointTransport {
    fn send(&self, _bytes: Vec<u8>) -> Result<(), String> {
        Err("no error-reporting endpoint is configured".to_string())
    }
}

/// The one `ReportingState` a panic hook can reach - `std::panic::set_hook`
/// takes a plain closure, not something with access to Tauri's managed
/// state, so the hook installed in `install_panic_hook` reads this instead.
static REPORTING_STATE: OnceLock<Arc<ReportingState>> = OnceLock::new();

/// Registers `state` as the target for `install_panic_hook`'s reports.
/// Called once, from `run()`'s `setup`.
pub fn set_global_state(state: Arc<ReportingState>) {
    let _ = REPORTING_STATE.set(state);
}

/// One frame's `Display` text is all `std::panic::Location` gives without a
/// backtrace crate; still enough for `sanitize` to reduce it to a file name.
fn frame_from_panic(
    info: &PanicHookInfo<'_>,
) -> Vec<skill_studio_core::report_sanitizer::RawFrame> {
    match info.location() {
        Some(location) => vec![skill_studio_core::report_sanitizer::RawFrame {
            absolute_path: Some(location.file().to_string()),
            function: "panic".to_string(),
            instruction_addr: None,
        }],
        None => Vec::new(),
    }
}

/// Installs a panic hook that reports through `REPORTING_STATE` (a no-op
/// until `set_global_state` runs) before running Rust's default hook, so a
/// panic still prints to stderr exactly as it did before this unit.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(state) = REPORTING_STATE.get() {
            let raw = RawReport {
                operation: None,
                dimensions: vec![],
                exceptions: vec![RawException {
                    message: info.to_string(),
                    frames: frame_from_panic(info),
                }],
            };
            let sensitive = SensitiveContext {
                home_dir: dirs::home_dir().map(|home| home.display().to_string()),
                skill_name: None,
                project_path: None,
            };
            state.maybe_report(&raw, &sensitive);
        }
        default_hook(info);
    }));
}

/// The saved switch, straight off disk - like `get_discovery_sources`, this
/// doesn't change anything, so it reads the registry directly.
#[tauri::command]
pub async fn get_error_reporting_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(
        &timing_app,
        "get_error_reporting_enabled",
        move || {
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            Ok(super::skill_fork_registry::read_fork_registry(&home)?.error_reporting_enabled)
        },
    )
    .await
}

/// Saves the switch and flips the live `ReportingState` so it takes effect
/// without a restart. Returns the saved value.
#[tauri::command]
pub async fn set_error_reporting_enabled(
    enabled: bool,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let timing_app = app.clone();
    crate::timing_log::time_command_blocking(
        &timing_app,
        "set_error_reporting_enabled",
        move || {
            let home = dirs::home_dir().ok_or("Could not find home directory")?;
            let mut registry = super::skill_fork_registry::read_fork_registry(&home)?;
            registry.error_reporting_enabled = enabled;
            super::skill_fork_registry::write_fork_registry(&home, &registry)?;
            app.state::<Arc<ReportingState>>().set_enabled(enabled);
            Ok(enabled)
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingTransport {
        sent: Mutex<Vec<Vec<u8>>>,
    }

    impl skill_studio_host::ReportTransport for RecordingTransport {
        fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
            self.sent.lock().expect("recorder").push(bytes);
            Ok(())
        }
    }

    struct Forwarding(Arc<RecordingTransport>);
    impl skill_studio_host::ReportTransport for Forwarding {
        fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
            self.0.send(bytes)
        }
    }

    /// A panic-shaped report quoting a home path, same shape
    /// `install_panic_hook` builds - so a test exercising `maybe_report`
    /// also exercises the sanitizer's redaction of it.
    fn sample_report() -> RawReport {
        RawReport {
            operation: Some("skill.scan".to_string()),
            dimensions: vec![],
            exceptions: vec![RawException {
                message: "panicked at /Users/alice/src/scan.rs:12".to_string(),
                frames: vec![],
            }],
        }
    }

    /// guards: the switch being off failing to stop `maybe_report` from
    /// queuing (and, on the next flush, sending) a report - the whole point
    /// of "off by default" is that nothing leaves the machine. Exercises
    /// the production `ReportingState` gate directly, not a copy of it.
    #[test]
    fn reporting_off_makes_no_network_call() {
        let recorder = Arc::new(RecordingTransport::default());
        let state = ReportingState::with_transport(false, Box::new(Forwarding(recorder.clone())));

        // A panic-shaped report, same as `install_panic_hook` would build.
        state.maybe_report(&sample_report(), &SensitiveContext::default());
        // A second report shaped like a failed command, to guard both paths
        // this unit's Sentry description names ("panics and command
        // failures").
        state.maybe_report(
            &RawReport {
                operation: Some("skill.park".to_string()),
                dimensions: vec![],
                exceptions: vec![],
            },
            &SensitiveContext::default(),
        );
        state.flush().expect("flush");

        let sent = recorder.sent.lock().expect("recorder");
        assert_eq!(
            sent.len(),
            0,
            "reporting made a network call while off: {sent:?}"
        );
    }

    /// guards: the switch being on failing to actually queue and send a
    /// report once flushed, or the report reaching the transport
    /// unsanitized - a home path in the panic message must not survive to
    /// what `send` receives.
    #[test]
    fn reporting_on_queues_a_sanitized_report_and_flush_sends_it_or_names_the_missing_send() {
        let recorder = Arc::new(RecordingTransport::default());
        let state = ReportingState::with_transport(true, Box::new(Forwarding(recorder.clone())));

        state.maybe_report(&sample_report(), &SensitiveContext::default());
        state.flush().expect("flush");

        let sent = recorder.sent.lock().expect("recorder");
        assert_eq!(
            sent.len(),
            1,
            "reporting was on but flush sent no report: {sent:?}"
        );
        let body = String::from_utf8(sent[0].clone()).expect("utf8 body");
        assert!(
            !body.contains("/Users/"),
            "a home path leaked into the bytes handed to the transport: {body}"
        );
    }
}
