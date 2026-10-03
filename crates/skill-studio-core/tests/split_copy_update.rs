#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk tests for `ops::update_split_copies`: one fetched version
//! written to every live copy `ops::split` made.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_studio_core::dto::{InstallFile, ScanRequest, SplitRequest};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{AgentId, RootKind, RootScope, SkillName};
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime, ScopeFs};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FailingFs, FakeClock, FakeIds, RecordingSink};
use skill_studio_core::SplitCopiesUpdate;

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

const HARNESS_DIRS: [&str; 3] = [".claude/skills", ".codex/skills", ".pi/agent/skills"];

fn copy_of(home: &Path, dir: &str) -> PathBuf {
    home.join(dir).join("gamma")
}

fn skill_md(revision: &str) -> Vec<u8> {
    format!("---\nname: gamma\ndescription: a skill to split\n---\nBody at {revision}.\n")
        .into_bytes()
}

fn runtime_with(home: &Path, fs: Arc<dyn ScopeFs>) -> Runtime {
    let ports = Ports {
        fs,
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
        telemetry: Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    Runtime::new(&RuntimeScope::fixture(home), ports).unwrap()
}

/// A home whose Universal skill `gamma` (at revision v1) was split into
/// Claude Code, Codex and pi copies.
fn split_home(name: &str) -> PathBuf {
    let home = unique_temp_dir(name);
    let dir = home.join(".agents/skills/gamma");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), skill_md("v1")).unwrap();
    let rt = runtime_with(&home, Arc::new(RealFs::new()));
    let id = ops::scan(&rt, &ctx(), &ScanRequest::default())
        .unwrap()
        .skills
        .iter()
        .find(|s| s.name.0 == "gamma")
        .unwrap()
        .deployments
        .iter()
        .find(|d| d.root.kind == RootKind::Universal)
        .unwrap()
        .id
        .clone();
    ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id: id,
            harnesses: ["claude-code", "codex", "pi"]
                .iter()
                .map(|h| AgentId::parse(h).unwrap())
                .collect(),
        },
    )
    .unwrap();
    home
}

fn v2() -> SplitCopiesUpdate {
    SplitCopiesUpdate {
        skill: SkillName("gamma".to_string()),
        scope: RootScope::Global,
        files: vec![InstallFile {
            relative_path: PathBuf::from("SKILL.md"),
            contents: skill_md("v2"),
            mode: None,
        }],
    }
}

/// `v1` or `v2` when the copy holds one whole version, `None` when it is
/// missing or holds neither.
fn revision_of(path: &Path) -> Option<&'static str> {
    let text = std::fs::read_to_string(path.join("SKILL.md")).ok()?;
    if text.contains("Body at v2") {
        Some("v2")
    } else if text.contains("Body at v1") {
        Some("v1")
    } else {
        None
    }
}

/// Flow: split into three copies, then update from one fetched version.
/// Expect all three at v2 and no shared `~/.agents/skills/gamma`. Catches an
/// update that skips a copy or recreates the shared folder.
#[test]
fn update_writes_the_new_version_to_all_three_split_copies_and_no_shared_folder() {
    let home = split_home("split_update_all");
    let rt = runtime_with(&home, Arc::new(RealFs::new()));

    let outcome = ops::update_split_copies(&rt, &ctx(), &v2()).unwrap();

    assert_eq!(outcome.updated.len(), 3, "{outcome:?}");
    for dir in HARNESS_DIRS {
        assert_eq!(revision_of(&copy_of(&home, dir)), Some("v2"), "{dir}");
    }
    assert!(!home.join(".agents/skills/gamma").exists());
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park the Codex copy (its folder moves away), then update. Expect
/// the parked folder unchanged and nothing recreated at the Codex place,
/// the other two at v2. Catches an update that writes into a parked copy's
/// old place and turns the agent back on.
#[test]
fn update_leaves_a_parked_split_copy_untouched() {
    let home = split_home("split_update_parked");
    let parked = home.join(".agents/skills-parked/universal/gamma");
    std::fs::create_dir_all(parked.parent().unwrap()).unwrap();
    std::fs::rename(copy_of(&home, ".codex/skills"), &parked).unwrap();
    let rt = runtime_with(&home, Arc::new(RealFs::new()));

    let outcome = ops::update_split_copies(&rt, &ctx(), &v2()).unwrap();

    assert_eq!(outcome.updated.len(), 2, "{outcome:?}");
    assert_eq!(revision_of(&parked), Some("v1"));
    assert!(!copy_of(&home, ".codex/skills").exists());
    assert_eq!(revision_of(&copy_of(&home, ".claude/skills")), Some("v2"));
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: edit the pi copy after the split, then update. Expect the pi copy
/// refused with a reason and keeping its edit, the other two at v2. Catches
/// an update that overwrites a user's local edits.
#[test]
fn update_refuses_a_split_copy_with_local_edits_and_updates_the_rest() {
    let home = split_home("split_update_edited");
    let pi = copy_of(&home, ".pi/agent/skills");
    std::fs::write(
        pi.join("SKILL.md"),
        b"---\nname: gamma\ndescription: mine\n---\nEdited.\n",
    )
    .unwrap();
    let rt = runtime_with(&home, Arc::new(RealFs::new()));

    let outcome = ops::update_split_copies(&rt, &ctx(), &v2()).unwrap();

    assert_eq!(outcome.updated.len(), 2, "{outcome:?}");
    assert_eq!(outcome.refused.len(), 1, "{outcome:?}");
    assert_eq!(outcome.refused[0].0, pi);
    assert!(std::fs::read_to_string(pi.join("SKILL.md"))
        .unwrap()
        .contains("Edited."));
    assert_eq!(revision_of(&copy_of(&home, ".claude/skills")), Some("v2"));
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: a rename fails at each step of the three-copy update. Expect every
/// copy to hold wholly v1 or wholly v2 (never missing or mixed), and a
/// second update to bring all three to v2. Catches a failure that leaves a
/// copy half-written or blocks the retry.
#[test]
fn update_failing_at_any_step_leaves_each_copy_wholly_old_or_new_and_a_retry_finishes() {
    for n in 1..=14 {
        let home = split_home(&format!("split_update_fail_{n}"));
        let failing = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
        let rt = runtime_with(&home, failing.clone());
        failing.fail_nth_fsops_rename(n);

        let result = ops::update_split_copies(&rt, &ctx(), &v2());

        for dir in HARNESS_DIRS {
            assert!(
                revision_of(&copy_of(&home, dir)).is_some(),
                "step {n}: {dir} is missing or mixed after {result:?}"
            );
        }
        let retry = runtime_with(&home, Arc::new(RealFs::new()));
        ops::update_split_copies(&retry, &ctx(), &v2()).unwrap();
        for dir in HARNESS_DIRS {
            assert_eq!(
                revision_of(&copy_of(&home, dir)),
                Some("v2"),
                "step {n} {dir}"
            );
        }
        std::fs::remove_dir_all(&home).ok();
    }
}
