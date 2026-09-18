//! Unit 1.2 Section B: an architecture pin proving `fsops`'s four
//! crash-critical primitives are never called without a plan writer to
//! journal them. Since Section B moved journal recording inside the
//! primitives themselves (there's no separate `journaled_*` wrapper to
//! grep for anymore), the pin instead checks that the four call patterns
//! only ever appear where a `&PlanWriter` is in scope by construction:
//! inside `fsops.rs`'s own definitions, inside `journal.rs`'s tests, inside
//! a `tests/` integration test, or inside another file's colocated
//! `#[cfg(test)]` module (this repo's convention for where a unit test
//! lives - see AGENTS.md - so a call there needs a `&PlanWriter` too, but
//! isn't production code).

use std::fs;
use std::path::Path;

const CALL_PATTERNS: [&str; 4] = [
    "fsops::stage(",
    "fsops::swap(",
    "fsops::link(",
    "fsops::write_file(",
];

const ALLOWED_FILES: [&str; 2] = ["fsops.rs", "journal.rs"];

/// Given every `.rs` source file under `apps/` and `crates/` (excluding
/// build output), when the production part of a file outside `fsops.rs`,
/// `journal.rs`, or a `tests/` directory - i.e. everything before that
/// file's own `#[cfg(test)]` module, if it has one - calls one of the four
/// fsops primitives directly by its fully-qualified name, then the pin
/// fails and names that call site; on success, every direct call site is
/// one that is known to carry a `&PlanWriter`.
#[test]
fn no_production_code_calls_fsops_without_a_plan_writer_or_names_the_call_site() {
    let workspace_root = workspace_root();
    let mut offending = Vec::new();

    for root in ["apps", "crates"] {
        walk(&workspace_root.join(root), &mut |path, contents| {
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                return;
            }
            if is_allowed(path) {
                return;
            }
            // A colocated `#[cfg(test)]` module isn't production code; only
            // the part of the file before it needs to stay call-free.
            let production = match contents.find("#[cfg(test)]") {
                Some(at) => &contents[..at],
                None => contents,
            };
            for pattern in CALL_PATTERNS {
                if production.contains(pattern) {
                    offending.push(format!("{}: calls {pattern}", path.display()));
                }
            }
        });
    }

    assert!(
        offending.is_empty(),
        "found fsops primitive calls outside fsops.rs/journal.rs/tests: {offending:#?}"
    );
}

fn is_allowed(path: &Path) -> bool {
    if path.components().any(|c| c.as_os_str() == "tests") {
        return true;
    }
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => ALLOWED_FILES.contains(&name),
        None => false,
    }
}

fn walk(dir: &Path, visit: &mut impl FnMut(&Path, &str)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.components().any(|c| c.as_os_str() == "target") {
            continue;
        }
        if path.is_dir() {
            walk(&path, visit);
        } else if let Ok(contents) = fs::read_to_string(&path) {
            visit(&path, &contents);
        }
    }
}

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root two levels above crates/skill-studio-core")
        .to_path_buf()
}
