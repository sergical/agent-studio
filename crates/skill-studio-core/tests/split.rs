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

/// Flow: `gamma` is off in Codex through a `[[skills.config]]` row that names
/// the Universal `SKILL.md`. Split it to Claude Code and Codex. Expect the
/// new Codex copy to be off too, and the Claude copy to stay untouched: a
/// split must not turn a skill on that the user switched off. Fails when the
/// Codex copy is on, because Codex keys its rows by path and the row still
/// names the folder the split moved away.
#[test]
fn split_keeps_a_skill_off_in_codex_off_for_the_new_codex_copy_or_names_the_path_left_on() {
    let home = unique_temp_dir("split_codex_off");
    splittable_home(&home);
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(
        home.join(".codex/config.toml"),
        format!(
            "model = \"o3\"\n\n[[skills.config]]\npath = \"{}\"\nenabled = false\n",
            universal(&home).join("SKILL.md").display()
        ),
    )
    .unwrap();
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);

    ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["claude-code", "codex"]),
        },
    )
    .unwrap();

    let fs = RealFs::new();
    let off = ops::codex_disabled_skill_md_paths(&fs, &home.join(".codex"));
    assert!(
        off.contains(&ops::codex_path_form(
            &fs,
            &codex_copy(&home).join("SKILL.md")
        )),
        "the Codex copy must stay off after the split, off paths: {off:?}"
    );
    assert!(
        !off.contains(&ops::codex_path_form(
            &fs,
            &claude_copy(&home).join("SKILL.md")
        )),
        "no row may name the Claude copy: {off:?}"
    );

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

fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Flow: split a skill whose `scripts/check.sh` is 0755, then undo the
/// split. Expect each copy's script and the restored Universal script to
/// stay 0755. Catches a copy or restore that writes files with the default
/// 0644, so the skill's script stops being runnable.
#[test]
fn split_and_its_undo_keep_a_script_executable() {
    use std::os::unix::fs::PermissionsExt;
    let home = unique_temp_dir("split_exec_bits");
    splittable_home(&home);
    let script = universal(&home).join("scripts/check.sh");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
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
    for copy in [claude_copy(&home), codex_copy(&home)] {
        assert_eq!(
            mode_of(&copy.join("scripts/check.sh")),
            0o755,
            "{} lost its executable bits",
            copy.display()
        );
    }

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
        mode_of(&script),
        0o755,
        "the restored Universal script lost its executable bits"
    );

    std::fs::remove_dir_all(&home).ok();
}

fn gamma_deployment_paths(rt: &Runtime) -> Vec<PathBuf> {
    let inventory = ops::scan(rt, &ctx(), &ScanRequest::default()).unwrap();
    let mut paths: Vec<PathBuf> = inventory
        .skills
        .iter()
        .filter(|s| s.name.0 == "gamma")
        .flat_map(|s| s.deployments.iter().map(|d| d.path.clone()))
        .collect();
    paths.sort();
    paths
}

/// Flow: the scope sets `CODEX_HOME` and a custom `OpenCode` root, and the
/// user splits to Codex and `OpenCode`. Expect the next scan to list both
/// copies. Catches a scan that reads only `~/.codex` and `~/.config/opencode`,
/// so the copies split wrote there vanish from the app.
#[test]
fn split_copies_under_codex_home_and_a_custom_opencode_root_show_in_the_scan() {
    let home = unique_temp_dir("split_custom_roots_scan");
    splittable_home(&home);
    let codex_home = home.join("custom-codex");
    let opencode_root = home.join("custom-opencode");
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::create_dir_all(&opencode_root).unwrap();
    let mut scope = RuntimeScope::fixture(&home).with_codex_home(&codex_home);
    scope.opencode_config_root = Some(opencode_root.clone());
    let rt = runtime_with_scope(&home, &scope);
    let deployment_id = universal_deployment_id(&rt);

    ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["codex", "open-code"]),
        },
    )
    .unwrap();

    let mut expected = vec![
        codex_home.join("skills/gamma"),
        opencode_root.join("skills/gamma"),
    ];
    expected.sort();
    assert_eq!(gamma_deployment_paths(&rt), expected);

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: Claude Code links to the Universal folder with a relative target,
/// the user splits to Codex, then undoes it. Expect the Claude link back
/// with the same relative target. Catches an undo that recreates the link
/// as absolute, which breaks when the user moves or syncs their home.
#[test]
fn undo_split_recreates_a_relative_link_as_relative() {
    let home = unique_temp_dir("split_relative_link");
    splittable_home(&home);
    let relative = PathBuf::from("../../.agents/skills/gamma");
    std::fs::remove_file(claude_copy(&home)).unwrap();
    std::os::unix::fs::symlink(&relative, claude_copy(&home)).unwrap();
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);
    let outcome = ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["codex"]),
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

    assert_eq!(std::fs::read_link(claude_copy(&home)).unwrap(), relative);
    assert_eq!(
        std::fs::read(claude_copy(&home).join("SKILL.md")).unwrap(),
        SKILL_MD
    );

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: the Universal folder holds an empty `assets/` folder; the user
/// splits to Codex and undoes it with no edits in between. Expect the undo
/// to pass without force. Catches drift that compares each copy with the
/// Universal fingerprint, which counts a folder the copy never gets.
#[test]
fn undo_split_of_a_skill_with_an_empty_folder_needs_no_force() {
    let home = unique_temp_dir("split_empty_folder");
    splittable_home(&home);
    std::fs::create_dir_all(universal(&home).join("assets")).unwrap();
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);
    let outcome = ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["codex"]),
        },
    )
    .unwrap();

    let result = ops::restore_event(
        &rt,
        &ctx(),
        &RestoreRequest {
            event_id: outcome.event_id,
            force: false,
        },
    );

    assert!(result.is_ok(), "undo reported drift: {result:?}");
    assert!(std::fs::symlink_metadata(codex_copy(&home)).is_err());

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: the Universal folder holds a link (`refs/latest.md`), and the user
/// splits. Expect a refusal that names the link, with no journal row and no
/// backup folder left. Catches a refusal that comes after the backup, which
/// leaves an orphan backup no event points to.
#[test]
fn split_refuses_a_skill_with_a_nested_link_before_its_backup() {
    let home = unique_temp_dir("split_nested_link");
    splittable_home(&home);
    let nested = universal(&home).join("refs/latest.md");
    std::os::unix::fs::symlink("notes.md", &nested).unwrap();
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);

    let err = ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["codex"]),
        },
    )
    .unwrap_err();

    let backups = home.join(".history/backups");
    let left = std::fs::read_dir(&backups).map_or(0, Iterator::count);
    assert_eq!(
        left,
        0,
        "a refused split left a backup in {}",
        backups.display()
    );
    assert_eq!(split_event_count(&rt), 0);
    assert!(
        err.message.contains(&nested.display().to_string()),
        "{}",
        err.message
    );
    assert!(std::fs::symlink_metadata(claude_copy(&home))
        .unwrap()
        .file_type()
        .is_symlink());

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: a regular file sits where the quarantine folder must go, so the
/// split fails after it removed the links and wrote the copies. Expect the
/// error, no copies, both links back, and the Universal folder in place.
/// Catches a split that returns its error but leaves the skill half-split:
/// real copies plus missing links that no undo can reach.
#[test]
fn split_that_fails_part_way_rolls_back_its_copies_and_links() {
    let home = unique_temp_dir("split_rollback");
    splittable_home(&home);
    std::fs::write(home.join(".agents/skills/.skill-studio-quarantine"), b"").unwrap();
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);

    let result = ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["claude-code", "codex"]),
        },
    );

    assert!(result.is_err(), "the split must fail: {result:?}");
    assert!(std::fs::symlink_metadata(codex_copy(&home)).is_err());
    for link in [claude_copy(&home), pi_link(&home)] {
        assert!(
            std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()),
            "{} must be a link again",
            link.display()
        );
    }
    assert_eq!(
        std::fs::read(universal(&home).join("SKILL.md")).unwrap(),
        SKILL_MD
    );

    std::fs::remove_dir_all(&home).ok();
}

fn codex_config_with_universal_off(home: &Path) -> String {
    let text = format!(
        "model = \"o3\"\n\n[[skills.config]]\npath = \"{}\"\nenabled = false\n",
        universal(home).join("SKILL.md").display()
    );
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(home.join(".codex/config.toml"), &text).unwrap();
    text
}

/// Flow: the skill is off in Codex, split carries a disabled row to the
/// Codex copy, then the split is undone. Expect Codex's config back to its
/// pre-split bytes: the Universal row kept, the carried row gone. Catches
/// an undo that removes the copy but leaves a row naming a path that no
/// longer exists, which a later split or install at that path would inherit.
#[test]
fn undo_split_removes_the_codex_row_it_carried_and_keeps_the_universal_row() {
    let home = unique_temp_dir("split_undo_codex_row");
    splittable_home(&home);
    let before = codex_config_with_universal_off(&home);
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
    assert_ne!(
        std::fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
        before,
        "the split must carry a row to the Codex copy"
    );

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
        std::fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
        before
    );

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: the skill is off in Codex and a regular file blocks the quarantine
/// folder, so the split fails after it wrote the Codex copy and its carried
/// row. Expect Codex's config back to its pre-split bytes. Catches a
/// rollback that removes the copy folder but leaves its row behind.
#[test]
fn split_that_fails_part_way_removes_the_codex_row_it_carried() {
    let home = unique_temp_dir("split_rollback_codex_row");
    splittable_home(&home);
    let before = codex_config_with_universal_off(&home);
    std::fs::write(home.join(".agents/skills/.skill-studio-quarantine"), b"").unwrap();
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);

    let result = ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["claude-code", "codex"]),
        },
    );

    assert!(result.is_err(), "the split must fail: {result:?}");
    assert_eq!(
        std::fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
        before
    );

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: the skill is off in Codex and Codex reads it through its own
/// per-skill link into the Universal folder; split keeps Codex. Expect the
/// new Codex copy off. Catches a carried-row check that follows the Codex
/// link to the Universal `SKILL.md`, finds it already off, and adds no row,
/// so the real copy starts on.
#[test]
fn split_keeps_a_skill_off_for_a_codex_copy_that_replaces_a_codex_link() {
    let home = unique_temp_dir("split_codex_link_off");
    splittable_home(&home);
    std::fs::create_dir_all(home.join(".codex/skills")).unwrap();
    std::os::unix::fs::symlink(universal(&home), codex_copy(&home)).unwrap();
    codex_config_with_universal_off(&home);
    let rt = runtime_for(&home);
    let deployment_id = universal_deployment_id(&rt);

    ops::split(
        &rt,
        &ctx(),
        &SplitRequest {
            deployment_id,
            harnesses: harnesses(&["claude-code", "codex"]),
        },
    )
    .unwrap();

    assert!(is_real_dir(&codex_copy(&home)));
    let fs = RealFs::new();
    let off = ops::codex_disabled_skill_md_paths(&fs, &home.join(".codex"));
    assert!(
        off.contains(&ops::codex_path_form(
            &fs,
            &codex_copy(&home).join("SKILL.md")
        )),
        "the Codex copy must stay off after the split, off paths: {off:?}"
    );

    std::fs::remove_dir_all(&home).ok();
}
