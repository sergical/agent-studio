use sentry_types::protocol::latest::*;
use serde_json::json;
use skill_studio_telemetry::{
    sanitize_envelope, TelemetryEnvironment, TelemetryIdentity, TelemetrySurface,
};
use std::time::SystemTime;

const TRACE: &str = "12345678901234567890123456789012";
const SPAN: &str = "1234567890123456";
const PRIVATE: &str = "PRIVATE_SECRET /Users/private/skill.md";

fn identity() -> TelemetryIdentity {
    TelemetryIdentity::new(TelemetrySurface::Cli, TelemetryEnvironment::Test, (1, 2, 3))
}
fn encoded(envelope: &Envelope) -> String {
    let mut bytes = Vec::new();
    envelope.to_writer(&mut bytes).unwrap();
    String::from_utf8(bytes).unwrap()
}
fn trace() -> serde_json::Value {
    json!({"type":"trace", "trace_id":TRACE, "span_id":SPAN, "op":"skill.scan", "description":PRIVATE,
        "data":{"outcome":"complete","duration_ms":12.5,"skill_count":3,"private":PRIVATE,"project_count":{"private":PRIVATE}}})
}
fn dirty_transaction() -> Transaction<'static> {
    serde_json::from_value(json!({"transaction":"skill.scan", "release":PRIVATE,"environment":PRIVATE,
        "user":{"username":PRIVATE},"server_name":PRIVATE,"tags":{"private":PRIVATE},"extra":{"private":PRIVATE},
        "request":{"url":"https://private.invalid/PRIVATE_SECRET"},
        "contexts":{"trace":trace(),"private":{"value":PRIVATE}},
        "spans":[{"trace_id":TRACE,"span_id":SPAN,"op":"skill.scan","description":PRIVATE,"data":{"outcome":"complete","private":PRIVATE}}]
    })).unwrap()
}
fn dirty_log() -> Log {
    Log {
        level: LogLevel::Info,
        body: "skill.scan.finished".into(),
        trace_id: Some(TRACE.parse().unwrap()),
        timestamp: SystemTime::now(),
        severity_number: None,
        attributes: [
            ("outcome".into(), LogAttribute("complete".into())),
            ("extent".into(), LogAttribute(PRIVATE.into())),
            (
                "duration_ms".into(),
                LogAttribute(json!({"secret":PRIVATE})),
            ),
            ("private".into(), LogAttribute(PRIVATE.into())),
        ]
        .into(),
    }
}
fn metric() -> Metric {
    Metric {
        r#type: MetricType::Counter,
        name: "skill.scan.count".into(),
        value: 1.0,
        timestamp: SystemTime::now(),
        trace_id: TRACE.parse().unwrap(),
        span_id: Some(SPAN.parse().unwrap()),
        unit: None,
        attributes: [
            ("outcome".into(), LogAttribute("complete".into())),
            ("run_id".into(), LogAttribute(PRIVATE.into())),
        ]
        .into(),
    }
}

#[test]
fn all_supported_items_remove_private_data_and_preserve_correlation() {
    let event: Event<'static> = serde_json::from_value(json!({"level":"error","message":PRIVATE,"fingerprint":[PRIVATE],"culprit":PRIVATE,
        "logger":PRIVATE,"server_name":PRIVATE,"release":PRIVATE,"environment":PRIVATE,"user":{"username":PRIVATE},
        "contexts":{"trace":trace()},"extra":{"private":PRIVATE},"tags":{"private":PRIVATE},
        "exception":{"values":[{"type":PRIVATE,"value":PRIVATE,"stacktrace":{"frames":[{"filename":PRIVATE,"context_line":PRIVATE}]}}]},
        "breadcrumbs":{"values":[{"message":PRIVATE}]}})).unwrap();
    let mut input = Envelope::new().with_headers(
        EnvelopeHeaders::new().with_trace(
            DynamicSamplingContext::new()
                .with_trace_id(TRACE.parse().unwrap())
                .with_public_key(PRIVATE.to_string()),
        ),
    );
    input.add_item(event);
    input.add_item(dirty_transaction());
    input.add_item(vec![dirty_log()]);
    input.add_item(vec![metric()]);
    assert!(encoded(&input).contains("PRIVATE_SECRET"));
    let output = sanitize_envelope(input, &identity())
        .map(|safe| Envelope::from_slice(&safe.into_bytes()).unwrap())
        .unwrap();
    let bytes = encoded(&output);
    assert!(!bytes.contains("PRIVATE_SECRET"), "{bytes}");
    assert!(!bytes.contains("/Users/"), "{bytes}");
    assert!(!bytes.contains("public_key"), "{bytes}");
    assert!(bytes.contains("skill-studio@1.2.3"));
    assert_eq!(output.items().count(), 4);
    for item in output.items() {
        match item {
            EnvelopeItem::Event(event) => {
                assert_eq!(
                    event.message.as_deref(),
                    Some("Skill Studio runtime failure")
                );
                assert!(event.exception.values.is_empty());
                assert_eq!(
                    serde_json::to_value(&event.contexts).unwrap()["trace"]["trace_id"],
                    TRACE
                );
            }
            EnvelopeItem::Transaction(transaction) => {
                assert_eq!(transaction.spans.len(), 1);
                assert_eq!(
                    transaction.spans[0].description.as_deref(),
                    Some("skill.scan")
                );
                let trace = serde_json::to_value(&transaction.contexts).unwrap();
                assert_eq!(trace["trace"]["data"]["duration_ms"], 12.5);
                assert!(trace["trace"]["data"].get("project_count").is_none());
            }
            EnvelopeItem::ItemContainer(ItemContainer::Logs(logs)) => {
                assert_eq!(logs[0].trace_id.unwrap().to_string(), TRACE);
                assert!(!logs[0].attributes.contains_key("duration_ms"));
                assert!(!logs[0].attributes.contains_key("extent"));
            }
            EnvelopeItem::ItemContainer(ItemContainer::Metrics(metrics)) => {
                assert_eq!(metrics[0].trace_id.to_string(), TRACE);
                assert_eq!(metrics[0].span_id.unwrap().to_string(), SPAN);
                assert_eq!(metrics[0].value, 1.0);
                assert!(!metrics[0].attributes.contains_key("run_id"));
            }
            _ => panic!("unexpected output item"),
        }
    }
}

#[test]
fn raw_envelopes_unknown_operations_and_expected_conflicts_are_dropped() {
    assert!(sanitize_envelope(
        Envelope::from_bytes_raw(b"{}\nPRIVATE_SECRET".to_vec()).unwrap(),
        &identity()
    )
    .is_none());
    let mut envelope = Envelope::new();
    let mut transaction = dirty_transaction();
    transaction.name = Some(PRIVATE.into());
    envelope.add_item(transaction);
    let mut log = dirty_log();
    log.body = PRIVATE.into();
    envelope.add_item(vec![log]);
    for code in [
        "cancelled",
        "scope_busy",
        "scope_deadline_exceeded",
        "scope_changed",
    ] {
        let mut event = Event {
            level: Level::Error,
            ..Default::default()
        };
        event.tags.insert("error_code".into(), code.into());
        envelope.add_item(event);
    }
    assert!(sanitize_envelope(envelope, &identity()).is_none());
}

#[test]
fn metrics_reject_unknown_names_units_types_and_invalid_values() {
    let mut invalid = Vec::new();
    let mut value = metric();
    value.name = PRIVATE.into();
    invalid.push(value);
    let mut value = metric();
    value.unit = Some(Unit::Other(PRIVATE.into()));
    invalid.push(value);
    for value in [-1.0, 0.0, 2.0, f64::NAN, f64::INFINITY] {
        let mut sample = metric();
        sample.value = value;
        invalid.push(sample);
    }
    let mut value = metric();
    value.r#type = MetricType::Gauge;
    invalid.push(value);
    let mut envelope = Envelope::new();
    envelope.add_item(invalid);
    assert!(sanitize_envelope(envelope, &identity()).is_none());
}

#[test]
fn bounded_collections_and_final_size_limit_control_output() {
    let mut one = Envelope::new();
    one.add_item(vec![dirty_log(); 1000]);
    let output =
        Envelope::from_slice(&sanitize_envelope(one, &identity()).unwrap().into_bytes()).unwrap();
    let EnvelopeItem::ItemContainer(ItemContainer::Logs(logs)) = output.items().next().unwrap()
    else {
        panic!()
    };
    assert_eq!(logs.len(), 128);
    let mut large = Envelope::new();
    for _ in 0..64 {
        large.add_item(vec![dirty_log(); 128]);
    }
    assert!(sanitize_envelope(large, &identity()).is_none());
}

#[test]
fn duration_and_inventory_metrics_preserve_units_and_reject_invalid_ranges() {
    for (name, kind, unit, value, accepted) in [
        (
            "skill.scan.duration",
            MetricType::Distribution,
            Some(Unit::Millisecond),
            12.5,
            true,
        ),
        (
            "skill.scan.duration",
            MetricType::Distribution,
            Some(Unit::Second),
            12.5,
            false,
        ),
        (
            "skill.scan.duration",
            MetricType::Distribution,
            Some(Unit::Millisecond),
            86_400_001.0,
            false,
        ),
        (
            "skill.scan.inventory_count",
            MetricType::Gauge,
            None,
            100.0,
            true,
        ),
        (
            "skill.scan.inventory_count",
            MetricType::Gauge,
            None,
            0.5,
            false,
        ),
    ] {
        let mut sample = metric();
        sample.name = name.into();
        sample.r#type = kind;
        sample.unit = unit.clone();
        sample.value = value;
        let mut envelope = Envelope::new();
        envelope.add_item(vec![sample]);
        let output = sanitize_envelope(envelope, &identity());
        assert_eq!(output.is_some(), accepted, "{name} {value}");
        if let Some(output) = output {
            let envelope = Envelope::from_slice(&output.into_bytes()).unwrap();
            let EnvelopeItem::ItemContainer(ItemContainer::Metrics(values)) =
                envelope.items().next().unwrap()
            else {
                panic!()
            };
            assert_eq!(values[0].value, value);
            assert_eq!(values[0].unit, unit);
        }
    }
}
