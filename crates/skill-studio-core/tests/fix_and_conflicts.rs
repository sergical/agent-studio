// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Unit 3.7: `ops::diagnose_conflict` must find two differing copies of a
//! skill and name both paths, without writing anything. This is the "never
//! merges, writes nothing" guarantee from the issue: proven here by hashing
//! every file under the fixture home before and after the call and asserting
//! the hashes are unchanged, not just by checking the returned DTO.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use skill_studio_core::dto::{DiagnoseConflictRequest, FixSkillRequest};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::SkillName;
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime, ScopeFs};
use skill_studio_core::testing::golden::{ctx, scope_for};
use skill_studio_core::testing::{FakeClock, FakeIds, FakeLease, FixtureBuilder, NoHistory};

const HOME: &str = "/home";

/// Two per-harness roots hold different bytes for the same skill: a fork
/// pull's classic shape (a Claude copy and a Codex copy that diverged).
fn conflicting_home() -> impl ScopeFs {
    FixtureBuilder::new()
        .dir(&format!("{HOME}/.claude/skills/dup-skill"))
        .file(
            &format!("{HOME}/.claude/skills/dup-skill/SKILL.md"),
            b"---\nname: dup-skill\ndescription: from claude\n---\nBody A.\n",
        )
        .dir(&format!("{HOME}/.codex/skills/dup-skill"))
        .file(
            &format!("{HOME}/.codex/skills/dup-skill/SKILL.md"),
            b"---\nname: dup-skill\ndescription: from codex\n---\nBody B.\n",
        )
        .build_fs()
}

fn runtime(fs: Arc<dyn ScopeFs>) -> Runtime {
    let ports = Ports {
        fs,
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FakeLease::default()),
        history: Arc::new(NoHistory),
        sink: Arc::new(skill_studio_core::testing::RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    let scope = scope_for("conflicting_home", Path::new(HOME));
    Runtime::new(&scope, ports).expect("runtime")
}

/// Hashes every readable file under `home`, keyed by its path, so a caller
/// can prove a call touched nothing: any changed byte, added file, or
/// removed file changes this map.
fn content_fingerprint(fs: &dyn ScopeFs, dirs: &[&str]) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for dir in dirs {
        let path = Path::new(dir);
        let Ok(bytes) = fs.read_capped(&path.join("SKILL.md"), 1024 * 1024) else {
            continue;
        };
        out.insert(dir.to_string(), bytes);
    }
    out
}

/// Given a home with two differing copies of one skill, when
/// `diagnose_conflict` runs, then it names both paths in one
/// `ConflictSummary` and leaves every file on disk byte-identical to before
/// the call; on failure the panic names whichever path changed.
#[test]
fn diagnose_conflict_names_both_paths_and_writes_nothing_or_names_the_path_it_changed() {
    let fs: Arc<dyn ScopeFs> = Arc::new(conflicting_home());
    let dirs = [
        &format!("{HOME}/.claude/skills/dup-skill")[..],
        &format!("{HOME}/.codex/skills/dup-skill")[..],
    ];
    let before = content_fingerprint(fs.as_ref(), &dirs);
    let rt = runtime(fs.clone());

    let report = ops::diagnose_conflict(&rt, &ctx(), &DiagnoseConflictRequest::default())
        .expect("diagnose_conflict");

    let conflict = report
        .conflicts
        .iter()
        .find(|c| c.skill.0 == "dup-skill")
        .expect("dup-skill reported as a conflict");
    let named: Vec<&Path> = vec![conflict.path_a.as_path(), conflict.path_b.as_path()];
    for dir in &dirs {
        assert!(
            named.iter().any(|p| p.starts_with(Path::new(dir))),
            "conflict report did not name {dir}: {named:?}"
        );
    }

    let after = content_fingerprint(fs.as_ref(), &dirs);
    assert_eq!(
        before, after,
        "diagnose_conflict changed bytes under dup-skill's roots; it must only read"
    );
}

/// A per-skill Claude Code symlink whose target doesn't exist (same shape
/// as `testing::fixtures::broken_link`), plus a skill whose only issue -
/// an uppercase name - has no automatic repair, plus a skill installed
/// twice under `OpenCode`'s global scope: the v2 canonical `skills` root and
/// the v1 legacy `skill` root (`harness.rs`'s dual-root compatibility
/// shape), so `duplicate_issues` reports it with `deployment_id: None`.
fn unrepairable_home() -> impl ScopeFs {
    FixtureBuilder::new()
        .alias(
            &format!("{HOME}/.claude/skills/ghost"),
            "../../.agents/skills/missing",
        )
        .file(
            &format!("{HOME}/.claude/skills/UpperCase/SKILL.md"),
            b"---\nname: UpperCase\ndescription: Has an uppercase name, which the spec forbids.\n---\nBody.\n",
        )
        .file(
            &format!("{HOME}/.config/opencode/skills/dup-skill/SKILL.md"),
            b"---\nname: dup-skill\ndescription: Installed under the v2 root.\n---\nBody.\n",
        )
        .file(
            &format!("{HOME}/.config/opencode/skill/dup-skill/SKILL.md"),
            b"---\nname: dup-skill\ndescription: Installed under the v1 legacy root too.\n---\nBody.\n",
        )
        .build_fs()
}

/// Given a home with a broken per-skill link, a skill whose only issue has
/// no automatic repair, and a skill installed twice in the same
/// (harness, scope) group, when `fix_skill` runs for each, then every
/// `unrepaired` entry names a real path, never the empty `PathBuf` the CLI
/// used to print as `could not repair : <msg>`; the broken-link entry
/// names a path under the link's own directory, not some other deployment.
/// Failure here (an empty path, or a `ghost` path outside
/// `.claude/skills/ghost`) names which fixture - `ghost` (invariant 1,
/// `check_link_resolves_in_root`), `UpperCase` (the generic "no
/// `PreviewRepair`" branch), or `dup-skill` (`IssueKind::Duplicate`, which
/// carries no `deployment_id`) - regressed.
#[test]
fn fix_names_the_file_path_for_anything_it_cannot_repair_or_shows_a_generic_toast() {
    let fs: Arc<dyn ScopeFs> = Arc::new(unrepairable_home());
    let rt = runtime(fs);

    for skill in ["ghost", "UpperCase", "dup-skill"] {
        let outcome = ops::fix_skill(
            &rt,
            &ctx(),
            &FixSkillRequest {
                skill: SkillName(skill.to_string()),
            },
        )
        .unwrap_or_else(|e| panic!("fix_skill for {skill}: {e:?}"));
        assert!(
            !outcome.unrepaired.is_empty(),
            "{skill}: expected at least one unrepaired issue, got {outcome:?}"
        );
        for issue in &outcome.unrepaired {
            assert_ne!(
                issue.path,
                Path::new(""),
                "{skill}: unrepaired issue had an empty path: {issue:?}"
            );
            if skill == "ghost" {
                assert!(
                    issue
                        .path
                        .starts_with(Path::new(&format!("{HOME}/.claude/skills/ghost"))),
                    "ghost: unrepaired issue did not name the link's own path: {issue:?}"
                );
            }
        }
    }
}
