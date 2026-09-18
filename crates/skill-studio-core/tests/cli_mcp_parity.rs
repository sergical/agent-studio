#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Unit 4.2: every user-facing `ops` function must be reachable from both
//! the CLI and the MCP server, so a skill fixed by one surface is fixed by
//! all three (the desktop is covered separately by
//! `apps/desktop/src-tauri/tests`). This file reads `ops.rs`, `main.rs`,
//! and `lib.rs` as text rather than depending on the `cli`/`mcp` crates:
//! `skill-studio-core` sits below both in the workspace graph, and a
//! source-text scan is enough to prove "this function is called from that
//! file" without introducing a reverse dependency.
//!
//! The scan reads each surface's own declaration - the `Command` enum's
//! variants, the `#[tool]`-attributed methods - not the `ops::<name>(` call
//! sites: a private helper that calls an op (`apps/mcp`'s `park_direct`) is
//! not a subcommand and not a tool, and must not stand in for one.

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

/// An `ops` function whose surface spells it differently: the op's name,
/// then the subcommand or tool that runs it. Every other op reaches its
/// surface under its own name.
struct SurfaceName {
    op: &'static str,
    surface: &'static str,
}

/// `Command` variants, snake_cased, that do not repeat their op's name.
const CLI_SUBCOMMANDS: &[SurfaceName] = &[
    SurfaceName {
        op: "preview_frontmatter_repair",
        surface: "preview_repair",
    },
    SurfaceName {
        op: "apply_frontmatter_repair",
        surface: "apply_repair",
    },
    SurfaceName {
        op: "list_events",
        surface: "events",
    },
    SurfaceName {
        op: "restore_event",
        surface: "restore",
    },
    SurfaceName {
        op: "fix_skill",
        surface: "fix",
    },
    SurfaceName {
        op: "diagnose_conflict",
        surface: "conflicts",
    },
    SurfaceName {
        op: "install",
        surface: "add",
    },
    // One `update` subcommand covers both: a single `--skill` is still a
    // one-item batch through `ops::update_all`.
    SurfaceName {
        op: "update_all",
        surface: "update",
    },
];

/// `#[tool]` method names that do not repeat their op's name.
const MCP_TOOLS: &[SurfaceName] = &[
    SurfaceName {
        op: "fix_skill",
        surface: "fix",
    },
    SurfaceName {
        op: "install",
        surface: "add",
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

/// Drops a `//` line comment, so a doc comment naming an op (`ops::park`)
/// or a commented-out variant never counts as a declaration.
fn strip_line_comment(line: &str) -> &str {
    match line.find("//") {
        Some(at) => &line[..at],
        None => line,
    }
}

/// `SweepQuarantine` -> `sweep_quarantine`, the way clap derives a
/// subcommand's name from its variant.
fn snake_case(variant: &str) -> String {
    let mut out = String::new();
    for (index, ch) in variant.char_indices() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Every variant of `apps/cli/src/main.rs`'s `Command` enum, snake_cased.
/// Variants sit at one level of indentation and start with an upper-case
/// letter; their fields, attributes, and doc comments do not.
fn cli_subcommand_names(cli_main: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_enum = false;
    for line in cli_main.lines() {
        if line.starts_with("enum Command {") {
            in_enum = true;
            continue;
        }
        if !in_enum {
            continue;
        }
        if line == "}" {
            break;
        }
        let line = strip_line_comment(line);
        if line.len() - line.trim_start().len() != 4 {
            continue;
        }
        let variant: String = line
            .trim_start()
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        if variant.starts_with(|c: char| c.is_ascii_uppercase()) {
            names.push(snake_case(&variant));
        }
    }
    names
}

/// Every `#[tool]`-attributed method name inside `apps/mcp/src/lib.rs`'s
/// `#[tool_router] impl` - the tools the server actually publishes.
fn mcp_tool_names(mcp_lib: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_router = false;
    let mut under_tool_attribute = false;
    for line in mcp_lib.lines() {
        let line = strip_line_comment(line).trim();
        if line == "#[tool_router]" {
            in_router = true;
            continue;
        }
        if !in_router {
            continue;
        }
        if line == "#[tool]" || line.starts_with("#[tool(") {
            under_tool_attribute = true;
        } else if under_tool_attribute {
            // The attribute may span several lines; the method's signature
            // is the first `fn` after it.
            if let Some((_, rest)) = line.split_once("fn ") {
                names.push(fn_name(rest));
                under_tool_attribute = false;
            }
        }
    }
    names
}

/// The name a surface spells `op` with: its alias when the two differ, the
/// op's own name otherwise.
fn surface_name(op: &str, aliases: &[SurfaceName]) -> String {
    aliases
        .iter()
        .find(|alias| alias.op == op)
        .map_or(op, |alias| alias.surface)
        .to_string()
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

    let subcommands = cli_subcommand_names(&cli_main);
    let tools = mcp_tool_names(&mcp_lib);
    // A parser that reads nothing would report every op as missing; say so
    // in the surface's own terms first, so the failure names the scan and
    // not twenty innocent ops.
    assert!(
        subcommands.contains(&"scan".to_string()),
        "no `Command` enum variants were found in apps/cli/src/main.rs; this test's scan is broken, not the CLI"
    );
    assert!(
        tools.contains(&"scan".to_string()),
        "no `#[tool]` methods were found in apps/mcp/src/lib.rs; this test's scan is broken, not the MCP server"
    );

    let excluded: Vec<&str> = EXCLUSIONS.iter().map(|e| e.name).collect();
    let all_names = ops_rs_public_names(&ops_rs);
    let mut missing_cli = Vec::new();
    let mut missing_mcp = Vec::new();

    for name in &all_names {
        if excluded.contains(&name.as_str()) {
            continue;
        }
        if !subcommands.contains(&surface_name(name, CLI_SUBCOMMANDS)) {
            missing_cli.push(name.clone());
        }
        if !tools.contains(&surface_name(name, MCP_TOOLS)) {
            missing_mcp.push(name.clone());
        }
    }

    assert!(
        missing_cli.is_empty(),
        "ops.rs functions with no variant in apps/cli/src/main.rs's `Command` enum: {missing_cli:?} \
         (found: {subcommands:?})"
    );
    assert!(
        missing_mcp.is_empty(),
        "ops.rs functions with no `#[tool]` method in apps/mcp/src/lib.rs: {missing_mcp:?} \
         (found: {tools:?})"
    );

    // Every exclusion and every alias must still resolve to a real ops.rs
    // function: a typo or a rename here should fail loudly, not silently
    // stop excluding - or stop redirecting - anything.
    for exclusion in EXCLUSIONS {
        assert!(
            all_names.iter().any(|n| n == exclusion.name),
            "excluded name `{}` ({}) is not a pub fn/pub use in ops.rs any more - \
             drop the exclusion",
            exclusion.name,
            exclusion.reason,
        );
    }
    for alias in CLI_SUBCOMMANDS.iter().chain(MCP_TOOLS) {
        assert!(
            all_names.iter().any(|n| n == alias.op),
            "aliased name `{}` (surfaced as `{}`) is not a pub fn/pub use in ops.rs any more - \
             drop the alias",
            alias.op,
            alias.surface,
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

    for utility in ["schema", "watch"] {
        assert!(
            cli_subcommand_names(&cli_main).contains(&utility.to_string()),
            "`{utility}` is no longer a CLI subcommand; update this test if that was deliberate"
        );
        assert!(
            !mcp_tool_names(&mcp_lib).contains(&utility.to_string()),
            "an MCP `{utility}` tool appeared; {utility} is a CLI-only utility by design"
        );
    }
}
