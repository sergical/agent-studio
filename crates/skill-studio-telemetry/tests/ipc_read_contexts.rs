use sentry::{Client, ClientOptions, Hub, Scope};
use skill_studio_telemetry::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing_subscriber::prelude::*;

#[test]
fn ipc_reads_continue_valid_headers_and_ignore_invalid_headers_without_scope_bleed() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let records = output.clone();
    let transport = Arc::new(
        SentryTransport::with_sink(
            TelemetryIdentity::new(
                TelemetrySurface::Desktop,
                TelemetryEnvironment::Test,
                (1, 2, 3),
            ),
            move |bytes| {
                records
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(bytes.to_vec()).unwrap());
                Ok(())
            },
        )
        .unwrap(),
    );
    let client = Arc::new(Client::from(
        ClientOptions::new()
            .dsn("https://public@example.invalid/1")
            .default_integrations(false)
            .traces_sample_rate(1.0)
            .transport(transport),
    ));
    let hub = Arc::new(Hub::new(Some(client.clone()), Arc::new(Scope::default())));
    Hub::run(hub, || {
        tracing::subscriber::with_default(
            tracing_subscriber::registry()
                .with(read_sentry_layer())
                .with(ScanMetricsLayer),
            || {
                let unrelated = sentry::start_transaction(sentry::TransactionContext::new(
                    "PRIVATE_SENTINEL",
                    "private",
                ));
                sentry::configure_scope(|scope| scope.set_span(Some(unrelated.clone().into())));
                let context = ReadContext::capture_ipc(
                    ReadOperation::Inventory,
                    Some("0123456789abcdef0123456789abcdef-0123456789abcdef-1"),
                );
                std::thread::spawn(move || context.run(|| {
                ReadContext::capture(TelemetrySurface::Desktop, ReadOperation::Scan).run(|| {
                    let scan = tracing::info_span!(target: "skill_studio_core::skill_service", "skill.scan");
                    let _entered = scan.enter();
                    tracing::info!(target: "skill_studio_core::skill_service", outcome = "complete", duration_ms = 1.5, "skill.scan.finished");
                });
            })).join().unwrap();
                assert_eq!(
                    ReadContext::capture_ipc(ReadOperation::Snapshot, Some("PRIVATE_SENTINEL"))
                        .run(|| 42),
                    42
                );
                assert_eq!(
                    ReadContext::capture_ipc(
                        ReadOperation::Snapshot,
                        Some("abcdef0123456789abcdef0123456789-abcdef0123456789-0")
                    )
                    .run(|| 43),
                    43
                );
                sentry::configure_scope(|scope| scope.set_span(None));
                unrelated.finish();
            },
        )
    });
    assert!(client.close(Some(Duration::from_secs(1))));
    let records = output.lock().unwrap();
    let mut transactions = Vec::new();
    let mut signals = Vec::new();
    for encoded in records.iter() {
        assert!(!encoded.contains("PRIVATE_SENTINEL"));
        let mut lines = encoded.lines();
        lines.next();
        while let Some(header) = lines.next() {
            let header: serde_json::Value = serde_json::from_str(header).unwrap();
            let body: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
            match header["type"].as_str().unwrap() {
                "transaction" => transactions.push(body),
                "log" | "trace_metric" => {
                    signals.extend(body["items"].as_array().unwrap().iter().cloned())
                }
                _ => panic!("unexpected item"),
            }
        }
    }
    assert_eq!(transactions.len(), 2);
    let remote = transactions
        .iter()
        .find(|tx| tx["contexts"]["trace"]["trace_id"] == "0123456789abcdef0123456789abcdef")
        .unwrap();
    assert_eq!(
        remote["contexts"]["trace"]["parent_span_id"],
        "0123456789abcdef"
    );
    let spans = remote["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 2);
    let read = spans
        .iter()
        .find(|span| span["description"] == "skill.read")
        .unwrap();
    let scan = spans
        .iter()
        .find(|span| span["description"] == "skill.scan")
        .unwrap();
    assert_eq!(
        read["parent_span_id"],
        remote["contexts"]["trace"]["span_id"]
    );
    assert_eq!(scan["parent_span_id"], read["span_id"]);
    assert_eq!(signals.len(), 3);
    assert!(signals
        .iter()
        .all(|signal| signal["trace_id"] == "0123456789abcdef0123456789abcdef"));
    let fresh = transactions
        .iter()
        .find(|tx| tx["contexts"]["trace"]["trace_id"] != "0123456789abcdef0123456789abcdef")
        .unwrap();
    assert!(fresh["contexts"]["trace"]["parent_span_id"].is_null());
    assert_ne!(
        fresh["contexts"]["trace"]["trace_id"],
        "abcdef0123456789abcdef0123456789"
    );
}
