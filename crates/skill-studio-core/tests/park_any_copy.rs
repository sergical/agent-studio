// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

//! Real-disk tests for parking any real copy of a skill (#386): a global
//! agent folder, a project folder, the Universal folder, and the old flat
//! parked layout. Each test names the flow, what must hold, and what a
//! failure means.
//!
//! The git cases run the real `git` binary through the process spawner
//! port; they skip the tracked assertion, with a message, when it is absent.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use skill_studio_core::dto::{
    DeploymentDto, ParkCheckRequest, ParkRequest, ScanRequest, UnparkRequest,
};
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{AgentId, RootKind, RootScope};
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, ProcessSpawner, Runtime, ScopeFs};
use skill_studio_core::scope::{ProjectSelection, RuntimeScope};
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FailingFs, FakeClock, FakeIds, RecordingSink};

use skill_studio_host::{FileLease, RealFs, RealProcessSpawner, SqliteHistoryOpener};

const PARKED_ROOT_RELATIVE: &str = ".agents/skills-parked";

fn write_skill(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: a parkable skill\n---\nBody.\n"),
    )
    .unwrap();
}

fn runtime(home: &Path, projects: &[PathBuf]) -> Runtime {
    runtime_with(
        home,
        projects,
        Arc::new(RealFs::new()),
        Arc::new(RealProcessSpawner::new()),
    )
}

fn runtime_with(
    home: &Path,
    projects: &[PathBuf],
    fs: Arc<dyn ScopeFs>,
    spawner: Arc<dyn ProcessSpawner>,
) -> Runtime {
    let mut scope = RuntimeScope::fixture(home);
    scope.projects = ProjectSelection::Explicit {
        paths: projects.to_vec(),
    };
    let ports = Ports {
        fs,
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(
            home.join(".history").join("events.sqlite3"),
        )),
        sink: Arc::new(RecordingSink::default()),
        spawner: Some(spawner),
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
        telemetry: Arc::new(skill_studio_core::ports::NoopTelemetry),
    };
    Runtime::new(&scope, ports).unwrap()
}

fn deployments(rt: &Runtime, name: &str) -> Vec<DeploymentDto> {
    let inventory = ops::scan(rt, &ctx(), &ScanRequest::default()).unwrap();
    inventory
        .skills
        .into_iter()
        .filter(|skill| skill.name.0 == name)
        .flat_map(|skill| skill.deployments)
        .collect()
}

/// The one live copy under `root_kind` whose path is `path`.
fn live_copy(rt: &Runtime, name: &str, path: &Path) -> DeploymentDto {
    deployments(rt, name)
        .into_iter()
        .find(|d| d.root.kind != RootKind::Parked && d.path == path)
        .unwrap_or_else(|| panic!("no live copy of {name} at {}", path.display()))
}

/// The one parked copy of `name` that came from `origin_kind`.
fn parked_copy(rt: &Runtime, name: &str, origin_kind: &RootKind) -> DeploymentDto {
    deployments(rt, name)
        .into_iter()
        .find(|d| {
            d.root.kind == RootKind::Parked
                && d.parked_origin.as_ref().map(|o| &o.kind) == Some(origin_kind)
        })
        .unwrap_or_else(|| panic!("no parked copy of {name} from {origin_kind:?}"))
}

fn git_present() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn git_in(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// Flow: park then unpark a global Universal copy. Expectation: the copy sits
/// under `skills-parked/universal/`, its origin is the global Universal root,
/// and unpark returns it to `~/.agents/skills`. Failure: the new layout is
/// not used or the copy returns somewhere else.
#[test]
fn global_universal_copy_parks_under_universal_and_unparks_to_the_same_place() {
    let home = unique_temp_dir("park_any_universal");
    let live = home.join(".agents/skills/foo");
    write_skill(&live, "foo");
    let rt = runtime(&home, &[]);

    let outcome = ops::park(
        &rt,
        &ctx(),
        &ParkRequest {
            deployment_id: live_copy(&rt, "foo", &live).id,
        },
    )
    .unwrap();
    assert_eq!(
        outcome.parked_path,
        home.join(PARKED_ROOT_RELATIVE).join("universal/foo")
    );
    assert!(!live.exists());

    let parked = parked_copy(&rt, "foo", &RootKind::Universal);
    assert_eq!(
        parked.parked_origin.as_ref().map(|o| &o.scope),
        Some(&RootScope::Global)
    );
    let restored = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: parked.id,
        },
    )
    .unwrap();
    assert_eq!(restored.restored_path, live);
    assert!(live.join("SKILL.md").exists());
    assert!(!outcome.parked_path.exists());

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park then unpark a real copy in `~/.codex/skills`. Expectation: it
/// parks under `skills-parked/codex/` and unparks to `~/.codex/skills/foo`.
/// Failure: park still refuses a non-Universal copy, or unpark guesses the
/// Universal root.
#[test]
fn global_codex_folder_copy_parks_under_codex_and_unparks_to_the_codex_folder() {
    let home = unique_temp_dir("park_any_codex");
    let live = home.join(".codex/skills/foo");
    write_skill(&live, "foo");
    let rt = runtime(&home, &[]);

    let outcome = ops::park(
        &rt,
        &ctx(),
        &ParkRequest {
            deployment_id: live_copy(&rt, "foo", &live).id,
        },
    )
    .unwrap();
    assert_eq!(
        outcome.parked_path,
        home.join(PARKED_ROOT_RELATIVE).join("codex/foo")
    );

    let codex = RootKind::Harness(AgentId::parse("codex").unwrap());
    let restored = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: parked_copy(&rt, "foo", &codex).id,
        },
    )
    .unwrap();
    assert_eq!(restored.restored_path, live);
    assert!(live.join("SKILL.md").exists());

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: a project `.claude/skills/foo` that git tracks. Expectation:
/// `park_check` says `git_tracked`, park files the copy under
/// `projects/<basename>-<12 hex>/claude-code/foo`, and unpark restores it into
/// the project. Failure: the tracked check misses the repo, the project copy
/// is parked as global, or it returns to the wrong project.
#[test]
fn tracked_project_copy_is_flagged_parks_under_its_project_key_and_unparks_into_the_project() {
    let home = unique_temp_dir("park_any_project_git");
    let project = home.join("work/app");
    let live = project.join(".claude/skills/foo");
    write_skill(&live, "foo");
    let have_git = git_present();
    if have_git {
        git_in(&project, &["init", "-q"]);
        git_in(&project, &["add", ".claude/skills/foo/SKILL.md"]);
    } else {
        eprintln!("git is not installed: skipping the git_tracked assertion");
    }
    let rt = runtime(&home, std::slice::from_ref(&project));
    let copy = live_copy(&rt, "foo", &live);

    let check = ops::park_check(
        &rt,
        &ctx(),
        &ParkCheckRequest {
            deployment_id: copy.id.clone(),
        },
    )
    .unwrap();
    if have_git {
        assert_eq!(
            check.git_tracked,
            Some(true),
            "git lists the folder, so it is tracked"
        );
    }
    assert_eq!(check.project.as_deref(), Some(project.as_path()));

    let outcome = ops::park(
        &rt,
        &ctx(),
        &ParkRequest {
            deployment_id: copy.id,
        },
    )
    .unwrap();
    let key_dir = outcome
        .parked_path
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf();
    let key = key_dir.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        key.starts_with("app-") && key.len() == "app-".len() + 12,
        "unexpected project key {key}"
    );
    assert_eq!(
        outcome.parked_path,
        home.join(PARKED_ROOT_RELATIVE)
            .join("projects")
            .join(&key)
            .join("claude-code/foo")
    );
    assert!(!live.exists());

    let claude = RootKind::Harness(AgentId::parse("claude-code").unwrap());
    let parked = parked_copy(&rt, "foo", &claude);
    assert_eq!(
        parked
            .parked_origin
            .as_ref()
            .map(|o| matches!(o.scope, RootScope::Project(_))),
        Some(true),
        "the scan must report the project the copy came from"
    );
    let restored = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: parked.id,
        },
    )
    .unwrap();
    assert_eq!(restored.restored_path, live);
    assert!(live.join("SKILL.md").exists());

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: a project copy outside any git repository. Expectation:
/// `park_check` says `git_tracked: false`. Failure: a non-repo folder is
/// reported as tracked, and the confirm would warn for nothing.
#[test]
fn project_copy_outside_git_is_not_flagged_as_tracked() {
    let home = unique_temp_dir("park_any_project_plain");
    let project = home.join("work/plain");
    let live = project.join(".agents/skills/foo");
    write_skill(&live, "foo");
    let rt = runtime(&home, std::slice::from_ref(&project));

    let check = ops::park_check(
        &rt,
        &ctx(),
        &ParkCheckRequest {
            deployment_id: live_copy(&rt, "foo", &live).id,
        },
    )
    .unwrap();

    assert_eq!(check.git_tracked, Some(false));
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: a repository that tracks one skill and holds another untracked.
/// Expectation: only the tracked one reports `git_tracked`. Failure: the
/// check answers per repository, not per folder.
#[test]
fn untracked_folder_inside_a_repository_is_not_flagged_as_tracked() {
    if !git_present() {
        eprintln!("git is not installed: skipping");
        return;
    }
    let home = unique_temp_dir("park_any_project_untracked");
    let project = home.join("work/app");
    let tracked = project.join(".claude/skills/kept");
    let untracked = project.join(".claude/skills/loose");
    write_skill(&tracked, "kept");
    write_skill(&untracked, "loose");
    git_in(&project, &["init", "-q"]);
    git_in(&project, &["add", ".claude/skills/kept/SKILL.md"]);
    let rt = runtime(&home, std::slice::from_ref(&project));

    let check = |name: &str, path: &Path| {
        ops::park_check(
            &rt,
            &ctx(),
            &ParkCheckRequest {
                deployment_id: live_copy(&rt, name, path).id,
            },
        )
        .unwrap()
        .git_tracked
    };

    assert_eq!(check("kept", &tracked), Some(true));
    assert_eq!(check("loose", &untracked), Some(false));
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: the same skill name parked from the Universal folder and from
/// `~/.codex/skills`. Expectation: both parked copies coexist and each unparks
/// to its own origin. Failure: the second park collides with the first, or
/// an unpark restores to the other origin.
#[test]
fn two_parked_copies_of_one_name_coexist_and_each_unparks_to_its_own_origin() {
    let home = unique_temp_dir("park_any_coexist");
    let universal = home.join(".agents/skills/foo");
    let codex_dir = home.join(".codex/skills/foo");
    write_skill(&universal, "foo");
    write_skill(&codex_dir, "foo");
    let rt = runtime(&home, &[]);

    for path in [&universal, &codex_dir] {
        ops::park(
            &rt,
            &ctx(),
            &ParkRequest {
                deployment_id: live_copy(&rt, "foo", path).id,
            },
        )
        .unwrap();
    }
    assert!(home
        .join(PARKED_ROOT_RELATIVE)
        .join("universal/foo")
        .exists());
    assert!(home.join(PARKED_ROOT_RELATIVE).join("codex/foo").exists());

    let codex = RootKind::Harness(AgentId::parse("codex").unwrap());
    let from_codex = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: parked_copy(&rt, "foo", &codex).id,
        },
    )
    .unwrap();
    assert_eq!(from_codex.restored_path, codex_dir);
    assert!(!universal.exists(), "the Universal copy is still parked");

    let from_universal = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: parked_copy(&rt, "foo", &RootKind::Universal).id,
        },
    )
    .unwrap();
    assert_eq!(from_universal.restored_path, universal);

    std::fs::remove_dir_all(&home).ok();
}

/// Flow: a copy left in the old flat `skills-parked/foo` layout. Expectation:
/// the scan reports it as parked from the Universal root and unpark returns
/// it to `~/.agents/skills/foo`. Failure: the legacy folder is invisible to
/// the scan or goes to the wrong place.
#[test]
fn legacy_flat_parked_copy_is_scanned_and_unparks_to_the_universal_folder() {
    let home = unique_temp_dir("park_any_legacy");
    write_skill(&home.join(PARKED_ROOT_RELATIVE).join("foo"), "foo");
    let rt = runtime(&home, &[]);

    let parked = parked_copy(&rt, "foo", &RootKind::Universal);
    let restored = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: parked.id,
        },
    )
    .unwrap();

    assert_eq!(restored.restored_path, home.join(".agents/skills/foo"));
    assert!(home.join(".agents/skills/foo/SKILL.md").exists());
    assert!(!home.join(PARKED_ROOT_RELATIVE).join("foo").exists());
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park a Universal copy, then put a new copy back at its origin and
/// unpark. Expectation: unpark refuses and leaves both folders alone.
/// Failure: unpark overwrites the live copy or deletes the parked one.
#[test]
fn unpark_refuses_when_a_live_copy_already_sits_at_the_origin() {
    let home = unique_temp_dir("park_any_unpark_blocked");
    let live = home.join(".agents/skills/foo");
    write_skill(&live, "foo");
    let rt = runtime(&home, &[]);
    let outcome = ops::park(
        &rt,
        &ctx(),
        &ParkRequest {
            deployment_id: live_copy(&rt, "foo", &live).id,
        },
    )
    .unwrap();
    write_skill(&live, "foo");

    let err = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: parked_copy(&rt, "foo", &RootKind::Universal).id,
        },
    )
    .unwrap_err();

    assert!(err.message.contains("already exists"), "{}", err.message);
    assert!(outcome.parked_path.join("SKILL.md").exists());
    assert!(live.join("SKILL.md").exists());
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park a Universal copy, put a new one back, and park that too.
/// Expectation: the second park refuses because a parked copy already exists
/// for that origin. Failure: the first parked copy is overwritten.
#[test]
fn park_refuses_when_a_parked_copy_already_exists_for_the_same_origin() {
    let home = unique_temp_dir("park_any_park_blocked");
    let live = home.join(".agents/skills/foo");
    write_skill(&live, "foo");
    let rt = runtime(&home, &[]);
    let first = ops::park(
        &rt,
        &ctx(),
        &ParkRequest {
            deployment_id: live_copy(&rt, "foo", &live).id,
        },
    )
    .unwrap();
    write_skill(&live, "foo");

    let err = ops::park(
        &rt,
        &ctx(),
        &ParkRequest {
            deployment_id: live_copy(&rt, "foo", &live).id,
        },
    )
    .unwrap_err();

    assert!(
        err.message
            .contains("parked copy from this folder already exists"),
        "{}",
        err.message
    );
    assert!(live.join("SKILL.md").exists());
    assert!(first.parked_path.join("SKILL.md").exists());
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park a skill that a Claude Code plugin ships. Expectation: refused
/// with the `/plugin` hint. Failure: park moves files out of the plugin cache.
#[test]
fn park_refuses_a_plugin_copy_and_points_at_the_plugin_command() {
    let home = unique_temp_dir("park_any_plugin");
    let root = home.join(".claude/plugins/cache/vendor-1/plugin-1/1.0.0");
    std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    std::fs::write(
        root.join(".claude-plugin/plugin.json"),
        br#"{"name":"plugin-1","version":"1.0.0"}"#,
    )
    .unwrap();
    let shipped = root.join("skills/shipped");
    write_skill(&shipped, "shipped");
    let rt = runtime(&home, &[]);
    let copy = deployments(&rt, "shipped")
        .into_iter()
        .next()
        .expect("the plugin skill is scanned");

    let err = ops::park(
        &rt,
        &ctx(),
        &ParkRequest {
            deployment_id: copy.id,
        },
    )
    .unwrap_err();

    assert!(err.message.contains("/plugin"), "{}", err.message);
    assert!(shipped.join("SKILL.md").exists());
    std::fs::remove_dir_all(&home).ok();
}

/// Forgets every journal row, so an unpark has only the parked folder and
/// its markers to go on.
fn forget_history(home: &Path) {
    std::fs::remove_dir_all(home.join(".history")).unwrap();
}

fn park_copy_at(rt: &Runtime, name: &str, path: &Path) -> PathBuf {
    ops::park(
        rt,
        &ctx(),
        &ParkRequest {
            deployment_id: live_copy(rt, name, path).id,
        },
    )
    .unwrap()
    .parked_path
}

fn only_parked_copy(rt: &Runtime, name: &str) -> DeploymentDto {
    deployments(rt, name)
        .into_iter()
        .find(|d| d.root.kind == RootKind::Parked)
        .unwrap_or_else(|| panic!("no parked copy of {name}"))
}

/// Flow: a Codex-folder copy of `foo` is parked, then a Universal copy of a
/// skill named `codex` is parked. Expectation: the second lands in
/// `universal/codex` and the `codex` slot keeps `foo`; both unpark to their
/// origins. Failure: the skill named like a slot is refused as already parked,
/// or it lands inside the slot.
#[test]
fn universal_skill_named_like_a_slot_parks_beside_that_slot() {
    let home = unique_temp_dir("park_any_slot_name_universal");
    let codex_copy = home.join(".codex/skills/foo");
    let shared = home.join(".agents/skills/codex");
    write_skill(&codex_copy, "foo");
    write_skill(&shared, "codex");
    let rt = runtime(&home, &[]);

    park_copy_at(&rt, "foo", &codex_copy);
    let parked = park_copy_at(&rt, "codex", &shared);

    let parked_root = home.join(PARKED_ROOT_RELATIVE);
    assert_eq!(parked, parked_root.join("universal/codex"));
    assert!(parked_root.join("codex/foo/SKILL.md").exists());
    assert!(!parked_root.join("codex/SKILL.md").exists());
    let restored = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: parked_copy(&rt, "codex", &RootKind::Universal).id,
        },
    )
    .unwrap();
    assert_eq!(restored.restored_path, shared);
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: an old flat `skills-parked/codex` (a skill named `codex`) exists,
/// then a Codex-folder copy of `bar` is parked. Expectation: the old copy
/// moves to `universal/codex` first, `bar` lands in the `codex` slot, and the
/// old copy still unparks to `~/.agents/skills/codex`. Failure: `bar` nests
/// inside the old skill's folder, or the old skill is lost.
#[test]
fn legacy_flat_copy_named_like_a_slot_moves_aside_when_a_codex_copy_parks() {
    let home = unique_temp_dir("park_any_slot_name_legacy");
    let parked_root = home.join(PARKED_ROOT_RELATIVE);
    write_skill(&parked_root.join("codex"), "codex");
    let codex_copy = home.join(".codex/skills/bar");
    write_skill(&codex_copy, "bar");
    let rt = runtime(&home, &[]);

    let parked = park_copy_at(&rt, "bar", &codex_copy);

    assert_eq!(parked, parked_root.join("codex/bar"));
    assert!(parked_root.join("universal/codex/SKILL.md").exists());
    assert!(!parked_root.join("codex/SKILL.md").exists());
    let restored = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: parked_copy(&rt, "codex", &RootKind::Universal).id,
        },
    )
    .unwrap();
    assert_eq!(restored.restored_path, home.join(".agents/skills/codex"));
    assert!(home.join(".agents/skills/codex/SKILL.md").exists());
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park a Codex copy whose rename fails with EXDEV (another volume).
/// Expectation: the folder is copied with its file modes, verified, and the
/// source removed. Failure: park fails on a second volume, or the copy loses
/// the executable bit or content.
#[test]
fn park_across_volumes_copies_verifies_and_removes_the_source() {
    use std::os::unix::fs::PermissionsExt;
    let home = unique_temp_dir("park_any_exdev");
    let live = home.join(".codex/skills/foo");
    write_skill(&live, "foo");
    std::fs::create_dir_all(live.join("scripts")).unwrap();
    std::fs::write(live.join("scripts/run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(
        live.join("scripts/run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let failing = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(
        &home,
        &[],
        failing.clone(),
        Arc::new(RealProcessSpawner::new()),
    );
    let deployment_id = live_copy(&rt, "foo", &live).id;
    failing.fail_next_rename_cross_device();

    let parked = ops::park(&rt, &ctx(), &ParkRequest { deployment_id })
        .unwrap()
        .parked_path;

    assert!(!live.exists(), "the source folder must be gone");
    assert!(parked.join("SKILL.md").exists());
    assert_eq!(
        std::fs::read_to_string(parked.join("scripts/run.sh")).unwrap(),
        "#!/bin/sh\necho hi\n"
    );
    let mode = std::fs::metadata(parked.join("scripts/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park a project copy across volumes, with one file that cannot be
/// read halfway through the copy. Expectation: park fails, the source is
/// untouched, and no parked folder (not even the empty project key folder)
/// is left. Failure: a partial copy or an empty key folder stays behind, or
/// the source loses files.
#[test]
fn failed_cross_volume_park_leaves_the_source_whole_and_no_parked_folders() {
    use std::os::unix::fs::PermissionsExt;
    let home = unique_temp_dir("park_any_exdev_fail");
    let project = home.join("work/app");
    let live = project.join(".claude/skills/foo");
    write_skill(&live, "foo");
    std::fs::write(live.join("z-unreadable.md"), "secret").unwrap();
    std::fs::set_permissions(
        live.join("z-unreadable.md"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let failing = Arc::new(FailingFs::wrap(Arc::new(RealFs::new())));
    let rt = runtime_with(
        &home,
        std::slice::from_ref(&project),
        failing.clone(),
        Arc::new(RealProcessSpawner::new()),
    );
    let deployment_id = live_copy(&rt, "foo", &live).id;
    failing.fail_next_rename_cross_device();

    let result = ops::park(&rt, &ctx(), &ParkRequest { deployment_id });

    std::fs::set_permissions(
        live.join("z-unreadable.md"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(result.is_err(), "an unreadable file must fail the park");
    assert!(live.join("SKILL.md").exists());
    assert!(live.join("z-unreadable.md").exists());
    assert!(
        !home.join(PARKED_ROOT_RELATIVE).exists(),
        "no partial copy or key folder may remain"
    );
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park an `OpenCode` copy, check the `.origin` marker, forget the
/// journal, and unpark. Expectation: the marker holds the exact skills folder
/// the catalog spells (`.config/opencode/skills`) and the copy returns there.
/// Failure: the marker is missing or wrong, so a no-row unpark has to guess
/// the folder. (The singular `skill/` folder is a legacy root and park
/// refuses it, so it never needs a marker.)
#[test]
fn unpark_without_a_journal_row_returns_an_opencode_copy_to_its_own_folder() {
    let home = unique_temp_dir("park_any_opencode_skill");
    let live = home.join(".config/opencode/skills/foo");
    write_skill(&live, "foo");
    let rt = runtime(&home, &[]);
    park_copy_at(&rt, "foo", &live);
    let marker = home
        .join(PARKED_ROOT_RELATIVE)
        .join("open-code/.origin/foo");
    assert_eq!(
        std::fs::read_to_string(marker).unwrap().trim(),
        ".config/opencode/skills"
    );
    forget_history(&home);
    let rt = runtime(&home, &[]);

    let restored = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: only_parked_copy(&rt, "foo").id,
        },
    )
    .unwrap();

    assert_eq!(restored.restored_path, live);
    assert!(live.join("SKILL.md").exists());
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park a project copy, forget the journal, unpark. Expectation: the
/// copy returns into its project through the `.origin` markers. Failure: a
/// project copy with no journal row cannot come back, or comes back global.
#[test]
fn unpark_without_a_journal_row_returns_a_project_copy_through_its_markers() {
    let home = unique_temp_dir("park_any_project_no_row");
    let project = home.join("work/app");
    let live = project.join(".claude/skills/foo");
    write_skill(&live, "foo");
    let projects = [project.clone()];
    let rt = runtime(&home, &projects);
    park_copy_at(&rt, "foo", &live);
    forget_history(&home);
    let rt = runtime(&home, &projects);

    let restored = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: only_parked_copy(&rt, "foo").id,
        },
    )
    .unwrap();

    assert_eq!(restored.restored_path, live);
    assert!(live.join("SKILL.md").exists());
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park a project copy, forget the journal, then unpark with a scope
/// that no longer covers that project. Expectation: refused, nothing moves.
/// Failure: a marker alone writes into a folder outside the scope.
#[test]
fn unpark_without_a_journal_row_refuses_a_project_outside_the_scope() {
    let home = unique_temp_dir("park_any_project_unknown");
    let project = home.join("work/app");
    let live = project.join(".claude/skills/foo");
    write_skill(&live, "foo");
    let rt = runtime(&home, std::slice::from_ref(&project));
    let parked = park_copy_at(&rt, "foo", &live);
    forget_history(&home);
    let rt = runtime(&home, &[]);

    let result = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: only_parked_copy(&rt, "foo").id,
        },
    );

    assert!(
        result.is_err(),
        "a project the scope does not know must be refused"
    );
    assert!(parked.join("SKILL.md").exists());
    assert!(!live.exists());
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park a Codex copy, forget the journal, rewrite its `.origin` marker
/// to a path outside the catalog, and unpark. Expectation: refused, the
/// parked copy stays. Failure: a hand-edited marker steers the copy to any
/// folder it names.
#[test]
fn unpark_without_a_journal_row_refuses_a_marker_that_names_an_unknown_folder() {
    let home = unique_temp_dir("park_any_marker_evil");
    let live = home.join(".codex/skills/foo");
    write_skill(&live, "foo");
    let rt = runtime(&home, &[]);
    let parked = park_copy_at(&rt, "foo", &live);
    forget_history(&home);
    std::fs::write(
        home.join(PARKED_ROOT_RELATIVE).join("codex/.origin/foo"),
        "../../../elsewhere",
    )
    .unwrap();
    let rt = runtime(&home, &[]);

    let result = ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: only_parked_copy(&rt, "foo").id,
        },
    );

    assert!(
        result.is_err(),
        "a marker naming an unknown folder must be refused"
    );
    assert!(parked.join("SKILL.md").exists());
    assert!(!home.parent().unwrap().join("elsewhere").exists());
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: `~/.claude/skills/foo` links to the real copy in
/// `~/.codex/skills/foo`; park the Codex copy, then unpark. Expectation: park
/// removes the link, unpark puts it back and it resolves. Failure: the link
/// dangles after park, or is lost after unpark.
#[test]
fn links_to_a_parked_agent_folder_copy_are_removed_and_recreated() {
    let home = unique_temp_dir("park_any_agent_link");
    let live = home.join(".codex/skills/foo");
    write_skill(&live, "foo");
    let link = home.join(".claude/skills/foo");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("../../.codex/skills/foo", &link).unwrap();
    let rt = runtime(&home, &[]);

    park_copy_at(&rt, "foo", &live);
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "the link must not dangle after park"
    );
    ops::unpark(
        &rt,
        &ctx(),
        &UnparkRequest {
            deployment_id: only_parked_copy(&rt, "foo").id,
        },
    )
    .unwrap();

    assert!(live.join("SKILL.md").exists());
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(
        link.join("SKILL.md").exists(),
        "the link must resolve again"
    );
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: park the same project copy once with the project named by a
/// symlink, and once by its real path. Expectation: both land under one
/// `projects/<key>` folder. Failure: the key follows the spelling of the
/// path, so one project gets two parked folders.
#[test]
fn project_key_is_the_same_for_a_symlinked_and_a_real_project_path() {
    let home = unique_temp_dir("park_any_project_key");
    let project = home.join("work/app");
    let alias = home.join("work/app-alias");
    let live = project.join(".claude/skills/foo");
    write_skill(&live, "foo");
    std::os::unix::fs::symlink(&project, &alias).unwrap();

    let via_alias = runtime(&home, std::slice::from_ref(&alias));
    let alias_live = alias.join(".claude/skills/foo");
    let first = park_copy_at(&via_alias, "foo", &alias_live);
    ops::unpark(
        &via_alias,
        &ctx(),
        &UnparkRequest {
            deployment_id: only_parked_copy(&via_alias, "foo").id,
        },
    )
    .unwrap();
    // A new runtime restarts the fake event ids, so it needs a fresh journal.
    forget_history(&home);
    let via_real = runtime(&home, std::slice::from_ref(&project));
    let second = park_copy_at(&via_real, "foo", &live);

    let key_dir = |parked: &Path| parked.parent().unwrap().parent().unwrap().to_path_buf();
    assert_eq!(key_dir(&first), key_dir(&second));
    let keys = std::fs::read_dir(home.join(PARKED_ROOT_RELATIVE).join("projects"))
        .unwrap()
        .count();
    assert_eq!(keys, 1);
    std::fs::remove_dir_all(&home).ok();
}

/// Flow: `park_check` on a macOS machine whose command line tools are not
/// installed, where `git` is a stub that opens an install dialog.
/// Expectation: `git_tracked` is unknown (`None`) and git is not run.
/// Failure: the stub runs, or the answer claims `false` for a repo it never
/// looked at.
#[cfg(target_os = "macos")]
#[test]
fn park_check_reports_unknown_when_the_command_line_tools_are_missing() {
    use skill_studio_core::testing::FakeProcessSpawner;
    let home = unique_temp_dir("park_any_no_clt");
    let live = home.join(".codex/skills/foo");
    write_skill(&live, "foo");
    let mut spawner = FakeProcessSpawner::default();
    spawner
        .outputs
        .insert("xcode-select".to_string(), (String::new(), 2));
    let rt = runtime_with(&home, &[], Arc::new(RealFs::new()), Arc::new(spawner));

    let check = ops::park_check(
        &rt,
        &ctx(),
        &ParkCheckRequest {
            deployment_id: live_copy(&rt, "foo", &live).id,
        },
    )
    .unwrap();

    assert_eq!(check.git_tracked, None);
    std::fs::remove_dir_all(&home).ok();
}
