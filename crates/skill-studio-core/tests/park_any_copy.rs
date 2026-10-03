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
use skill_studio_core::ports::{Ports, Runtime};
use skill_studio_core::scope::{ProjectSelection, RuntimeScope};
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{FakeClock, FakeIds, RecordingSink};

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
    let mut scope = RuntimeScope::fixture(home);
    scope.projects = ProjectSelection::Explicit {
        paths: projects.to_vec(),
    };
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(
            home.join(".history").join("events.sqlite3"),
        )),
        sink: Arc::new(RecordingSink::default()),
        spawner: Some(Arc::new(RealProcessSpawner::new())),
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
        assert!(check.git_tracked, "git lists the folder, so it is tracked");
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

    assert!(!check.git_tracked);
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

    assert!(check("kept", &tracked));
    assert!(!check("loose", &untracked));
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
