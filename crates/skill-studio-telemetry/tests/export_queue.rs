use sentry_types::protocol::latest::{Envelope, Log, LogLevel};
use skill_studio_telemetry::*;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

fn envelope() -> SanitizedEnvelope {
    let mut input = Envelope::new();
    input.add_item(vec![Log {
        body: "skill.scan.finished".into(),
        level: LogLevel::Info,
        trace_id: None,
        timestamp: SystemTime::now(),
        severity_number: None,
        attributes: Default::default(),
    }]);
    sanitize_envelope(
        input,
        &TelemetryIdentity::new(TelemetrySurface::Cli, TelemetryEnvironment::Test, (1, 2, 3)),
    )
    .unwrap()
}

#[test]
fn drains_accepted_bytes_once_and_closes_admission() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    let exporter = TelemetryExporter::start(move |bytes| {
        sink.lock().unwrap().push(bytes.to_vec());
        Ok(())
    })
    .unwrap();
    assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Queued);
    assert_eq!(
        exporter.shutdown(Duration::from_secs(1)),
        FlushOutcome::Drained
    );
    assert_eq!(exporter.shutdown(Duration::ZERO), FlushOutcome::Drained);
    assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Closed);
    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert!(Envelope::from_slice(&captured[0]).is_ok());
    assert_eq!(
        exporter.stats(),
        ExportStats {
            accepted: 1,
            delivered: 1,
            failed: 0,
            dropped_full: 0,
            worker_panics: 0,
            dropped_closed: 1
        }
    );
}

#[test]
fn saturated_sink_bounds_admission_and_shutdown_then_can_finish_draining() {
    let (started, waiting) = mpsc::sync_channel(1);
    let (release, blocked) = mpsc::sync_channel(1);
    let mut first = true;
    let exporter = TelemetryExporter::start(move |_| {
        if first {
            first = false;
            started.send(()).unwrap();
            blocked.recv().unwrap();
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Queued);
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    for _ in 0..8 {
        assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Queued);
    }
    let start = Instant::now();
    for _ in 0..100 {
        assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Full);
    }
    assert!(start.elapsed() < Duration::from_secs(1));
    let start = Instant::now();
    assert_eq!(
        exporter.shutdown(Duration::from_millis(20)),
        FlushOutcome::TimedOut
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(exporter.stats().dropped_full, 100);
    assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Closed);
    release.send(()).unwrap();
    assert_eq!(
        exporter.shutdown(Duration::from_secs(1)),
        FlushOutcome::Drained
    );
    assert_eq!(exporter.stats().delivered, 9);
}

#[test]
fn failed_delivery_is_counted_without_retry_and_later_delivery_can_succeed() {
    let mut first = true;
    let exporter = TelemetryExporter::start(move |_| {
        if first {
            first = false;
            Err(ExportFailure)
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Queued);
    assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Queued);
    assert_eq!(
        exporter.shutdown(Duration::from_secs(1)),
        FlushOutcome::Failed
    );
    assert_eq!(exporter.stats().failed, 1);
    assert_eq!(exporter.stats().delivered, 1);
}

#[test]
fn worker_panic_is_reported_as_failure_not_a_successful_flush() {
    let exporter = TelemetryExporter::start(|_| panic!("fixture sink panic")).unwrap();
    assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Queued);
    assert_eq!(
        exporter.shutdown(Duration::from_secs(1)),
        FlushOutcome::Failed
    );
    assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Closed);
    assert_eq!(exporter.stats().worker_panics, 1);
}

#[test]
fn flush_drains_its_snapshot_without_closing_admission() {
    let exporter = TelemetryExporter::start(|_| Ok(())).unwrap();
    assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Queued);
    assert_eq!(
        exporter.flush(Duration::from_secs(1)),
        FlushOutcome::Drained
    );
    assert_eq!(exporter.try_enqueue(envelope()), EnqueueOutcome::Queued);
    assert_eq!(
        exporter.flush(Duration::from_secs(1)),
        FlushOutcome::Drained
    );
    assert_eq!(
        exporter.shutdown(Duration::from_secs(1)),
        FlushOutcome::Drained
    );
    assert_eq!(exporter.stats().delivered, 2);
}
