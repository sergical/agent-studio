// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Unit 4.2: `park` run once through each of the three surfaces - the real
//! CLI binary, the MCP server's tool handler (called directly, in-process,
//! per `apps/mcp/src/lib.rs::run_op_envelope`), and the desktop's own
//! `park_with_runtime` seam - must leave byte-identical disk state. Unlike
//! `fix_parity.rs`, which reasons that CLI parity stands in for MCP because
//! both shared one runtime builder, `apps/mcp` now has its own `lib.rs`
//! with its own env-var-driven runtime construction (`scope::resolve`), so
//! this test drives all three independently rather than treating any pair
//! as equivalent.
//!
//! `SKILL_STUDIO_HOME`/`SKILL_STUDIO_FIXTURE`/`SKILL_STUDIO_PROJECT` are
//! process-wide env vars `apps/mcp`'s `scope::resolve` reads at call time;
//! this file has exactly one `#[test]`, so there is no other test in this
//! binary to race with over those vars.
//!
//! The comparison covers the skill roots and the `.skill-studio` data root.
//! Two things there cannot be compared as bytes and are compared by
//! substitute instead:
//!
//! - `history/events.sqlite3`: a binary database file, whose pages,
//!   rowids, and free space differ between two runs that recorded
//!   identical history. Its
//!   rows are compared instead, with the per-run columns (`id`, `ts`,
//!   `reverted_by`, `created_by`) blanked and each home's own path
//!   rewritten to `<home>`, plain and percent-encoded.
//! - `leases/*.lock`: named after a hash of the root's absolute path and
//!   holding `<pid>|<epoch ms>`, so neither the name nor the body repeats
//!   across homes or runs. Only how many lock files exist is compared.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use skill_studio_core::dto::{ParkRequest, ScanRequest};
use skill_studio_core::identity::{DeploymentId, RootKind};
use skill_studio_core::ops::{self, Operation};
use skill_studio_core::testing::golden::ctx;
use skill_studio_core::OpStatus;

use skill_studio_lib::skills::core_runtime::{
    build_runtime_write_at_with_search_dirs, process_path_search_dirs,
};
use skill_studio_lib::skills::skill_park::park_with_runtime;

mod cli_binary;
use cli_binary::cli_binary_path;

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";
const CLAUDE_ROOT_RELATIVE: &str = ".claude/skills";
const DATA_ROOT_RELATIVE: &str = ".skill-studio";

/// A temp home that removes itself when it drops, whether the test passes
/// or panics.
fn temp_home(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(prefix).tempdir().unwrap()
}

/// A home with one universal skill (`gamma`), linked from Claude Code's
/// per-skill root - the shape `ops::park` looks for. Matches
/// `crates/skill-studio-core/tests/park_and_unpark.rs`'s `parkable_home`.
fn parkable_home(home: &Path) {
    let dir = home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        b"---\nname: gamma\ndescription: a parkable skill\n---\nBody.\n",
    )
    .unwrap();
    let claude_skills = home.join(CLAUDE_ROOT_RELATIVE);
    fs::create_dir_all(&claude_skills).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&dir, claude_skills.join("gamma")).unwrap();
}

/// `gamma`'s universal deployment id under `home`. `DeploymentId` encodes
/// `home`'s absolute path, so three different temp directories - even with
/// identical content - get three different ids; each home's id is computed
/// from that same home, not shared across them.
fn universal_deployment_id(home: &Path) -> DeploymentId {
    // The process's own PATH, not a real login-shell probe: park/unpark
    // parity never spawns `npx`, so it doesn't need to pay for (or risk
    // hanging on) a real `$SHELL -lic` spawn.
    let rt = build_runtime_write_at_with_search_dirs(
        home,
        &home.join(".skill-studio"),
        process_path_search_dirs(),
    )
    .expect("runtime");
    let inventory = ops::scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
    let skill = inventory
        .skills
        .iter()
        .find(|s| s.name.0 == "gamma")
        .expect("gamma is scanned");
    skill
        .deployments
        .iter()
        .find(|d| d.root.kind == RootKind::Universal)
        .expect("gamma has a universal deployment")
        .id
        .clone()
}

/// Runs the real `skill-studio` CLI binary's `park` subcommand against
/// `home`.
fn cli_park(home: &Path, deployment_id: &DeploymentId) {
    let binary = cli_binary_path();
    assert!(
        binary.is_file(),
        "{} not found - run `cargo build -p skill-studio-cli` first",
        binary.display()
    );
    let output = std::process::Command::new(&binary)
        .args(["park", "--home"])
        .arg(home)
        .args(["--deployment-id", deployment_id.as_str(), "--json"])
        .output()
        .expect("spawn skill-studio-cli park");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "skill-studio park printed no envelope ({e}): {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(
        envelope["status"],
        "ok",
        "cli park at {}: {envelope:?}",
        home.display()
    );
}

/// Runs `park` through `apps/mcp/src/lib.rs::run_op_envelope`, the one
/// function the MCP server's `park` tool runs (via `run_op`, as every tool
/// method does), against `home`, via `SKILL_STUDIO_HOME` - the only way
/// `apps/mcp`'s `scope::resolve` learns which home to use.
fn mcp_park(home: &Path, deployment_id: &DeploymentId) {
    // SAFETY (env-var race): this file has exactly one #[test]; nothing
    // else in this process reads or writes these vars concurrently.
    std::env::set_var("SKILL_STUDIO_HOME", home);
    std::env::remove_var("SKILL_STUDIO_FIXTURE");
    std::env::remove_var("SKILL_STUDIO_PROJECT");
    let req = ParkRequest {
        deployment_id: deployment_id.clone(),
    };
    let envelope = skill_studio_mcp::run_op_envelope(Operation::Park, true, |rt, ctx| {
        ops::park(rt, ctx, &req)
    });
    std::env::remove_var("SKILL_STUDIO_HOME");
    assert_eq!(
        envelope.status,
        OpStatus::Ok,
        "mcp park at {}: {:?}",
        home.display(),
        envelope.errors
    );
}

/// Runs `ops::park` the way the desktop's `park_skill` Tauri command does -
/// `skill_park::park_with_runtime` - against `home`.
fn desktop_park(home: &Path, deployment_id: DeploymentId) {
    park_with_runtime(home, &home.join(".skill-studio"), deployment_id)
        .unwrap_or_else(|e| panic!("desktop park_with_runtime at {}: {}", home.display(), e));
}

/// Every regular file under `home`'s skill roots, keyed by its path
/// relative to `home`, plus whether a Claude Code `gamma` link still
/// exists (park must remove it) and where the universal `gamma` directory
/// ended up (`.agents/skills` or `.agents/skills-parked`).
fn skill_tree_snapshot(home: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for root in [".agents/skills", ".agents/skills-parked", ".claude/skills"] {
        let base = home.join(root);
        if !base.is_dir() {
            continue;
        }
        for entry in walkdir(&base) {
            let rel = entry.strip_prefix(home).unwrap().to_path_buf();
            if entry.is_file() {
                out.insert(rel, fs::read(&entry).unwrap());
            } else if entry.is_symlink() {
                let target = fs::read_link(&entry).unwrap();
                out.insert(rel, target.display().to_string().into_bytes());
            }
        }
    }
    out
}

/// Every file under `home`'s `.skill-studio` data root, keyed by its path
/// relative to that root. Per the module doc, the journal is compared as
/// normalised rows and the lease locks only by count; anything else a
/// surface writes there is compared as bytes, with `home`'s own path
/// rewritten so three different temp homes can still be equal.
fn data_root_snapshot(home: &Path) -> BTreeMap<String, String> {
    let data_root = home.join(DATA_ROOT_RELATIVE);
    let mut out = BTreeMap::new();
    let mut lease_locks = 0usize;
    for entry in walkdir(&data_root) {
        if !entry.is_file() {
            continue;
        }
        let rel = entry.strip_prefix(&data_root).unwrap().to_path_buf();
        if rel.starts_with("leases") {
            lease_locks += 1;
        } else if rel == Path::new("history/events.sqlite3") {
            out.insert(
                "history/events.sqlite3 rows".to_string(),
                journal_rows(&entry, home),
            );
        } else {
            let bytes = fs::read(&entry).unwrap();
            out.insert(
                rel.display().to_string(),
                without_home(&String::from_utf8_lossy(&bytes), home),
            );
        }
    }
    out.insert("leases/*.lock count".to_string(), lease_locks.to_string());
    out
}

/// Every row of every table in the history journal, rendered as text: one
/// `column=value` line per row, tables in name order and rows sorted so
/// insertion order does not decide equality. The per-run columns are
/// blanked rather than dropped, so a surface that stops writing one still
/// shows up as a difference.
fn journal_rows(db_path: &Path, home: &Path) -> String {
    const PER_RUN_COLUMNS: &[&str] = &[
        "id",
        "event_id",
        "correlation_id",
        "ts",
        "reverted_by",
        "created_by",
    ];

    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    let mut out = String::new();
    for table in tables {
        let mut statement = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let columns: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut lines: Vec<String> = statement
            .query_map([], |row| {
                Ok(columns
                    .iter()
                    .enumerate()
                    .map(|(index, column)| {
                        let value = if PER_RUN_COLUMNS.contains(&column.as_str()) {
                            "<per-run>".to_string()
                        } else {
                            cell_text(row.get_ref_unwrap(index), home)
                        };
                        format!("{column}={value}")
                    })
                    .collect::<Vec<_>>()
                    .join(" "))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        lines.sort();
        out.push('[');
        out.push_str(&table);
        out.push_str("]\n");
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

fn cell_text(value: rusqlite::types::ValueRef<'_>, home: &Path) -> String {
    match value {
        rusqlite::types::ValueRef::Null => "null".to_string(),
        rusqlite::types::ValueRef::Integer(n) => n.to_string(),
        rusqlite::types::ValueRef::Real(n) => n.to_string(),
        rusqlite::types::ValueRef::Text(bytes) => {
            without_home(&String::from_utf8_lossy(bytes), home)
        }
        rusqlite::types::ValueRef::Blob(bytes) => format!("blob:{} bytes", bytes.len()),
    }
}

/// Rewrites `home`'s absolute path to `<home>`, both as written and
/// percent-encoded - a `DeploymentId` carries the path in the second form.
fn without_home(text: &str, home: &Path) -> String {
    let path = home.display().to_string();
    text.replace(&path, "<home>")
        .replace(&path.replace('/', "%2F"), "<home>")
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_symlink = fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if is_symlink {
                out.push(path);
            } else if path.is_dir() {
                stack.push(path.clone());
                out.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Given three byte-identical fixture homes, when `park` runs for the same
/// deployment once through the real CLI binary, once through the MCP
/// server's tool handler (in-process), and once through the desktop's own
/// runtime constructor, then all three resulting trees agree: the `gamma`
/// directory left `.agents/skills` for `.agents/skills-parked` on every
/// surface, and the Claude Code link is gone on every surface, or the test
/// names which surface diverged.
#[test]
fn cli_and_mcp_and_desktop_write_the_same_disk_state_for_park_or_names_the_diverging_surface() {
    // TempDir, not a plain path: it removes the tree when it drops, so a
    // failing assertion below does not leave three fixture homes behind.
    let cli_dir = temp_home("park-parity-cli");
    let mcp_dir = temp_home("park-parity-mcp");
    let desktop_dir = temp_home("park-parity-desktop");
    let (home_cli, home_mcp, home_desktop) = (cli_dir.path(), mcp_dir.path(), desktop_dir.path());
    for home in [home_cli, home_mcp, home_desktop] {
        parkable_home(home);
    }

    // Each home's own deployment id: DeploymentId encodes the home's
    // absolute path, so the three temp directories get three different
    // (but each internally consistent) ids for the same `gamma` skill.
    cli_park(home_cli, &universal_deployment_id(home_cli));
    mcp_park(home_mcp, &universal_deployment_id(home_mcp));
    desktop_park(home_desktop, universal_deployment_id(home_desktop));

    let cli_tree = skill_tree_snapshot(home_cli);
    let mcp_tree = skill_tree_snapshot(home_mcp);
    let desktop_tree = skill_tree_snapshot(home_desktop);

    assert_eq!(
        cli_tree, mcp_tree,
        "the CLI and the MCP server disagree on the disk state park left"
    );
    assert_eq!(
        cli_tree, desktop_tree,
        "the CLI and the desktop disagree on the disk state park left"
    );

    let cli_data = data_root_snapshot(home_cli);
    // Without this the two comparisons below could pass on three empty
    // snapshots, proving nothing about the journal.
    assert!(
        cli_data
            .get("history/events.sqlite3 rows")
            .is_some_and(|rows| rows.contains("kind=park")),
        "the data root snapshot holds no park row: {cli_data:?}"
    );
    assert_eq!(
        cli_data,
        data_root_snapshot(home_mcp),
        "the CLI and the MCP server disagree on the data root park left"
    );
    assert_eq!(
        cli_data,
        data_root_snapshot(home_desktop),
        "the CLI and the desktop disagree on the data root park left"
    );

    // park moved `gamma` out of `.agents/skills` and dropped the Claude
    // Code link, on every surface.
    for home in [home_cli, home_mcp, home_desktop] {
        assert!(!home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma").exists());
        assert!(home.join(".agents/skills-parked/gamma/SKILL.md").is_file());
        assert!(!home.join(CLAUDE_ROOT_RELATIVE).join("gamma").exists());
    }
}
