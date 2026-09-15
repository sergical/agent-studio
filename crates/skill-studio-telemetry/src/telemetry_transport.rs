use crate::{
    sanitize_envelope, ExportFailure, ExportStats, FlushOutcome, TelemetryExporter,
    TelemetryIdentity,
};
use reqwest::header::{HeaderValue, CONTENT_TYPE};
use sentry_core::{Envelope, Transport};
use sentry_types::Dsn;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportSetupError {
    InsecureEndpoint,
    SecretDsn,
    InvalidHeader,
    WorkerUnavailable,
}

#[derive(Debug, Clone, Copy)]
pub struct TransportStats {
    pub rejected_envelopes: u64,
    pub export: ExportStats,
}

pub struct SentryTransport {
    exporter: TelemetryExporter,
    identity: TelemetryIdentity,
    rejected: AtomicU64,
}

impl SentryTransport {
    pub fn with_sink(
        identity: TelemetryIdentity,
        sink: impl FnMut(&[u8]) -> Result<(), ExportFailure> + Send + 'static,
    ) -> Result<Self, TransportSetupError> {
        Ok(Self {
            exporter: TelemetryExporter::start(sink)
                .map_err(|_| TransportSetupError::WorkerUnavailable)?,
            identity,
            rejected: AtomicU64::new(0),
        })
    }

    pub fn http(dsn: &Dsn, identity: TelemetryIdentity) -> Result<Self, TransportSetupError> {
        let endpoint = dsn.envelope_api_url();
        let loopback = endpoint
            .host_str()
            .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && loopback) {
            return Err(TransportSetupError::InsecureEndpoint);
        }
        if dsn.secret_key().is_some() {
            return Err(TransportSetupError::SecretDsn);
        }
        let auth = HeaderValue::from_str(&dsn.to_auth(Some("skill-studio/0.1.0")).to_string())
            .map_err(|_| TransportSetupError::InvalidHeader)?;
        let mut client = None;
        Self::with_sink(identity, move |bytes| {
            let client = client.get_or_insert_with(|| {
                reqwest::blocking::Client::builder()
                    .use_rustls_tls()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(Duration::from_millis(500))
                    .timeout(Duration::from_secs(2))
                    .build()
            });
            let client = client.as_ref().map_err(|_| ExportFailure)?;
            let response = client
                .post(endpoint.clone())
                .header("X-Sentry-Auth", auth.clone())
                .header(CONTENT_TYPE, "application/x-sentry-envelope")
                .body(bytes.to_vec())
                .send()
                .map_err(|_| ExportFailure)?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(ExportFailure)
            }
        })
    }

    pub fn stats(&self) -> TransportStats {
        TransportStats {
            rejected_envelopes: self.rejected.load(Ordering::Relaxed),
            export: self.exporter.stats(),
        }
    }
}

impl Transport for SentryTransport {
    fn send_envelope(&self, envelope: Envelope) {
        if let Some(safe) = sanitize_envelope(envelope, &self.identity) {
            self.exporter.try_enqueue(safe);
        } else {
            self.rejected.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn flush(&self, timeout: Duration) -> bool {
        self.exporter.flush(timeout) == FlushOutcome::Drained
    }
    fn shutdown(&self, timeout: Duration) -> bool {
        self.exporter.shutdown(timeout) == FlushOutcome::Drained
    }
}
