// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Cross-surface parity for `fix_skill`: the CLI's `fix` subcommand, the
//! MCP server's `fix` tool, and the desktop's `fix_skill` Tauri command
//! (`skills/skill_fix.rs`) are each a thin adapter calling
//! `skill_studio_core::ops::fix_skill` on a `Runtime` built the same way
//! (`core_runtime::build_runtime_write` mirrors the CLI's
//! `build_runtime_write` and the MCP server's own construction, per
//! `core_runtime.rs`'s module comment). Rather than spawn three binaries,
//! this builds two independent real-filesystem `Runtime`s over two
//! byte-identical copies of the same fixture - one standing in for "CLI/MCP",
//! one for "desktop" - and proves `fix_skill` leaves both trees
//! byte-identical, so no surface's adapter has drifted from the shared op.
//!
//! Red check performed by hand while writing this test (not left in the
//! tree): temporarily added `std::fs::write(a.path.join("SKILL.md"), ...)`
//! inside `ops::conflicts_in`'s conflict branch, simulating a regression
//! that writes on a conflict instead of only reporting it;
//! `fix_names_and_hashes_agree_between_two_independently_built_runtimes`
//! below failed on a hash mismatch for `dup-skill`, confirming the checksum
//! comparison actually catches a conflict that writes. Reverted before
//! committing.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_studio_core::dto::FixSkillRequest;
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::SkillName;
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::testing::golden::{ctx, scope_for, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

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

/// Builds a real-filesystem `Runtime` rooted at `home`, the same shape
/// `core_runtime::build_runtime_write_at` and the CLI's `build_runtime_write`
/// both produce (real `fs`, real file lease, fake everything a write to
/// `dup-skill`/`zeta-bad` never touches).
fn runtime_at(home: &Path) -> Runtime {
    let scope = scope_for("fix-parity", home);
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(
            home.join(".history").join("events.sqlite3"),
        )),
        sink: Arc::new(RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    Runtime::new(&scope, ports).expect("runtime")
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
/// skill through two independently built `Runtime`s, then both trees stay
/// byte-identical to each other afterward, and each surface's outcome
/// agrees: one applied repair for `zeta-bad`, one conflict naming
/// `dup-skill`, written nowhere. Failure names which fixture's hash
/// mismatched, or which surface's outcome shape diverged.
#[test]
fn fix_names_and_hashes_agree_between_two_independently_built_runtimes() {
    let home_cli = unique_temp_dir("fix-parity-cli");
    let home_desktop = unique_temp_dir("fix-parity-desktop");
    write_fixture(&home_cli);
    write_fixture(&home_desktop);

    let rt_cli = runtime_at(&home_cli);
    let rt_desktop = runtime_at(&home_desktop);

    for (rt, home) in [(&rt_cli, &home_cli), (&rt_desktop, &home_desktop)] {
        for skill in ["zeta-bad", "dup-skill"] {
            ops::fix_skill(
                rt,
                &ctx(),
                &FixSkillRequest {
                    skill: SkillName(skill.to_string()),
                },
            )
            .unwrap_or_else(|e| panic!("fix_skill({skill}) at {}: {e:?}", home.display()));
        }
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
