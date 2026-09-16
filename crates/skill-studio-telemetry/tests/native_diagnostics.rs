use sentry_types::protocol::latest::*;
use serde_json::{json, Value};
use skill_studio_telemetry::{
    sanitize_envelope, TelemetryEnvironment, TelemetryIdentity, TelemetrySurface,
};

const PRIVATE: &str = "PRIVATE /Users/private/secret.rs";
const ID: &str = "01234567-89ab-cdef-0123-456789abcdef";

fn sanitize(value: Value, surface: TelemetrySurface) -> (Event<'static>, String) {
    let mut input = Envelope::new();
    input.add_item(serde_json::from_value::<Event<'static>>(value).unwrap());
    let bytes = sanitize_envelope(
        input,
        &TelemetryIdentity::new(surface, TelemetryEnvironment::Test, (1, 2, 3)),
    )
    .unwrap()
    .into_bytes();
    let output = Envelope::from_slice(&bytes).unwrap();
    let event = output
        .into_items()
        .find_map(|item| match item {
            EnvelopeItem::Event(event) => Some(*event),
            _ => None,
        })
        .unwrap();
    (event, String::from_utf8(bytes).unwrap())
}

fn hostile_event() -> Value {
    let frame = json!({"instruction_addr":"0x1010", "function":PRIVATE,"symbol":PRIVATE,
        "module":PRIVATE,"package":PRIVATE,"filename":PRIVATE,"abs_path":PRIVATE,
        "context_line":PRIVATE,"pre_context":[PRIVATE],"post_context":[PRIVATE],
        "vars":{"secret":PRIVATE},"lineno":42,"symbol_addr":"0x1001","image_addr":"0x1000"});
    json!({"level":"error","message":PRIVATE,"platform":PRIVATE,
    "stacktrace":{"frames":[frame.clone()],"registers":{"private":"0x123"}},
    "exception":{"values":[{"type":PRIVATE,"value":PRIVATE,"module":PRIVATE,
        "thread_id":PRIVATE,"mechanism":{"type":PRIVATE},
        "stacktrace":{"frames":[frame.clone()]},"raw_stacktrace":{"frames":[frame]}}]},
    "threads":{"values":[{"id":PRIVATE,"name":PRIVATE}]},
    "debug_meta":{"sdk_info":{"sdk_name":PRIVATE,"version_major":1,"version_minor":0,"version_patchlevel":0},
        "images":[
            {"type":"apple","name":PRIVATE,"uuid":ID,"image_addr":"0x1000","image_size":256,"image_vmaddr":"0x1000","arch":PRIVATE},
            {"type":"symbolic","name":PRIVATE,"debug_file":PRIVATE,"id":ID,"image_addr":"0x1000","image_size":256,"arch":PRIVATE},
            {"type":"apple","name":PRIVATE,"uuid":ID,"image_addr":"0x2000","image_size":256},
            {"type":"apple","name":PRIVATE,"uuid":ID,"image_addr":"0xffffffffffffffff","image_size":256}
        ]}})
}

#[test]
fn desktop_envelope_retains_address_matching_without_private_details() {
    let (event, bytes) = sanitize(hostile_event(), TelemetrySurface::Desktop);
    assert!(!bytes.contains("PRIVATE"));
    assert!(!bytes.contains("/Users/"));
    assert_eq!(event.platform, "native");
    let frame = Frame {
        instruction_addr: Some(Addr(0x1010)),
        ..Default::default()
    };
    assert_eq!(
        event.stacktrace.unwrap(),
        Stacktrace {
            frames: vec![frame.clone()],
            ..Default::default()
        }
    );
    assert_eq!(
        event.exception.values,
        vec![Exception {
            ty: "SkillStudioError".into(),
            stacktrace: Some(Stacktrace {
                frames: vec![frame],
                ..Default::default()
            }),
            ..Default::default()
        }]
    );
    assert!(event.threads.values.is_empty());
    assert!(event.debug_meta.sdk_info.is_none());
    assert_eq!(event.debug_meta.images.len(), 2);
    assert_eq!(
        event.debug_meta.images[0],
        DebugImage::Apple(AppleDebugImage {
            name: "native-image".into(),
            uuid: ID.parse().unwrap(),
            image_addr: Addr(0x1000),
            image_size: 256,
            image_vmaddr: Addr(0x1000),
            arch: None,
            cpu_type: None,
            cpu_subtype: None,
        })
    );
    assert_eq!(
        event.debug_meta.images[1],
        DebugImage::Symbolic(SymbolicDebugImage {
            name: "native-image".into(),
            id: ID.parse().unwrap(),
            image_addr: Addr(0x1000),
            image_size: 256,
            image_vmaddr: Addr(0),
            arch: None,
            code_id: None,
            debug_file: None,
        })
    );
}

#[test]
fn existing_non_desktop_policy_still_drops_native_diagnostics() {
    for surface in [TelemetrySurface::Cli, TelemetrySurface::Mcp] {
        let (event, bytes) = sanitize(hostile_event(), surface);
        assert!(event.stacktrace.is_none());
        assert!(event.exception.values.is_empty());
        assert!(event.debug_meta.images.is_empty());
        assert!(!bytes.contains("PRIVATE"));
    }
}

#[test]
fn bounds_keep_the_newest_frames_and_exceptions() {
    let frames: Vec<_> = (1..=140)
        .map(|address| json!({"instruction_addr":format!("0x{address:x}")}))
        .collect();
    let exceptions: Vec<_> = (1..=12).map(|address| json!({"type":PRIVATE,"stacktrace":{"frames":[{"instruction_addr":format!("0x{address:x}")}]}})).collect();
    let (event, _) = sanitize(
        json!({"level":"fatal","stacktrace":{"frames":frames},"exception":{"values":exceptions}}),
        TelemetrySurface::Desktop,
    );
    let frames = event.stacktrace.unwrap().frames;
    assert_eq!(frames.len(), 128);
    assert_eq!(frames.first().unwrap().instruction_addr, Some(Addr(13)));
    assert_eq!(frames.last().unwrap().instruction_addr, Some(Addr(140)));
    assert_eq!(event.exception.values.len(), 8);
    assert_eq!(
        event.exception.values[0]
            .stacktrace
            .as_ref()
            .unwrap()
            .frames[0]
            .instruction_addr,
        Some(Addr(5))
    );
    assert_eq!(
        event.exception.values[7]
            .stacktrace
            .as_ref()
            .unwrap()
            .frames[0]
            .instruction_addr,
        Some(Addr(12))
    );
}

#[test]
fn relative_addressless_and_zero_frames_do_not_claim_native_diagnostics() {
    let (event, _) = sanitize(
        json!({"level":"error","stacktrace":{"frames":[
            {"instruction_addr":"0x1010","addr_mode":"rel:0"},
            {"instruction_addr":"0x0"}, {"filename":PRIVATE}
        ]}}),
        TelemetrySurface::Desktop,
    );
    assert!(event.stacktrace.is_none());
    assert_ne!(event.platform, "native");
}

#[test]
fn image_inspection_is_bounded_and_ranges_are_half_open() {
    let mut value = hostile_event();
    let image = value["debug_meta"]["images"][0].clone();
    value["debug_meta"]["images"] = json!(vec![image.clone(); 140]);
    let (event, _) = sanitize(value.clone(), TelemetrySurface::Desktop);
    assert_eq!(event.debug_meta.images.len(), 128);
    let mut unrelated = image.clone();
    unrelated["image_addr"] = json!("0x2000");
    let mut images = vec![unrelated; 128];
    images.push(image.clone());
    value["debug_meta"]["images"] = json!(images);
    assert!(sanitize(value.clone(), TelemetrySurface::Desktop)
        .0
        .debug_meta
        .images
        .is_empty());
    let mut endpoint = image.clone();
    endpoint["image_size"] = json!(16);
    let mut empty = image;
    empty["image_size"] = json!(0);
    value["debug_meta"]["images"] = json!([endpoint, empty, {"type":"proguard", "uuid": ID}]);
    assert!(sanitize(value, TelemetrySurface::Desktop)
        .0
        .debug_meta
        .images
        .is_empty());
}

#[test]
fn only_current_thread_stack_is_promoted_without_thread_metadata() {
    let stack = json!({"frames":[{"instruction_addr":"0x1010","filename":PRIVATE}]});
    let mut value = json!({"level":"error","threads":{"values":[
        {"id":PRIVATE,"name":PRIVATE,"current":false,"stacktrace":stack.clone()},
        {"id":PRIVATE,"name":PRIVATE,"current":true,"stacktrace":stack.clone()}
    ]}});
    let (event, bytes) = sanitize(value.clone(), TelemetrySurface::Desktop);
    assert_eq!(
        event.stacktrace.unwrap().frames[0].instruction_addr,
        Some(Addr(0x1010))
    );
    assert!(event.threads.values.is_empty());
    assert!(!bytes.contains("PRIVATE"));
    value["threads"]["values"][1]["current"] = json!(false);
    assert!(sanitize(value.clone(), TelemetrySurface::Desktop)
        .0
        .stacktrace
        .is_none());
    let mut threads = vec![json!({"current":false}); 128];
    threads.push(json!({"current":true,"stacktrace":stack}));
    value["threads"]["values"] = json!(threads);
    assert!(sanitize(value, TelemetrySurface::Desktop)
        .0
        .stacktrace
        .is_none());
}
