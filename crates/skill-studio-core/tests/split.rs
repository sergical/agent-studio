// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Real-disk integration tests for `ops::split` and its undo through
//! `ops::restore_event`. Real adapters, because the op renames folders and
//! removes links that the in-memory `FixtureFs` cannot stand in for.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_studio_core::dto::{ListEventsRequest, RestoreRequest, ScanRequest, SplitRequest};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{AgentId, BackingRelationship, DeploymentId, RootKind};
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::scope::RuntimeScope;
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

const SKILL_MD: &[u8] = b"---\nname: gamma\ndescription: a skill to split\n---\nBody.\n";

fn universal(home: &Path) -> PathBuf {
    home.join(".agents/skills/gamma")
}
fn claude_copy(home: &Path) -> PathBuf {
    home.join(".claude/skills/gamma")
}
fn codex_copy(home: &Path) -> PathBuf {
    home.join(".codex/skills/gamma")
}
fn pi_link(home: &Path) -> PathBuf {
    home.join(".pi/agent/skills/gamma")
}

/// A home with one Universal skill `gamma` (SKILL.md plus a nested file),
/// linked per skill from Claude Code and pi.
fn splittable_home(home: &Path) {
    let dir = universal(home);
    std::fs::create_dir_all(dir.join("refs")).unwrap();
    std::fs::write(dir.join("SKILL.md"), SKILL_MD).unwrap();
    std::fs::write(dir.join("refs/notes.md"), b"notes\n").unwrap();
    for link in [claude_copy(home), pi_link(home)] {
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&dir, &link).unwrap();
    }
}

fn runtime_with_scope(home: &Path, scope: &RuntimeScope) -> Runtime {
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
        telemetry: Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    Runtime::new(scope, ports).unwrap()
}

fn runtime_for(home: &Path) -> Runtime {
    runtime_with_scope(home, &RuntimeScope::fixture(home))
}

fn universal_deployment_id(rt: &Runtime) -> DeploymentId {
    let inventory = ops::scan(rt, &ctx(), &ScanRequest::default()).unwrap();
    inventory
        .skills
        .iter()
        .find(|s| s.name.0 == "gamma")
        .unwrap()
        .deployments
        .iter()
        .find(|d| d.root.kind == RootKind::Universal)
        .unwrap()
        .id
        .clone()
}

fn harnesses(ids: &[&str]) -> Vec<AgentId> {
    ids.iter().map(|id| AgentId::parse(id).unwrap()).collect()
}

fn split_event_count(rt: &Runtime) -> usize {
    ops::list_events(rt, &ctx(), &ListEventsRequest::default())
        .unwrap()
        .iter()
        .filter(|e| e.kind == "split")
        .count()
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}

/// Flow: split a Universal skill linked from Claude Code and pi, keeping
/// Claude Code and Codex. Expect two real folders with the same files, no
/// Universal folder, no pi link, and a scan that shows only those two
/// copies. Catches a split that leaves links or the Universal folder behind
/// (the skill would stay visible to harnesses the user dropped), or writes a
/// link instead of a real copy.
#[test]
fn split_to_claude_and_codex_leaves_only_two_real_copies_in_the_scan() {
    let home = unique_temp_dir("split_claude_codex");
    splittable_home(&home);
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);

    let outcome = ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["claude-code", "codex"]),
        },
    )
    .unwrap();

    assert!(
        is_real_dir(&claude_copy(&home)),
        "Claude copy must be a real folder"
    );
    assert!(
        is_real_dir(&codex_copy(&home)),
        "Codex copy must be a real folder"
    );
    for copy in [claude_copy(&home), codex_copy(&home)] {
        assert_eq!(std::fs::read(copy.join("SKILL.md")).unwrap(), SKILL_MD);
        assert_eq!(
            std::fs::read(copy.join("refs/notes.md")).unwrap(),
            b"notes\n"
        );
    }
    assert!(std::fs::symlink_metadata(universal(&home)).is_err());
    assert!(std::fs::symlink_metadata(pi_link(&home)).is_err());
    assert!(outcome.update_note.contains("npx skills update"));
    assert!(outcome.removed_links.contains(&pi_link(&home)));

    let inventory = ops::scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
    let gamma = inventory
        .skills
        .iter()
        .find(|s| s.name.0 == "gamma")
        .unwrap();
    let mut paths: Vec<PathBuf> = gamma
        .deployments
        .iter()
        .map(|d| std::fs::canonicalize(&d.path).unwrap())
        .collect();
    paths.sort();
    let mut expected = vec![
        std::fs::canonicalize(claude_copy(&home)).unwrap(),
        std::fs::canonicalize(codex_copy(&home)).unwrap(),
    ];
    expected.sort();
    assert_eq!(paths, expected, "scan must show only the two copies");
    assert!(gamma
        .deployments
        .iter()
        .all(|d| d.backing == BackingRelationship::Independent));

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: split, then undo the split event. Expect the Universal folder and
/// both links back, and the copies gone. Catches an undo that writes the
/// Universal folder back but leaves the copies (the Claude copy would block
/// its own link from coming back).
#[test]
fn undo_split_restores_universal_and_links_and_removes_the_copies() {
    let home = unique_temp_dir("split_undo");
    splittable_home(&home);
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);
    let outcome = ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["claude-code", "codex"]),
        },
    )
    .unwrap();

    ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id,
            force: false,
        },
    )
    .unwrap();

    assert_eq!(
        std::fs::read(universal(&home).join("SKILL.md")).unwrap(),
        SKILL_MD
    );
    assert_eq!(
        std::fs::read(universal(&home).join("refs/notes.md")).unwrap(),
        b"notes\n"
    );
    for link in [claude_copy(&home), pi_link(&home)] {
        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "{} must be a link again",
            link.display()
        );
        assert_eq!(
            std::fs::canonicalize(&link).unwrap(),
            std::fs::canonicalize(universal(&home)).unwrap()
        );
    }
    assert!(std::fs::symlink_metadata(codex_copy(&home)).is_err());

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: Claude Code's whole skills folder is a link to `.agents/skills`,
/// and the user keeps Claude Code. Expect a refusal that names the
/// "Convert to per-skill links…" action and the `dotagents sync` warning,
/// with nothing written and no journal row. Catches a split that would
/// write the Claude copy into the very folder it then removes.
#[test]
fn split_refuses_a_whole_folder_claude_link_before_any_write() {
    let home = unique_temp_dir("split_whole_folder");
    let dir = universal(&home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), SKILL_MD).unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::os::unix::fs::symlink(home.join(".agents/skills"), home.join(".claude/skills")).unwrap();
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);

    let err = ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["codex", "claude-code"]),
        },
    )
    .unwrap_err();

    assert!(
        err.message.contains("Convert to per-skill links"),
        "{}",
        err.message
    );
    assert!(err.message.contains("dotagents sync"), "{}", err.message);
    assert_eq!(std::fs::read(dir.join("SKILL.md")).unwrap(), SKILL_MD);
    assert!(std::fs::symlink_metadata(codex_copy(&home)).is_err());
    assert_eq!(split_event_count(&rt), 0);

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: Codex already has its own unrelated `gamma` folder. Expect a
/// refusal that names that path, with the Universal folder, the links, and
/// the Codex folder untouched and no journal row. Catches a split that
/// overwrites a user's own copy.
#[test]
fn split_refuses_a_name_clash_before_any_write() {
    let home = unique_temp_dir("split_clash");
    splittable_home(&home);
    std::fs::create_dir_all(codex_copy(&home)).unwrap();
    std::fs::write(codex_copy(&home).join("SKILL.md"), b"mine\n").unwrap();
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);

    let err = ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["claude-code", "codex"]),
        },
    )
    .unwrap_err();

    assert!(
        err.message
            .contains(&codex_copy(&home).display().to_string()),
        "{}",
        err.message
    );
    assert_eq!(
        std::fs::read(codex_copy(&home).join("SKILL.md")).unwrap(),
        b"mine\n"
    );
    assert!(std::fs::symlink_metadata(claude_copy(&home))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(std::fs::symlink_metadata(pi_link(&home)).is_ok());
    assert!(universal(&home).join("SKILL.md").exists());
    assert_eq!(split_event_count(&rt), 0);

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: the scope sets a custom `OpenCode` config root, and the user keeps
/// only `OpenCode`. Expect the copy under that root's `skills`, not under
/// `~/.config/opencode`. Catches a hard-coded `OpenCode` path that writes a
/// copy `OpenCode` never reads.
#[test]
fn split_opencode_copy_follows_the_configured_opencode_root() {
    let home = unique_temp_dir("split_opencode_root");
    splittable_home(&home);
    let custom_root = home.join("custom-opencode");
    std::fs::create_dir_all(&custom_root).unwrap();
    let mut scope = RuntimeScope::fixture(&home);
    scope.opencode_config_root = Some(custom_root.clone());
    let rt = runtime_with_scope(&home, &scope);
    let deployment_id = universal_deployment_id(&rt);

    let outcome = ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["open-code"]),
        },
    )
    .unwrap();

    let expected = custom_root.join("skills/gamma");
    assert_eq!(outcome.copies.len(), 1);
    assert_eq!(outcome.copies[0].path, expected);
    assert_eq!(std::fs::read(expected.join("SKILL.md")).unwrap(), SKILL_MD);
    assert!(std::fs::symlink_metadata(home.join(".config/opencode/skills/gamma")).is_err());

    std::fs::remove_dir_all(&home).ok();
}
