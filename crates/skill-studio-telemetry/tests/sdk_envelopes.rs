#[cfg(test)]
mod tests {
    use sentry::protocol::{EnvelopeItem, ItemContainer};
    use sentry::{Client, ClientOptions, Envelope, Hub, Scope};
    use skill_studio_telemetry::{
        sanitize_envelope, TelemetryEnvironment, TelemetryIdentity, TelemetrySurface,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tracing_subscriber::prelude::*;

    fn capture(options: ClientOptions, run: impl FnOnce()) -> Vec<Envelope> {
        let transport = sentry::test::TestTransport::new();
        let client = Arc::new(Client::from(
            options
                .dsn("https://public@example.invalid/1")
                .transport(transport.clone())
                .default_integrations(false)
                .traces_sample_rate(1.0),
        ));
        let hub = Arc::new(Hub::new(Some(client.clone()), Arc::new(Scope::default())));
        Hub::run(hub, run);
        assert!(client.flush(Some(Duration::from_secs(1))));
        let identity =
            TelemetryIdentity::new(TelemetrySurface::Cli, TelemetryEnvironment::Test, (1, 2, 3));
        transport
            .fetch_and_clear_envelopes()
            .into_iter()
            .filter_map(|envelope| sanitize_envelope(envelope, &identity))
            .map(|safe| {
                let envelope = Envelope::from_slice(&safe.into_bytes()).unwrap();
                let mut bytes = Vec::new();
                envelope.to_writer(&mut bytes).unwrap();
                assert!(!String::from_utf8(bytes).unwrap().contains("PRIVATE_"));
                envelope
            })
            .collect()
    }

    #[test]
    fn final_boundary_removes_scope_data_and_client_reports_after_sdk_callbacks() {
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = calls.clone();
        let options = ClientOptions::new().before_send(move |_| {
            callback_calls.fetch_add(1, Ordering::Relaxed);
            None
        });
        let envelopes = capture(options, || {
            sentry::configure_scope(|scope| {
                scope.set_tag("repository", "PRIVATE_REPOSITORY_SENTINEL")
            });
            sentry::capture_message("PRIVATE_ERROR_SENTINEL", sentry::Level::Error);
            sentry::start_transaction(sentry::TransactionContext::new("skill.scan", "skill.scan"))
                .finish();
        });
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let items: Vec<_> = envelopes
            .into_iter()
            .flat_map(Envelope::into_items)
            .collect();
        assert_eq!(items.len(), 1);
        let transaction = items
            .iter()
            .find_map(|item| match item {
                EnvelopeItem::Transaction(transaction) => Some(transaction),
                _ => None,
            })
            .expect("transaction must bypass before_send");
        assert!(!transaction.tags.contains_key("repository"));
    }

    #[test]
    fn tracing_layer_correlates_error_log_metric_and_transaction() {
        let envelopes = capture(ClientOptions::default(), || {
            let layer = sentry::integrations::tracing::layer()
                .event_filter(|metadata| {
                    if metadata.target() == "skill_studio_core::skill_service" {
                        sentry::integrations::tracing::EventFilter::Log
                    } else {
                        sentry::integrations::tracing::EventFilter::Ignore
                    }
                })
                .span_filter(|metadata| matches!(metadata.name(), "skill.scan" | "skill.read"));
            tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
                let read = tracing::info_span!("skill.read");
                let _read_entered = read.enter();
                let scan = tracing::info_span!(target: "skill_studio_core::skill_service", "skill.scan", outcome = "complete");
                let _entered = scan.enter();
                tracing::info!(target: "skill_studio_core::skill_service", "skill.scan.finished");
                sentry::metrics::counter("skill.scan.count", 1).capture();
                sentry::capture_message("fixture failure", sentry::Level::Error);
            });
        });
        let mut error = None;
        let mut transaction = None;
        let mut log = None;
        let mut metric = None;
        for item in envelopes.into_iter().flat_map(Envelope::into_items) {
            match item {
                EnvelopeItem::Event(value) => error = Some(serde_json::to_value(value).unwrap()),
                EnvelopeItem::Transaction(value) => {
                    transaction = Some(serde_json::to_value(value).unwrap())
                }
                EnvelopeItem::ItemContainer(ItemContainer::Logs(values)) => {
                    assert_eq!(values.len(), 1);
                    log = Some(serde_json::to_value(&values[0]).unwrap());
                }
                EnvelopeItem::ItemContainer(ItemContainer::Metrics(values)) => {
                    assert_eq!(values.len(), 1);
                    metric = Some(serde_json::to_value(&values[0]).unwrap());
                }
                _ => panic!("unexpected envelope item"),
            }
        }
        let error = error.expect("error");
        let transaction = transaction.expect("transaction");
        let log = log.expect("log");
        let metric = metric.expect("metric");
        assert_eq!(
            transaction["spans"].as_array().unwrap().len(),
            1,
            "{transaction}"
        );
        assert_eq!(transaction["spans"][0]["op"], "skill.scan");
        let trace = &transaction["contexts"]["trace"]["trace_id"];
        assert!(trace.is_string(), "{transaction}");
        assert_eq!(&error["contexts"]["trace"]["trace_id"], trace, "{error}");
        assert_eq!(&log["trace_id"], trace, "{log}");
        assert_eq!(&metric["trace_id"], trace, "{metric}");
    }
    #[test]
    fn scan_phase_survives_the_sdk_and_final_privacy_boundary() {
        let envelopes = capture(ClientOptions::default(), || {
            let layer = skill_studio_telemetry::read_sentry_layer();
            tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
                let _scan =
                    tracing::info_span!(target: "skill_studio_core::skill_service", "skill.scan")
                        .entered();
                let _phase = tracing::info_span!(target: "skill_studio_core::skill_service",
                    "skill.scan.phase", phase = "discovery_enumeration", outcome = "complete",
                    duration_ms = 12.0, path = "PRIVATE_PATH")
                .entered();
                tracing::info!(target: "skill_studio_core::skill_service",
                    phase = "discovery_enumeration", outcome = "complete", duration_ms = 12.0,
                    path = "PRIVATE_PATH", "skill.scan.phase.finished");
            });
        });
        let mut saw_span = false;
        let mut saw_log = false;
        for item in envelopes.into_iter().flat_map(Envelope::into_items) {
            match item {
                EnvelopeItem::Transaction(transaction) => {
                    let value = serde_json::to_value(transaction).unwrap();
                    for span in value["spans"].as_array().unwrap() {
                        if span["op"] == "skill.scan.phase" {
                            assert_eq!(span["data"]["phase"], "discovery_enumeration");
                            assert_eq!(span["data"]["duration_ms"], 12.0);
                            saw_span = true;
                        }
                    }
                }
                EnvelopeItem::ItemContainer(ItemContainer::Logs(logs)) => {
                    for log in logs {
                        if log.body == "skill.scan.phase.finished" {
                            assert_eq!(log.attributes["phase"].0, "discovery_enumeration");
                            saw_log = true;
                        }
                    }
                }
                _ => {}
            }
        }
        assert!(saw_span && saw_log);
    }
}
