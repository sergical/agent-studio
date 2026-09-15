use sentry::protocol::{EnvelopeItem, ItemContainer};
use sentry::{Client, ClientOptions, Envelope, Hub, Scope};
use skill_studio_telemetry::{
    sanitize_envelope, TelemetryEnvironment, TelemetryIdentity, TelemetrySurface,
};
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::prelude::*;

#[test]
fn desktop_refresh_logs_reach_export_without_private_fields_or_unknown_causes() {
    let transport = sentry::test::TestTransport::new();
    let client = Arc::new(Client::from(
        ClientOptions::new()
            .dsn("https://public@example.invalid/1")
            .transport(transport.clone())
            .default_integrations(false),
    ));
    let hub = Arc::new(Hub::new(Some(client.clone()), Arc::new(Scope::default())));
    Hub::run(hub, || {
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(skill_studio_telemetry::read_sentry_layer()),
            || {
                tracing::info!(target: "skill_studio_desktop::refresh", cause = "watcher", generation = 7_u64, path = "PRIVATE_PATH", "skill.refresh.requested");
                tracing::info!(target: "skill_studio_desktop::refresh", cause = "PRIVATE_CAUSE", "skill.refresh.requested");
                tracing::info!(target: "skill_studio_desktop::refresh", extent = "named", duration_ms = 12.5, "skill.refresh.lock_acquired");
                tracing::info!(target: "skill_studio_desktop::refresh", outcome = "complete", generation = 7_u64, requested_generation = 8_u64, duration_ms = 123.5, "skill.refresh.finished");
                tracing::info!(target: "skill_studio_desktop::refresh", "PRIVATE_MESSAGE");
            },
        );
    });
    assert!(client.flush(Some(Duration::from_secs(1))));
    let identity = TelemetryIdentity::new(
        TelemetrySurface::Desktop,
        TelemetryEnvironment::Test,
        (1, 2, 3),
    );
    let mut logs = Vec::new();
    for envelope in transport.fetch_and_clear_envelopes() {
        if let Some(safe) = sanitize_envelope(envelope, &identity) {
            let bytes = safe.into_bytes();
            let text = String::from_utf8(bytes.clone()).unwrap();
            assert!(!text.contains("PRIVATE_"));
            for item in Envelope::from_slice(&bytes).unwrap().into_items() {
                if let EnvelopeItem::ItemContainer(ItemContainer::Logs(values)) = item {
                    logs.extend(values);
                }
            }
        }
    }
    assert_eq!(logs.len(), 4);
    assert_eq!(logs[0].attributes["cause"].0, "watcher");
    assert!(!logs[1].attributes.contains_key("cause"));
    assert_eq!(logs[2].attributes["extent"].0, "named");
    assert_eq!(logs[2].attributes["duration_ms"].0, 12.5);
    assert_eq!(logs[3].attributes["outcome"].0, "complete");
    assert_eq!(logs[3].attributes["duration_ms"].0, 123.5);
    for log in logs {
        assert!(!log.attributes.contains_key("generation"));
        assert!(!log.attributes.contains_key("requested_generation"));
        assert_eq!(log.attributes["surface"].0, "desktop");
    }
}
