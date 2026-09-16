use crate::TelemetryIdentity;
use sentry_types::protocol::latest::*;
use serde_json::Value;
use std::io::{self, Write};

const MAX_ITEMS: usize = 64;
const MAX_CHILDREN: usize = 128;
const MAX_EXCEPTIONS: usize = 8;
const MAX_ENCODED_BYTES: usize = 256 * 1024;

pub struct SanitizedEnvelope(Vec<u8>);

impl SanitizedEnvelope {
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

pub fn sanitize_envelope(
    input: Envelope,
    identity: &TelemetryIdentity,
) -> Option<SanitizedEnvelope> {
    let mut output = Envelope::new();
    for item in input.into_items().take(MAX_ITEMS) {
        match item {
            EnvelopeItem::Event(event) => {
                if let Some(event) = sanitize_event(*event, identity) {
                    output.add_item(event);
                }
            }
            EnvelopeItem::Transaction(transaction) => {
                if let Some(transaction) = sanitize_transaction(*transaction, identity) {
                    output.add_item(transaction);
                }
            }
            EnvelopeItem::ItemContainer(ItemContainer::Logs(logs)) => {
                let logs: Vec<_> = logs
                    .into_iter()
                    .take(MAX_CHILDREN)
                    .filter_map(|log| sanitize_log(log, identity))
                    .collect();
                if !logs.is_empty() {
                    output.add_item(logs);
                }
            }
            EnvelopeItem::ItemContainer(ItemContainer::Metrics(metrics)) => {
                let metrics: Vec<_> = metrics
                    .into_iter()
                    .take(MAX_CHILDREN)
                    .filter_map(|metric| sanitize_metric(metric, identity))
                    .collect();
                if !metrics.is_empty() {
                    output.add_item(metrics);
                }
            }
            _ => {}
        }
    }
    output.items().next()?;
    let mut encoded = ByteBudget(Vec::new());
    output.to_writer(&mut encoded).ok()?;
    Some(SanitizedEnvelope(encoded.0))
}

struct ByteBudget(Vec<u8>);
impl Write for ByteBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_ENCODED_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("telemetry size limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn operation(value: &str) -> Option<&'static str> {
    match value {
        "skill.scan" => Some("skill.scan"),
        "skill.scan.phase" => Some("skill.scan.phase"),
        "skill.read" => Some("skill.read"),
        "skill.repair.preview" => Some("skill.repair.preview"),
        "skill.history" => Some("skill.history"),
        _ => None,
    }
}

fn dimension(key: &str, value: &Value) -> Option<Value> {
    let value = value.as_str()?;
    let valid = match key {
        "outcome" => matches!(
            value,
            "running" | "complete" | "partial" | "failed" | "cancelled"
        ),
        "cause" => matches!(
            value,
            "mutation" | "manual" | "projects" | "watcher" | "named_reconciliation" | "retry"
        ),
        "extent" => matches!(value, "full" | "named"),
        "phase" => matches!(
            value,
            "initial_coordination"
                | "ownership_enumeration"
                | "ownership_coordination"
                | "discovery_enumeration"
                | "discovery_materialization"
                | "ownership_materialization"
                | "assembly"
                | "final_validation"
                | "ledger_collection"
        ),
        "operation" => matches!(
            value,
            "scan" | "doctor" | "inventory" | "snapshot" | "repair-preview" | "history"
        ),
        "error_code" => matches!(
            value,
            "none"
                | "invalid_scope"
                | "invalid_names"
                | "scope_busy"
                | "scope_deadline_exceeded"
                | "cancelled"
                | "invalid_timeout"
                | "filesystem_root_required"
                | "scope_unavailable"
                | "scope_changed"
                | "coordination_capacity"
                | "incomplete_inventory"
                | "target_not_found"
                | "ambiguous_target"
                | "document_unavailable"
                | "unsupported_repair"
        ),
        _ => false,
    };
    valid.then(|| Value::String(value.into()))
}

const DATA_FIELDS: [&str; 12] = [
    "outcome",
    "cause",
    "extent",
    "phase",
    "operation",
    "error_code",
    "selected_count",
    "project_count",
    "skill_count",
    "ledger_only_count",
    "discovery_issue_count",
    "duration_ms",
];

fn data_value(key: &str, value: &Value) -> Option<Value> {
    match key {
        "selected_count"
        | "project_count"
        | "skill_count"
        | "ledger_only_count"
        | "discovery_issue_count" => value
            .as_u64()
            .filter(|value| *value <= u32::MAX as u64)
            .map(Value::from),
        "duration_ms" => value
            .as_f64()
            .filter(|value| value.is_finite() && (0.0..=86_400_000.0).contains(value))
            .map(Value::from),
        _ => dimension(key, value),
    }
}

fn safe_data(data: &Map<String, Value>) -> Map<String, Value> {
    DATA_FIELDS
        .into_iter()
        .filter_map(|key| {
            data.get(key)
                .and_then(|value| data_value(key, value))
                .map(|value| (key.into(), value))
        })
        .collect()
}

fn trace_context(mut contexts: Map<String, Context>) -> Map<String, Context> {
    let mut output = Map::new();
    if let Some(Context::Trace(trace)) = contexts.remove("trace") {
        output.insert(
            "trace".into(),
            TraceContext {
                trace_id: trace.trace_id,
                span_id: trace.span_id,
                parent_span_id: trace.parent_span_id,
                op: trace.op.as_deref().and_then(operation).map(str::to_owned),
                status: trace.status,
                data: safe_data(&trace.data),
                ..Default::default()
            }
            .into(),
        );
    }
    output
}

fn sanitize_transaction(
    input: Transaction<'static>,
    identity: &TelemetryIdentity,
) -> Option<Transaction<'static>> {
    let name = operation(input.name.as_deref()?)?;
    Some(Transaction {
        event_id: input.event_id,
        name: Some(name.into()),
        release: Some(identity.release.clone().into()),
        environment: Some(identity.environment.into()),
        start_timestamp: input.start_timestamp,
        timestamp: input.timestamp,
        contexts: trace_context(input.contexts),
        tags: [("surface".into(), identity.surface.into())].into(),
        spans: input
            .spans
            .into_iter()
            .take(MAX_CHILDREN)
            .filter_map(|span| {
                let op = span
                    .op
                    .as_deref()
                    .and_then(operation)
                    .or_else(|| span.description.as_deref().and_then(operation))?;
                Some(Span {
                    trace_id: span.trace_id,
                    span_id: span.span_id,
                    parent_span_id: span.parent_span_id,
                    op: Some(op.into()),
                    description: Some(op.into()),
                    status: span.status,
                    start_timestamp: span.start_timestamp,
                    timestamp: span.timestamp,
                    data: safe_data(&span.data),
                    ..Default::default()
                })
            })
            .collect(),
        ..Default::default()
    })
}

fn sanitize_stacktrace(mut input: Stacktrace) -> Option<Stacktrace> {
    let keep_from = input.frames.len().saturating_sub(MAX_CHILDREN);
    let frames: Vec<_> = input
        .frames
        .drain(keep_from..)
        .filter_map(|frame| {
            if !matches!(frame.addr_mode.as_deref(), None | Some("abs")) {
                return None;
            }
            let address = frame.instruction_addr.filter(|address| address.0 != 0)?;
            Some(Frame {
                instruction_addr: Some(address),
                ..Default::default()
            })
        })
        .collect();
    (!frames.is_empty()).then_some(Stacktrace {
        frames,
        ..Default::default()
    })
}

fn sanitize_native_diagnostics(input: &mut Event<'static>) {
    input.stacktrace = input.stacktrace.take().and_then(sanitize_stacktrace);
    if input.stacktrace.is_none() {
        input.stacktrace = input
            .threads
            .values
            .iter_mut()
            .take(MAX_CHILDREN)
            .filter(|thread| thread.current)
            .find_map(|thread| thread.stacktrace.take().and_then(sanitize_stacktrace));
    }
    let keep_from = input.exception.values.len().saturating_sub(MAX_EXCEPTIONS);
    input.exception.values = input
        .exception
        .values
        .drain(keep_from..)
        .filter_map(|exception| {
            sanitize_stacktrace(exception.stacktrace?).map(|stacktrace| Exception {
                ty: "SkillStudioError".into(),
                stacktrace: Some(stacktrace),
                ..Default::default()
            })
        })
        .collect();
    let addresses: Vec<_> = input
        .stacktrace
        .iter()
        .chain(
            input
                .exception
                .values
                .iter()
                .filter_map(|exception| exception.stacktrace.as_ref()),
        )
        .flat_map(|stack| {
            stack
                .frames
                .iter()
                .filter_map(|frame| frame.instruction_addr)
        })
        .collect();
    let images = std::mem::take(&mut input.debug_meta.to_mut().images)
        .into_iter()
        .take(MAX_CHILDREN)
        .filter_map(|image| {
            let (base, size) = match &image {
                DebugImage::Apple(image) => (image.image_addr.0, image.image_size),
                DebugImage::Symbolic(image) => (image.image_addr.0, image.image_size),
                _ => return None,
            };
            let end = base.checked_add(size)?;
            if !addresses
                .iter()
                .any(|address| (base..end).contains(&address.0))
            {
                return None;
            }
            match image {
                DebugImage::Apple(image) => Some(DebugImage::Apple(AppleDebugImage {
                    name: "native-image".into(),
                    uuid: image.uuid,
                    image_addr: image.image_addr,
                    image_size: image.image_size,
                    image_vmaddr: image.image_vmaddr,
                    arch: None,
                    cpu_type: None,
                    cpu_subtype: None,
                })),
                DebugImage::Symbolic(image) => Some(DebugImage::Symbolic(SymbolicDebugImage {
                    name: "native-image".into(),
                    id: image.id,
                    image_addr: image.image_addr,
                    image_size: image.image_size,
                    image_vmaddr: image.image_vmaddr,
                    arch: None,
                    code_id: None,
                    debug_file: None,
                })),
                _ => None,
            }
        })
        .collect();
    input.debug_meta = std::borrow::Cow::Owned(DebugMeta {
        images,
        ..Default::default()
    });
}

fn sanitize_event(
    mut input: Event<'static>,
    identity: &TelemetryIdentity,
) -> Option<Event<'static>> {
    if !matches!(input.level, Level::Error | Level::Fatal) {
        return None;
    }
    if input.tags.get("error_code").is_some_and(|code| {
        matches!(
            code.as_str(),
            "cancelled" | "scope_busy" | "scope_deadline_exceeded" | "scope_changed"
        )
    }) {
        return None;
    }
    if identity.surface == "desktop" {
        sanitize_native_diagnostics(&mut input);
    } else {
        input.stacktrace = None;
        input.exception = Default::default();
        input.debug_meta = Default::default();
    }
    Some(Event {
        platform: if input.stacktrace.is_some() || !input.exception.values.is_empty() {
            "native".into()
        } else {
            Event::default().platform
        },
        stacktrace: input.stacktrace,
        exception: input.exception,
        debug_meta: input.debug_meta,
        event_id: input.event_id,
        timestamp: input.timestamp,
        level: input.level,
        message: Some("Skill Studio runtime failure".into()),
        release: Some(identity.release.clone().into()),
        environment: Some(identity.environment.into()),
        transaction: input
            .transaction
            .as_deref()
            .and_then(operation)
            .map(str::to_owned),
        contexts: trace_context(input.contexts),
        tags: [("surface".into(), identity.surface.into())].into(),
        ..Default::default()
    })
}

fn attributes(
    input: &Map<String, LogAttribute>,
    identity: &TelemetryIdentity,
) -> Map<String, LogAttribute> {
    let mut output: Map<_, _> = DATA_FIELDS
        .into_iter()
        .filter_map(|key| {
            input
                .get(key)
                .and_then(|value| data_value(key, &value.0))
                .map(|value| (key.into(), LogAttribute(value)))
        })
        .collect();
    output.insert("surface".into(), LogAttribute(identity.surface.into()));
    output.insert(
        "sentry.release".into(),
        LogAttribute(identity.release.clone().into()),
    );
    output.insert(
        "sentry.environment".into(),
        LogAttribute(identity.environment.into()),
    );
    output
}

fn sanitize_log(input: Log, identity: &TelemetryIdentity) -> Option<Log> {
    if !matches!(
        input.body.as_str(),
        "skill.scan.finished"
            | "skill.scan.phase.finished"
            | "skill.repair.preview.finished"
            | "skill.history.finished"
            | "skill.refresh.requested"
            | "skill.refresh.finished"
            | "skill.refresh.lock_acquired"
    ) {
        return None;
    }
    Some(Log {
        body: input.body,
        level: LogLevel::Info,
        trace_id: input.trace_id,
        timestamp: input.timestamp,
        severity_number: None,
        attributes: attributes(&input.attributes, identity),
    })
}

fn sanitize_metric(input: Metric, identity: &TelemetryIdentity) -> Option<Metric> {
    let valid = match (input.name.as_ref(), input.r#type, input.unit.as_ref()) {
        (
            "skill.scan.count" | "skill.repair.preview.count" | "skill.history.count",
            MetricType::Counter,
            None,
        ) => input.value == 1.0,
        (
            "skill.scan.duration" | "skill.repair.preview.duration" | "skill.history.duration",
            MetricType::Distribution,
            Some(Unit::Millisecond),
        ) => (0.0..=86_400_000.0).contains(&input.value),
        ("skill.scan.inventory_count", MetricType::Gauge, None) => {
            (0.0..=u32::MAX as f64).contains(&input.value) && input.value.fract() == 0.0
        }
        _ => false,
    };
    if !valid || !input.value.is_finite() {
        return None;
    }
    let mut attributes: Map<_, _> = [
        ("surface".into(), LogAttribute(identity.surface.into())),
        (
            "sentry.release".into(),
            LogAttribute(identity.release.clone().into()),
        ),
        (
            "sentry.environment".into(),
            LogAttribute(identity.environment.into()),
        ),
    ]
    .into();
    for key in ["outcome", "extent", "operation", "error_code"] {
        if let Some(value) = input
            .attributes
            .get(key)
            .and_then(|value| dimension(key, &value.0))
        {
            attributes.insert(key.into(), LogAttribute(value));
        }
    }
    Some(Metric {
        r#type: input.r#type,
        name: input.name,
        value: input.value,
        timestamp: input.timestamp,
        trace_id: input.trace_id,
        span_id: input.span_id,
        unit: input.unit,
        attributes,
    })
}
