use sentry::{Client, ClientOptions, Hub, Scope};
use skill_studio_telemetry::*;
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;
use tracing_subscriber::prelude::*;

#[test]
fn preview_contexts_export_private_free_correlated_signals() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let output = records.clone();
    let transport = Arc::new(
        SentryTransport::with_sink(
            TelemetryIdentity::new(
                TelemetrySurface::Desktop,
                TelemetryEnvironment::Test,
                (1, 2, 3),
            ),
            move |bytes| {
                output
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
    let barrier = Arc::new(Barrier::new(2));
    Hub::run(hub, || {
        tracing::subscriber::with_default(
            tracing_subscriber::registry()
                .with(read_sentry_layer())
                .with(ScanMetricsLayer),
            || {
                let mut workers = Vec::new();
                for operation in [ReadOperation::RepairPreview, ReadOperation::RepairPreview] {
                    let context = ReadContext::capture(TelemetrySurface::Desktop, operation);
                    let barrier = barrier.clone();
                    workers.push(std::thread::spawn(move || context.run(|| {
                    let span = tracing::info_span!(target: "skill_studio_core::skill_service", "skill.repair.preview");
                    let _entered = span.enter();
                    sentry::configure_scope(|scope| scope.set_tag("private", "PRIVATE_SENTINEL"));
                    barrier.wait();
                    tracing::info!(target: "skill_studio_core::skill_service", outcome = "failed", error_code = "unsupported_repair", private_document = "PRIVATE_SENTINEL", duration_ms = 1.5, "skill.repair.preview.finished");
                })));
                }
                for worker in workers {
                    worker.join().unwrap();
                }
            },
        );
    });
    assert!(client.close(Some(Duration::from_secs(1))));
    let records = records.lock().unwrap();
    let mut transactions = Vec::new();
    let mut logs = Vec::new();
    let mut metrics = Vec::new();
    for encoded in records.iter() {
        assert!(!encoded.contains("PRIVATE_SENTINEL"));
        let mut lines = encoded.lines();
        lines.next();
        while let Some(header) = lines.next() {
            let header: serde_json::Value = serde_json::from_str(header).unwrap();
            let body: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
            match header["type"].as_str().unwrap() {
                "transaction" => transactions.push(body),
                "log" => logs.extend(body["items"].as_array().unwrap().iter().cloned()),
                "trace_metric" => metrics.extend(body["items"].as_array().unwrap().iter().cloned()),
                _ => panic!("unexpected item"),
            }
        }
    }
    assert_eq!(transactions.len(), 2);
    assert_eq!(logs.len(), 2);
    assert_eq!(metrics.len(), 4);
    for log in &logs {
        assert_eq!(log["body"], "skill.repair.preview.finished");
    }
    for metric in &metrics {
        assert!(matches!(
            metric["name"].as_str(),
            Some("skill.repair.preview.count" | "skill.repair.preview.duration")
        ));
    }
    for transaction in &transactions {
        assert_eq!(transaction["spans"][0]["op"], "skill.repair.preview");
    }
    assert_ne!(
        transactions[0]["contexts"]["trace"]["trace_id"],
        transactions[1]["contexts"]["trace"]["trace_id"]
    );
    for transaction in transactions {
        let trace = &transaction["contexts"]["trace"]["trace_id"];
        assert_eq!(transaction["spans"].as_array().unwrap().len(), 1);
        assert_eq!(
            logs.iter().filter(|log| &log["trace_id"] == trace).count(),
            1
        );
        assert_eq!(
            metrics
                .iter()
                .filter(|metric| &metric["trace_id"] == trace)
                .count(),
            2
        );
    }
}
