#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Unit 4.2: every user-facing `ops` function must be reachable from both
//! the CLI and the MCP server, so a skill fixed by one surface is fixed by
//! all three (the desktop is covered separately by
//! `apps/desktop/src-tauri/tests`). This file reads `ops.rs`, `main.rs`,
//! and `lib.rs` as text rather than depending on the `cli`/`mcp` crates:
//! `skill-studio-core` sits below both in the workspace graph, and a
//! source-text scan is enough to prove "this function is called from that
//! file" without introducing a reverse dependency.

use std::path::PathBuf;

/// A top-level `ops` function this unit deliberately does not surface on
/// the CLI or MCP, with the one-line reason it stays an internal helper.
struct Exclusion {
    name: &'static str,
    reason: &'static str,
}

const EXCLUSIONS: &[Exclusion] = &[
    Exclusion {
        name: "skill_content_hash",
        reason: "a hashing helper `scan`/`diagnose` call internally, not a request/outcome op",
    },
    Exclusion {
        name: "set_codex_skill_disabled",
        reason: "superseded by set_codex_skill_disabled_with; only this crate's own tests call it",
    },
    Exclusion {
        name: "set_codex_skill_disabled_with",
        reason: "an implementation detail behind the set_harness_enabled surface \
                  (desktop's skill_harness_disable.rs calls it directly for the Codex arm)",
    },
    Exclusion {
        name: "set_codex_sidecar_implicit_invocation",
        reason: "unadopted: desktop's codex_openai_yaml path bypasses it (see skill_invocation.rs)",
    },
    Exclusion {
        name: "install_preferences",
        reason: "only desktop's skill_install.rs calls it, as a side effect of install, \
                  not a standalone command",
    },
];

fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join("Cargo.lock").is_file() && dir.join("apps").is_dir() {
            return dir;
        }
        assert!(dir.pop(), "could not find the workspace root above ops.rs");
    }
}

/// Every `pub fn`, `pub async fn`, and `pub use` name declared at the top
/// level of `ops.rs` (column 0 - nothing nested in an `impl` block, which
/// this file never puts at column 0). `pub use` lines may name more than
/// one item in a brace list.
fn ops_rs_public_names(ops_rs: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in ops_rs.lines() {
        if let Some(rest) = line.strip_prefix("pub fn ") {
            names.push(fn_name(rest));
        } else if let Some(rest) = line.strip_prefix("pub async fn ") {
            names.push(fn_name(rest));
        } else if let Some(rest) = line.strip_prefix("pub use ") {
            names.extend(use_names(rest));
        }
    }
    names
}

fn fn_name(rest: &str) -> String {
    rest.split(['(', '<', ' ']).next().unwrap_or("").to_string()
}

/// Parses the item list out of `crate::ops_install::{install, install_preferences};`
/// or `crate::ops_remove::remove;`.
fn use_names(rest: &str) -> Vec<String> {
    let rest = rest.trim_end_matches(';').trim();
    if let Some(open) = rest.find('{') {
        let close = rest.find('}').expect("pub use brace list is never closed");
        rest[open + 1..close]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![rest
            .rsplit("::")
            .next()
            .expect("pub use always has a path")
            .to_string()]
    }
}

#[test]
fn ops_functions_have_a_cli_subcommand_and_an_mcp_tool_or_names_the_gap() {
    let root = workspace_root();
    let ops_rs = std::fs::read_to_string(root.join("crates/skill-studio-core/src/ops.rs"))
        .expect("ops.rs must exist");
    let cli_main = std::fs::read_to_string(root.join("apps/cli/src/main.rs"))
        .expect("apps/cli/src/main.rs must exist");
    let mcp_lib = std::fs::read_to_string(root.join("apps/mcp/src/lib.rs"))
        .expect("apps/mcp/src/lib.rs must exist");

    let excluded: Vec<&str> = EXCLUSIONS.iter().map(|e| e.name).collect();
    let mut missing_cli = Vec::new();
    let mut missing_mcp = Vec::new();

    for name in ops_rs_public_names(&ops_rs) {
        if excluded.contains(&name.as_str()) {
            continue;
        }
        let call = format!("ops::{name}(");
        if !cli_main.contains(&call) {
            missing_cli.push(name.clone());
        }
        if !mcp_lib.contains(&call) {
            missing_mcp.push(name);
        }
    }

    assert!(
        missing_cli.is_empty(),
        "ops.rs functions with no CLI subcommand calling them (apps/cli/src/main.rs): {missing_cli:?}"
    );
    assert!(
        missing_mcp.is_empty(),
        "ops.rs functions with no MCP tool calling them (apps/mcp/src/lib.rs): {missing_mcp:?}"
    );

    // Every exclusion must still resolve to a real ops.rs function: a typo
    // or a rename here should fail loudly, not silently stop excluding
    // anything.
    let all_names = ops_rs_public_names(&ops_rs);
    for exclusion in EXCLUSIONS {
        assert!(
            all_names.iter().any(|n| n == exclusion.name),
            "excluded name `{}` ({}) is not a pub fn/pub use in ops.rs any more - \
             drop the exclusion",
            exclusion.name,
            exclusion.reason,
        );
    }
}

/// `schema` (dumps every DTO's JSON schema to disk) and `watch` (a
/// filesystem-change loop that re-runs `scan` and prints to a terminal)
/// are CLI-only by design: neither wraps one `ops` function behind a
/// request/outcome DTO an MCP client could call once and get a single
/// envelope back. This test pins that as a decision, not an oversight the
/// parity test above should flag: it fails if either ever disappears from
/// the CLI (so nobody removes the affected doc comment without noticing)
/// and it fails if either is ever named `ops::` (which would mean it grew
/// into a real op that the parity test above should now cover).
#[test]
fn schema_and_watch_stay_cli_only_and_are_named_as_utilities_in_the_parity_test() {
    let root = workspace_root();
    let cli_main = std::fs::read_to_string(root.join("apps/cli/src/main.rs"))
        .expect("apps/cli/src/main.rs must exist");
    let mcp_lib = std::fs::read_to_string(root.join("apps/mcp/src/lib.rs"))
        .expect("apps/mcp/src/lib.rs must exist");

    for utility in ["Schema", "Watch"] {
        assert!(
            cli_main.contains(&format!("{utility} {{"))
                || cli_main.contains(&format!("{utility},")),
            "`{utility}` is no longer a CLI subcommand; update this test if that was deliberate"
        );
    }
    assert!(
        !mcp_lib.to_lowercase().contains("fn schema("),
        "an MCP `schema` tool appeared; schema is a CLI-only utility by design"
    );
    assert!(
        !mcp_lib.to_lowercase().contains("fn watch("),
        "an MCP `watch` tool appeared; watch is a CLI-only utility by design"
    );
}
