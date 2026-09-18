// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Pins the two invariants that make `skill-studio-core` "adapter-free"
//! (see the crate's `Cargo.toml` description): no direct filesystem/process
//! access in production code (every side effect must go through a
//! [`skill_studio_core::ports::Ports`] port instead), and no dependency on a
//! concrete runtime/IO crate that would tie the core to one adapter.

use std::fs;
use std::path::{Path, PathBuf};

/// Files allowed to call `std::fs`/`std::process` directly: fixture and
/// benchmark support gated behind `#[cfg(any(test, feature = "testing"))]`
/// in `lib.rs` (`testing.rs`, `bench_estate.rs`), plus `lib.rs` itself,
/// whose only use is inside its own `#[cfg(test)] mod home_free_tests` -
/// the test that guards the sibling `HOME`-free invariant by reading
/// source files as text.
const ALLOWED_FILES: &[&str] = &["testing.rs", "bench_estate.rs", "lib.rs"];

/// `std::process::id()` reads the running process's own PID - it does no
/// I/O, spawns nothing, and has no `Ports` method to route through (unlike
/// a `Clock` or `ScopeFs` call, mocking it buys no test coverage). `fsops`
/// mixes it into a temp-name suffix purely so two names never collide.
const ALLOWED_CALLS: &[&str] = &["std::process::id()"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn core_crate_has_no_std_fs_or_std_process_or_names_the_call_site() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src_dir, &mut files);
    assert!(!files.is_empty(), "expected to find core source files");

    let mut violations = Vec::new();
    for path in files {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("utf8 file name");
        if ALLOWED_FILES.contains(&file_name) {
            continue;
        }
        let content = fs::read_to_string(&path).expect("read source file");
        for (line_no, line) in content.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue; // doc comments may mention std::fs/std::process in prose
            }
            let has_violation = (line.contains("std::fs::") || line.contains("std::process::"))
                && !ALLOWED_CALLS.iter().any(|call| line.contains(call));
            if has_violation {
                violations.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    line_no + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "core src must route filesystem/process access through a Ports adapter, \
         not std::fs/std::process directly (add to ALLOWED_FILES only for test/\
         bench support, never production code):\n{}",
        violations.join("\n")
    );
}

#[test]
fn core_crate_cargo_toml_has_no_tauri_rusqlite_tokio_or_reqwest_dependency_or_names_the_offender() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("read Cargo.toml");
    let parsed: toml::Value = toml::from_str(&manifest).expect("parse Cargo.toml");

    let forbidden = ["tauri", "rusqlite", "tokio", "reqwest"];
    let mut found = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(table) = parsed.get(section).and_then(|v| v.as_table()) else {
            continue;
        };
        for name in table.keys() {
            if forbidden.contains(&name.as_str()) {
                found.push(format!("{section}.{name}"));
            }
        }
    }

    assert!(
        found.is_empty(),
        "skill-studio-core must stay adapter-free - move these dependencies to \
         skill-studio-host or an app crate instead:\n{}",
        found.join("\n")
    );
}

/// The runtime proof that this deny list is live: add `let _ = Some(1).unwrap();`
/// to `lib.rs`, run `cargo clippy -p skill-studio-core --all-targets`, watch it
/// go red on `clippy::unwrap_used`, then revert the line before merging. This
/// test is the durable form of that one-time proof - it pins the deny list
/// itself in the root `Cargo.toml`, so a future edit that loosens it fails
/// here instead of silently letting `.unwrap()` back into core.
#[test]
fn workspace_lints_deny_unwrap_expect_panic_todo_unimplemented_dbg_print_in_core_or_names_the_missing_lint(
) {
    let manifest_path = workspace_root().join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("read root Cargo.toml");
    let parsed: toml::Value = toml::from_str(&manifest).expect("parse root Cargo.toml");

    let clippy_lints = parsed
        .get("workspace")
        .and_then(|w| w.get("lints"))
        .and_then(|l| l.get("clippy"))
        .and_then(|c| c.as_table())
        .expect("root Cargo.toml has no [workspace.lints.clippy] table");

    let required_deny = [
        "unwrap_used",
        "expect_used",
        "panic",
        "todo",
        "unimplemented",
        "dbg_macro",
        "print_stdout",
        "print_stderr",
    ];
    let mut missing = Vec::new();
    for lint in required_deny {
        let level = clippy_lints
            .get(lint)
            .and_then(toml::Value::as_str)
            .unwrap_or("");
        if level != "deny" {
            missing.push(format!("clippy::{lint} is {level:?}, expected \"deny\""));
        }
    }

    assert!(
        missing.is_empty(),
        "[workspace.lints.clippy] in the root Cargo.toml must deny every lint core \
         depends on to stay panic-free:\n{}",
        missing.join("\n")
    );
}

/// `cargo machete` and `cargo deny check` are the two supply-chain checks
/// plan.md section 7 asks for; both must run in CI with zero findings.
/// A findings failure surfaces the offending crate directly in that job's
/// output, so this test only pins that the steps exist and that `deny.toml`
/// (the policy `cargo deny check` reads) is in place - dropping either from
/// `rust.yml` fails here by naming the missing step.
#[test]
fn rust_ci_workflow_runs_cargo_machete_and_cargo_deny_against_deny_toml_or_names_the_missing_step()
{
    let workspace_root = workspace_root();
    let workflow_path = workspace_root.join(".github/workflows/rust.yml");
    let workflow = fs::read_to_string(&workflow_path).expect("read .github/workflows/rust.yml");

    let mut missing = Vec::new();
    if !workflow.contains("cargo machete") {
        missing.push("rust.yml has no `cargo machete` step".to_string());
    }
    if !workflow.contains("cargo deny check") {
        missing.push("rust.yml has no `cargo deny check` step".to_string());
    }
    if !workspace_root.join("deny.toml").exists() {
        missing.push("deny.toml (the policy cargo deny check reads) is missing".to_string());
    }

    assert!(missing.is_empty(), "{}", missing.join("\n"));
}
