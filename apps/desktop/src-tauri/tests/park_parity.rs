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

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use skill_studio_core::dto::{ParkRequest, ScanRequest};
use skill_studio_core::identity::{DeploymentId, RootKind};
use skill_studio_core::ops::{self, Operation};
use skill_studio_core::testing::golden::ctx;
use skill_studio_core::OpStatus;

use skill_studio_lib::skills::core_runtime::build_runtime_write_at;
use skill_studio_lib::skills::skill_park::park_with_runtime;

const UNIVERSAL_ROOT_RELATIVE: &str = ".agents/skills";
const CLAUDE_ROOT_RELATIVE: &str = ".claude/skills";

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
    let rt = build_runtime_write_at(home, &home.join(".skill-studio")).expect("runtime");
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

fn cli_binary_path() -> PathBuf {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let target_dir = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => workspace_root().join("target"),
    };
    target_dir.join(profile).join("skill-studio")
}

fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("Cargo.toml");
        if candidate.is_file() {
            let contents = fs::read_to_string(&candidate).unwrap_or_default();
            if contents.contains("[workspace]") {
                return dir;
            }
        }
        assert!(
            dir.pop(),
            "no workspace Cargo.toml found above {}",
            env!("CARGO_MANIFEST_DIR")
        );
    }
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
    assert!(
        !output.stdout.is_empty(),
        "skill-studio park produced no output: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs `park` through the same function the MCP server's `park` tool runs
/// - `apps/mcp/src/lib.rs::run_op_envelope`, which `run_op` and therefore
/// every tool method goes through - against `home`, via `SKILL_STUDIO_HOME`
/// (the only way `apps/mcp`'s `scope::resolve` learns which home to use).
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
fn cli_and_mcp_and_desktop_write_the_same_disk_state_for_each_op_or_names_the_diverging_surface() {
    let home_cli = skill_studio_core::testing::golden::unique_temp_dir("park-parity-cli");
    let home_mcp = skill_studio_core::testing::golden::unique_temp_dir("park-parity-mcp");
    let home_desktop = skill_studio_core::testing::golden::unique_temp_dir("park-parity-desktop");
    for home in [&home_cli, &home_mcp, &home_desktop] {
        parkable_home(home);
    }

    // Each home's own deployment id: DeploymentId encodes the home's
    // absolute path, so the three temp directories get three different
    // (but each internally consistent) ids for the same `gamma` skill.
    cli_park(&home_cli, &universal_deployment_id(&home_cli));
    mcp_park(&home_mcp, &universal_deployment_id(&home_mcp));
    desktop_park(&home_desktop, universal_deployment_id(&home_desktop));

    let cli_tree = skill_tree_snapshot(&home_cli);
    let mcp_tree = skill_tree_snapshot(&home_mcp);
    let desktop_tree = skill_tree_snapshot(&home_desktop);

    assert_eq!(
        cli_tree, mcp_tree,
        "the CLI and the MCP server disagree on the disk state park left"
    );
    assert_eq!(
        cli_tree, desktop_tree,
        "the CLI and the desktop disagree on the disk state park left"
    );

    // park moved `gamma` out of `.agents/skills` and dropped the Claude
    // Code link, on every surface.
    for home in [&home_cli, &home_mcp, &home_desktop] {
        assert!(!home.join(UNIVERSAL_ROOT_RELATIVE).join("gamma").exists());
        assert!(home.join(".agents/skills-parked/gamma/SKILL.md").is_file());
        assert!(!home.join(CLAUDE_ROOT_RELATIVE).join("gamma").exists());
    }

    for home in [&home_cli, &home_mcp, &home_desktop] {
        let _ = fs::remove_dir_all(home);
    }
}
