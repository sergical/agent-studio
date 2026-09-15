use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing_subscriber::Layer;

#[derive(Debug, Clone, Copy)]
pub enum ReadOperation {
    Scan,
    Doctor,
    Inventory,
    Snapshot,
    RepairPreview,
    History,
}
impl ReadOperation {
    fn name(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Doctor => "doctor",
            Self::Inventory => "inventory",
            Self::Snapshot => "snapshot",
            Self::RepairPreview => "repair-preview",
            Self::History => "history",
        }
    }
}

pub fn read_sentry_layer<S>() -> sentry_tracing::SentryLayer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    sentry_tracing::layer()
        .event_filter(|metadata| {
            if read_metadata_allowed(metadata) && metadata.is_event() {
                sentry_tracing::EventFilter::Log
            } else {
                sentry_tracing::EventFilter::Ignore
            }
        })
        .span_filter(|metadata| read_metadata_allowed(metadata) && metadata.is_span())
}

pub fn read_metadata_allowed(metadata: &tracing::Metadata<'_>) -> bool {
    *metadata.level() == tracing::Level::INFO
        && (matches!(
            (metadata.target(), metadata.name()),
            ("skill_studio_cli::read", "skill.read")
                | ("skill_studio_telemetry::read", "skill.read")
                | ("skill_studio_core::skill_service", "skill.scan")
                | ("skill_studio_core::skill_service", "skill.scan.phase")
                | ("skill_studio_core::skill_service", "skill.repair.preview")
                | ("skill_studio_desktop::history", "skill.history")
        ) || (matches!(
            metadata.target(),
            "skill_studio_core::skill_service"
                | "skill_studio_desktop::history"
                | "skill_studio_desktop::refresh"
        ) && metadata.is_event()))
}

pub struct ReadContext {
    dispatcher: tracing::Dispatch,
    hub: Option<Arc<sentry_core::Hub>>,
    operation: ReadOperation,
    surface: crate::TelemetrySurface,
    remote_trace: Option<String>,
}
impl ReadContext {
    pub fn capture(surface: crate::TelemetrySurface, operation: ReadOperation) -> Self {
        let current = sentry_core::Hub::current();
        Self {
            dispatcher: tracing::dispatcher::get_default(Clone::clone),
            hub: current
                .client()
                .map(|_| Arc::new(sentry_core::Hub::new_from_top(current))),
            operation,
            surface,
            remote_trace: None,
        }
    }
    pub fn capture_ipc(operation: ReadOperation, header: Option<&str>) -> Self {
        let mut context = Self::capture(crate::TelemetrySurface::Desktop, operation);
        context.remote_trace = header
            .filter(|header| valid_trace_header(header))
            .map(str::to_owned);
        if let Some(hub) = &context.hub {
            hub.configure_scope(|scope| scope.set_span(None));
        }
        context
    }
    pub fn run<T>(self, work: impl FnOnce() -> T) -> T {
        tracing::dispatcher::with_default(&self.dispatcher, || {
            let run = || {
                static NEXT_READ: AtomicU64 = AtomicU64::new(1);
                let span = tracing::info_span!(target: "skill_studio_telemetry::read", "skill.read", adapter = self.surface.name(), operation = self.operation.name(), sentry.trace = self.remote_trace.as_deref().unwrap_or(""), read_sequence = NEXT_READ.fetch_add(1, Ordering::Relaxed));
                let _entered = span.enter();
                work()
            };
            if let Some(hub) = self.hub {
                sentry_core::Hub::run(hub, run)
            } else {
                run()
            }
        })
    }
}

pub struct ScanMetricsLayer;
impl<S: tracing::Subscriber> Layer<S> for ScanMetricsLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if !matches!(
            event.metadata().target(),
            "skill_studio_core::skill_service" | "skill_studio_desktop::history"
        ) {
            return;
        }
        let mut fields = ScanFields::default();
        event.record(&mut fields);
        let (count_name, duration_name) = match fields.finished {
            Some(ReadCompletion::Scan) => ("skill.scan.count", "skill.scan.duration"),
            Some(ReadCompletion::RepairPreview) => (
                "skill.repair.preview.count",
                "skill.repair.preview.duration",
            ),
            Some(ReadCompletion::History) => ("skill.history.count", "skill.history.duration"),
            None => return,
        };
        let Some(outcome) = fields.outcome else {
            return;
        };
        sentry_core::metrics::counter(count_name, 1)
            .attribute("outcome", outcome)
            .capture();
        if let Some(duration) = fields.duration {
            sentry_core::metrics::distribution(duration_name, duration)
                .unit("millisecond")
                .attribute("outcome", outcome)
                .capture();
        }
    }
}
enum ReadCompletion {
    Scan,
    RepairPreview,
    History,
}
#[derive(Default)]
struct ScanFields {
    finished: Option<ReadCompletion>,
    outcome: Option<&'static str>,
    duration: Option<f64>,
}
impl tracing::field::Visit for ScanFields {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "outcome" {
            self.outcome = match value {
                "complete" => Some("complete"),
                "partial" => Some("partial"),
                "failed" => Some("failed"),
                "cancelled" => Some("cancelled"),
                _ => None,
            };
        }
    }
    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        if field.name() == "duration_ms"
            && value.is_finite()
            && (0.0..=86_400_000.0).contains(&value)
        {
            self.duration = Some(value);
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.finished = match format!("{value:?}").as_str() {
                "skill.scan.finished" => Some(ReadCompletion::Scan),
                "skill.repair.preview.finished" => Some(ReadCompletion::RepairPreview),
                "skill.history.finished" => Some(ReadCompletion::History),
                _ => None,
            };
        }
    }
}

fn valid_trace_header(header: &str) -> bool {
    let bytes = header.as_bytes();
    if bytes.len() != 51
        || bytes[32] != b'-'
        || bytes[49] != b'-'
        || !matches!(bytes[50], b'0' | b'1')
    {
        return false;
    }
    [&bytes[..32], &bytes[33..49]].iter().all(|id| {
        id.iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
            && id.iter().any(|byte| *byte != b'0')
    })
}

#[cfg(test)]
mod tests {
    use super::valid_trace_header;

    #[test]
    fn trace_header_accepts_only_bounded_nonzero_ids_and_sampling_bit() {
        let valid = "0123456789abcdef0123456789abcdef-0123456789abcdef-1";
        assert!(valid_trace_header(valid));
        assert!(valid_trace_header(&valid.replace("-1", "-0")));
        for invalid in [
            "",
            "PRIVATE_SENTINEL",
            "0123456789abcdef0123456789abcdef-0123456789abcdef",
            "00000000000000000000000000000000-0123456789abcdef-1",
            "0123456789abcdef0123456789abcdef-0000000000000000-1",
            "0123456789abcdef0123456789abcdef-0123456789abcdef-2",
            "0123456789ABCDEF0123456789abcdef-0123456789abcdef-1",
        ] {
            assert!(!valid_trace_header(invalid));
        }
        assert!(!valid_trace_header(&format!("{valid}\nPRIVATE_SENTINEL")));
        assert!(!valid_trace_header(&"x".repeat(100_000)));
    }
}
