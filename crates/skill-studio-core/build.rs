fn main() {
    println!("cargo:rerun-if-changed=native/event_fcntl_bridge.c");
    println!("cargo:rerun-if-changed=native/event_fcntl_bridge.h");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")
        && std::env::var_os("CARGO_FEATURE_EVENT_STORE").is_some()
    {
        cc::Build::new()
            .file("native/event_fcntl_bridge.c")
            .define("_DARWIN_C_SOURCE", None)
            .flag_if_supported("-std=c11")
            .warnings_into_errors(true)
            .compile("skill_studio_event_fcntl");
    }
}
