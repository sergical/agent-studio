// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Cross-surface parity for `fix_skill`: the CLI's `fix` subcommand, the
//! MCP server's `fix` tool, and the desktop's `fix_skill` Tauri command
//! (`skills/skill_fix.rs`) are each a thin adapter calling
//! `skill_studio_core::ops::fix_skill` on a `Runtime` built the same way.
//! The MCP server shares the CLI's own runtime builder
//! (`apps/cli/src/main.rs`'s `build_runtime_write`, called from
//! `apps/mcp`), so proving CLI/desktop parity here also covers MCP - there
//! is no third construction to test separately. This test runs the real
//! `skill-studio` CLI binary (`skill-studio-cli`'s `fix` subcommand,
//! `--home` pointed at a fixture) for the CLI/MCP side, and the desktop
//! adapter's own `core_runtime::build_runtime_write_at` for the desktop
//! side, over two byte-identical copies of the same fixture, and proves
//! `fix_skill` leaves both trees byte-identical, so no surface's adapter
//! has drifted from the shared op.
//!
//! Red check performed by hand while writing this test (not left in the
//! tree): temporarily added `std::fs::write(a.path.join("SKILL.md"), ...)`
//! inside `ops::conflicts_in`'s conflict branch, simulating a regression
//! that writes on a conflict instead of only reporting it; the parity test
//! below failed on a hash mismatch for `dup-skill`, confirming the checksum
//! comparison actually catches a conflict that writes. Reverted before
//! committing.
//!
//! Coverage gap (G5, review round 2): the fixture here has no `OpenCode`
//! skills, so it never exercises the difference between
//! `core_runtime.rs`'s `opencode_config_dir(home)` (desktop) and
//! `scope.rs`'s `opencode_config_dir_under(home)` (CLI) - if those two
//! ever resolve a different directory for the same `home`, this test
//! would not catch it. Tracked in `issue-3.7b-followup-a.md`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use skill_studio_core::dto::FixSkillRequest;
use skill_studio_core::identity::SkillName;
use skill_studio_core::ops;
use skill_studio_core::ports::Runtime;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};

use skill_studio_lib::skills::core_runtime::build_runtime_write_at;

mod cli_binary;
use cli_binary::cli_binary_path;

/// A malformed-frontmatter skill (the one safe repair `fix_skill` applies)
/// plus two differing copies of another skill (a conflict `fix_skill` must
/// name, never merge) - the same two doctor invariants
/// `crates/skill-studio-core/tests/fix_and_conflicts.rs` and unit 3.7b's
/// fork-pull conflict test each cover separately, combined here so one
/// `fix_skill` call exercises both an `applied` and a `conflicts` entry.
fn write_fixture(home: &Path) {
    fs::create_dir_all(home.join(".claude/skills/zeta-bad")).unwrap();
    fs::write(
        home.join(".claude/skills/zeta-bad/SKILL.md"),
        b"---\nname: zeta-bad\ndescription: Use this: when needed\n---\nBody.\n",
    )
    .unwrap();

    fs::create_dir_all(home.join(".claude/skills/dup-skill")).unwrap();
    fs::write(
        home.join(".claude/skills/dup-skill/SKILL.md"),
        b"---\nname: dup-skill\ndescription: from claude\n---\nBody A.\n",
    )
    .unwrap();
    fs::create_dir_all(home.join(".codex/skills/dup-skill")).unwrap();
    fs::write(
        home.join(".codex/skills/dup-skill/SKILL.md"),
        b"---\nname: dup-skill\ndescription: from codex\n---\nBody B.\n",
    )
    .unwrap();
}

/// Builds the desktop's own `Runtime`, via the exact function
/// `skills/skill_fix.rs`'s `fix_skill` Tauri command calls
/// (`core_runtime::build_runtime_write` minus the host `dirs::home_dir()`
/// lookup), rooted at `home` with its data root namespaced alongside it so
/// the test never touches the real machine's `~/.local/share/skill-studio`.
fn desktop_runtime_at(home: &Path) -> Runtime {
    build_runtime_write_at(home, &home.join(".skill-studio")).expect("desktop runtime")
}

/// Runs the real `skill-studio` CLI binary's `fix` subcommand against
/// `home` - the same binary the MCP server's runtime-building shares
/// (`apps/mcp` calls the CLI's `build_runtime_write`, per `apps/cli/src/
/// main.rs`), so this stands in for both the CLI and MCP surfaces.
fn cli_fix(home: &Path, skill: &str) {
    let binary = cli_binary_path();
    assert!(
        binary.is_file(),
        "{} not found - run `cargo build -p skill-studio-cli` first",
        binary.display()
    );
    let output = std::process::Command::new(&binary)
        .args(["fix", "--home"])
        .arg(home)
        .args(["--skill", skill, "--json"])
        .output()
        .expect("spawn skill-studio-cli");
    // The CLI's exit code reflects `fix_skill`'s own outcome (non-zero when
    // anything is left `unrepaired` or `conflicts`, by design - see
    // `finish` in `apps/cli/src/main.rs`), not whether the adapter itself
    // ran; only a crash (empty stdout) means this call didn't work.
    assert!(
        !output.stdout.is_empty(),
        "skill-studio fix --skill {skill} produced no output: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Hashes every `SKILL.md` under `home`'s two fixture skills, keyed by the
/// path relative to `home` so two different homes compare equal when their
/// content matches.
fn content_fingerprint(home: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for rel in [
        ".claude/skills/zeta-bad/SKILL.md",
        ".claude/skills/dup-skill/SKILL.md",
        ".codex/skills/dup-skill/SKILL.md",
    ] {
        out.insert(PathBuf::from(rel), fs::read(home.join(rel)).unwrap());
    }
    out
}

/// Given two byte-identical fixture homes, when `fix_skill` runs for each
/// skill once through the real CLI binary and once through the desktop
/// adapter's own runtime constructor, then both trees stay byte-identical
/// to each other afterward, or names the diverging file. `OpencodeHomeGuard`
/// (G6, review round 2) pins `XDG_CONFIG_HOME`/`OPENCODE_CONFIG_DIR` to
/// `home_desktop` for the whole test so it reads its own fixture rather
/// than a real `~/.config/opencode` a CI runner might export.
#[test]
fn fix_names_and_hashes_agree_between_the_cli_binary_and_the_desktop_adapter_or_names_the_diverging_file(
) {
    let home_cli = unique_temp_dir("fix-parity-cli");
    let home_desktop = unique_temp_dir("fix-parity-desktop");
    write_fixture(&home_cli);
    write_fixture(&home_desktop);

    let _opencode_home =
        skill_studio_lib::skills::test_support::OpencodeHomeGuard::new(&home_desktop);
    let rt_desktop = desktop_runtime_at(&home_desktop);

    for skill in ["zeta-bad", "dup-skill"] {
        cli_fix(&home_cli, skill);
        ops::fix_skill(
            &rt_desktop,
            &ctx(),
            &FixSkillRequest {
                skill: SkillName(skill.to_string()),
            },
        )
        .unwrap_or_else(|e| panic!("fix_skill({skill}) at {}: {e:?}", home_desktop.display()));
    }

    assert_eq!(
        content_fingerprint(&home_cli),
        content_fingerprint(&home_desktop),
        "fix_skill left the two independently built runtimes' trees with different bytes"
    );

    // `zeta-bad` was repaired identically on both sides.
    let repaired = fs::read_to_string(home_cli.join(".claude/skills/zeta-bad/SKILL.md")).unwrap();
    assert!(repaired.contains("description: |-"), "{repaired}");

    // `dup-skill` was never merged: both original copies are untouched.
    assert_eq!(
        fs::read(home_cli.join(".claude/skills/dup-skill/SKILL.md")).unwrap(),
        b"---\nname: dup-skill\ndescription: from claude\n---\nBody A.\n"
    );
    assert_eq!(
        fs::read(home_cli.join(".codex/skills/dup-skill/SKILL.md")).unwrap(),
        b"---\nname: dup-skill\ndescription: from codex\n---\nBody B.\n"
    );

    let _ = fs::remove_dir_all(&home_cli);
    let _ = fs::remove_dir_all(&home_desktop);
}
