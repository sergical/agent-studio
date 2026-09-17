//! [`EventSink`] adapters: discard, or print one JSON line per notice. Also
//! the [`ReportSink`] transport for opt-in error reporting (unit 6.4): a
//! bounded queue in front of a [`ReportTransport`], and the real network
//! transport behind it.

use std::collections::VecDeque;
use std::sync::Mutex;

use skill_studio_core::ports::{CoreNotice, EventSink, ReportSink};
use skill_studio_core::report_sanitizer::SanitizedEnvelope;

/// Discards every notice. The default when nobody is listening.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSink;

impl EventSink for NoopSink {
    fn notify(&self, _notice: CoreNotice) {}
}

/// Prints one JSON line per notice to stderr, for CLI and debug builds.
#[derive(Debug, Default, Clone, Copy)]
pub struct StderrSink;

impl EventSink for StderrSink {
    fn notify(&self, notice: CoreNotice) {
        match serde_json::to_string(&notice) {
            Ok(line) => eprintln!("{line}"),
            Err(e) => eprintln!("{{\"notice\":\"unserializable\",\"error\":{e:?}}}"),
        }
    }
}

/// Sends one sanitized envelope's bytes to wherever error reports go.
/// [`QueuedReportSink`] is the only caller; a test swaps in a recording
/// implementation instead of [`HttpReportTransport`] so no test opens a
/// socket.
pub trait ReportTransport: Send + Sync {
    /// Sends `bytes` - a JSON-serialized [`SanitizedEnvelope`] - or
    /// describes why it couldn't.
    fn send(&self, bytes: Vec<u8>) -> Result<(), String>;
}

/// Envelopes queued for [`QueuedReportSink::flush`] beyond this count drop
/// the oldest one first, so a burst of failures can't grow the queue
/// without bound while reporting is on.
pub const QUEUE_CAPACITY: usize = 8;

/// A [`ReportSink`] that queues envelopes in memory and sends them through
/// a [`ReportTransport`] on [`Self::flush`]. `report` never blocks on the
/// network: it only pushes onto the queue.
///
/// Flushing on a timer or at shutdown is the adapter's job (the desktop, in
/// this unit) - `QueuedReportSink` exposes `flush` for that caller to drive,
/// rather than spawning its own thread, so a test can call `flush` directly
/// with no timing involved.
pub struct QueuedReportSink {
    queue: Mutex<VecDeque<SanitizedEnvelope>>,
    transport: Box<dyn ReportTransport>,
}

impl QueuedReportSink {
    /// Builds an empty queue in front of `transport`.
    pub fn new(transport: Box<dyn ReportTransport>) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            transport,
        }
    }

    /// Sends every queued envelope now, in the order they were queued.
    /// Stops and reports the first transport error rather than losing the
    /// rest of the queue silently; the caller decides whether to retry.
    pub fn flush(&self) -> Result<(), String> {
        let mut queue = self.queue.lock().expect("report queue poisoned");
        while let Some(envelope) = queue.pop_front() {
            let bytes = serde_json::to_vec(&envelope)
                .map_err(|e| format!("failed to serialize a report envelope: {e}"))?;
            self.transport.send(bytes)?;
        }
        Ok(())
    }
}

impl ReportSink for QueuedReportSink {
    fn report(&self, envelope: SanitizedEnvelope) {
        let mut queue = self.queue.lock().expect("report queue poisoned");
        if queue.len() >= QUEUE_CAPACITY {
            queue.pop_front();
        }
        queue.push_back(envelope);
    }
}

/// Name of the environment variable holding the error-reporting endpoint at
/// runtime - the Rust project's DSN from `docs/sentry-project-mapping.md`.
/// Read once, at [`HttpReportTransport::from_env`]; never written into the
/// repository, and unset in every test.
pub const REPORT_ENDPOINT_ENV: &str = "SKILL_STUDIO_SENTRY_DSN";

/// Posts a sanitized envelope's bytes to a fixed HTTP endpoint. The thin
/// adapter behind [`ReportTransport`]: it does not speak the Sentry
/// envelope protocol, retry, or batch - `QueuedReportSink` already batches,
/// and a hanging endpoint's timeout is unit 6.4's shutdown-flush follow-up.
pub struct HttpReportTransport {
    endpoint: String,
    client: reqwest::blocking::Client,
}

impl HttpReportTransport {
    /// Reads [`REPORT_ENDPOINT_ENV`]; `None` when it's unset or empty, which
    /// is how the switch being off keeps the app from ever building a
    /// transport that could make a network call.
    pub fn from_env() -> Option<Self> {
        let endpoint = std::env::var(REPORT_ENDPOINT_ENV).ok()?;
        if endpoint.is_empty() {
            return None;
        }
        Some(Self {
            endpoint,
            client: reqwest::blocking::Client::new(),
        })
    }
}

impl ReportTransport for HttpReportTransport {
    fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.client
            .post(&self.endpoint)
            .header("content-type", "application/json")
            .body(bytes)
            .send()
            .map_err(|e| format!("failed to send an error report: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    #[test]
    fn noop_sink_accepts_every_notice_without_panicking() {
        NoopSink.notify(CoreNotice::Recovered { events: Vec::new() });
    }

    /// A transport that records every envelope it was asked to send,
    /// instead of opening a socket - the fixture behind the desktop's
    /// `reporting_off_makes_no_network_call` test.
    #[derive(Default)]
    pub struct RecordingTransport {
        pub sent: StdMutex<Vec<Vec<u8>>>,
    }

    impl ReportTransport for RecordingTransport {
        fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
            self.sent
                .lock()
                .expect("recording transport poisoned")
                .push(bytes);
            Ok(())
        }
    }

    fn sample_envelope() -> SanitizedEnvelope {
        SanitizedEnvelope {
            operation: Some("skill.scan".to_string()),
            dimensions: vec![],
            exceptions: vec![],
        }
    }

    /// guards: a queue that forgets to bound itself, growing without limit
    /// while error reporting is on during a burst of failures.
    #[test]
    fn queued_report_sink_drops_the_oldest_envelope_past_queue_capacity() {
        let recorder = std::sync::Arc::new(RecordingTransport::default());
        struct ForwardingTransport(std::sync::Arc<RecordingTransport>);
        impl ReportTransport for ForwardingTransport {
            fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
                self.0.send(bytes)
            }
        }
        let sink = QueuedReportSink::new(Box::new(ForwardingTransport(recorder.clone())));

        for i in 0..(QUEUE_CAPACITY + 3) {
            let mut envelope = sample_envelope();
            envelope.operation = Some(format!("skill.scan#{i}"));
            sink.report(envelope);
        }
        sink.flush().expect("flush");

        let sent = recorder.sent.lock().expect("recorder");
        assert_eq!(
            sent.len(),
            QUEUE_CAPACITY,
            "queue did not bound itself at {QUEUE_CAPACITY}: sent {} envelopes",
            sent.len()
        );
    }
}
