//! Guards unit 4.1: no `#[tauri::command]` may be registered in
//! `tauri::generate_handler!` without a frontend caller. Both sides are
//! derived from the source on disk (`lib.rs`'s handler list, `skill-api.ts`'s
//! `callCommand("...")` call sites), never from a copied list, so a future
//! addition to either file is checked automatically.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Pulls every `module::path::fn_name` entry out of the
/// `tauri::generate_handler![ ... ]` list in `lib.rs` and returns the bare
/// `fn_name`s - what Tauri registers each command under, absent a
/// `#[tauri::command(rename_all = ...)]` override (none of these use one).
/// `//` section headers inside the list are skipped.
fn registered_commands(lib_rs: &str) -> BTreeSet<String> {
    let start_marker = "tauri::generate_handler![";
    let start = lib_rs
        .find(start_marker)
        .expect("lib.rs must contain a tauri::generate_handler![ list")
        + start_marker.len();
    let end = lib_rs[start..]
        .find(']')
        .expect("the generate_handler! list must close with ]")
        + start;

    let mut commands = BTreeSet::new();
    for line in lib_rs[start..end].lines().map(str::trim) {
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let path = line.trim_end_matches(',');
        let name = path
            .rsplit("::")
            .next()
            .expect("each entry is a module::path::fn_name");
        commands.insert(name.to_string());
    }
    commands
}

/// Pulls every command name out of `callCommand("name", ...)` call sites -
/// `skill-api.ts` is the one file allowed to name a Tauri command (see
/// CLAUDE.md), so scanning it alone is the whole frontend caller set.
fn frontend_callers(skill_api_ts: &str) -> BTreeSet<String> {
    let mut callers = BTreeSet::new();
    let mut rest = skill_api_ts;
    while let Some(call_at) = rest.find("callCommand") {
        rest = &rest[call_at + "callCommand".len()..];
        let Some(paren_at) = rest.find('(') else {
            break;
        };
        let after_paren = &rest[paren_at + 1..];
        let Some(quote_at) = after_paren.find('"') else {
            continue;
        };
        let literal = &after_paren[quote_at + 1..];
        let Some(close_at) = literal.find('"') else {
            continue;
        };
        callers.insert(literal[..close_at].to_string());
    }
    callers
}

#[test]
fn command_registration_matches_frontend_caller_count_or_names_the_orphan() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lib_rs = fs::read_to_string(manifest_dir.join("src/lib.rs")).expect("read lib.rs");
    let skill_api_ts = fs::read_to_string(manifest_dir.join("../src/lib/skill-api.ts"))
        .expect("read skill-api.ts");

    let registered = registered_commands(&lib_rs);
    let called = frontend_callers(&skill_api_ts);

    let orphans: Vec<&String> = registered.difference(&called).collect();
    assert!(
        orphans.is_empty(),
        "these commands are registered in lib.rs's generate_handler! but have no \
         callCommand(\"...\") caller in skill-api.ts: {orphans:?}"
    );
}

/// Unit 3.8 confirmed `set_shared_harness_skill_enabled` has no frontend
/// caller and was removed under unit 4.1. Locks that: the name may only
/// reappear in `lib.rs`'s handler list alongside a matching
/// `callCommand("set_shared_harness_skill_enabled")` in `skill-api.ts`.
#[test]
fn set_shared_harness_skill_enabled_has_a_frontend_caller_or_is_removed() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lib_rs = fs::read_to_string(manifest_dir.join("src/lib.rs")).expect("read lib.rs");
    let skill_api_ts = fs::read_to_string(manifest_dir.join("../src/lib/skill-api.ts"))
        .expect("read skill-api.ts");

    let name = "set_shared_harness_skill_enabled";
    let registered = registered_commands(&lib_rs).contains(name);
    let called = frontend_callers(&skill_api_ts).contains(name);
    assert_eq!(
        registered, called,
        "{name} must be either registered with a frontend caller or fully removed; \
         registered={registered}, called={called}"
    );
}
