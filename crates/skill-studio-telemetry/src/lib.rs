mod telemetry_envelope;
mod telemetry_identity;

pub use telemetry_envelope::{sanitize_envelope, SanitizedEnvelope};
pub use telemetry_identity::{TelemetryEnvironment, TelemetryIdentity, TelemetrySurface};

mod telemetry_export;
pub use telemetry_export::{
    EnqueueOutcome, ExportFailure, ExportStats, FlushOutcome, TelemetryExporter,
};

mod telemetry_transport;
pub use telemetry_transport::{SentryTransport, TransportSetupError, TransportStats};

mod telemetry_read;
mod telemetry_session;
pub use telemetry_read::{
    read_metadata_allowed, read_sentry_layer, ReadContext, ReadOperation, ScanMetricsLayer,
};
pub use telemetry_session::{
    session_from_environment, session_from_environment_with_defaults, SentrySession,
    SessionDefaults, SessionSetupError,
};
