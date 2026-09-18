//! Guards unit 1.3: the desktop crate must not reintroduce the old
//! process-wide `ForkMutationLock` mutex. Every write it used to guard now
//! takes a per-root `WriteLease` (`skills/write_lease.rs`) instead, so a
//! concurrent CLI or MCP write on the same root serializes with the desktop
//! too - a global in-process mutex never could.

// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

/// Recursively collects every `.rs` file under `dir`.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("read_dir entry");
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn fork_mutation_lock_is_deleted_from_the_desktop_crate_or_names_the_file_that_still_uses_it() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_dir = manifest_dir.join("src");

    let mut files = Vec::new();
    rust_files(&src_dir, &mut files);

    let offenders: Vec<String> = files
        .into_iter()
        .filter(|path| {
            let contents = fs::read_to_string(path).unwrap_or_default();
            contents.contains("ForkMutationLock")
        })
        .map(|path| {
            path.strip_prefix(manifest_dir)
                .unwrap_or(&path)
                .display()
                .to_string()
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "ForkMutationLock should be gone from the desktop crate, still referenced in: {offenders:?}"
    );
}
