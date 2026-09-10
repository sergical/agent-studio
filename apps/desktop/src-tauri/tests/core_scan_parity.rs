//! Pins core `ops::scan`'s deployment classification (owner kind, ambiguity
//! carve-outs, destination/backing/mutability derivation) to concrete
//! expected values, run over every fixture in
//! `skill_studio_core::testing::fixtures::all`.
//!
//! The desktop no longer has its own scanner to compare against: it now
//! assembles its `InstalledSkill`s from this same core scan
//! (`skill_refresh::core_scan_installed_skills`). `run_desktop` (still
//! present below, for `desktop_assembly_matches_core_scan_for_every_fixture`) is
//! therefore no longer "the desktop's independent scan" - it is core's scan
//! run through the desktop's assembly layer (`skill_assembly.rs`), and
//! comparing its projection to core's raw scan output checks that assembly
//! is a faithful projection, not that two scanners agree. Every other test
//! in this file asserts core's own scan output against a literal expected
//! value instead of a desktop comparison, since that comparison is now a
//! tautology (both sides run the same scan).
//!
//! `build_snapshot` and `skill_refresh::BuildPaths` were made `pub` (from
//! crate-private) so this test - a separate crate, like any other consumer
//! of `skill_studio_lib`'s public API - can call the desktop's real
//! assembly path directly against a fixture home, rather than
//! reimplementing its wiring. No behavior changed, only visibility.
//!
//! `build_snapshot` takes `home` explicitly for every source except the
//! skill lock file (`lock_file::read_lock_file()` resolves
//! `dirs::home_dir()` internally); this test sets the process `HOME` env var
//! to the fixture home around each call so that source agrees too, and runs
//! every fixture from one `#[test]` function (serially - env vars are
//! process-global) as the desktop's own `skill_refresh` tests already do.
//!
//! Both scans are projected to one comparison shape (skill name -> sorted
//! deployment rows) and diffed, including `destination`, `backing`,
//! `mutability`, `owner_kind`, `owner_id`, and the exact `link_target`
//! string. Documented exclusions, each mapped to a
//! `docs/spec-core-primitives.md` section 9 row:
//!
//! - `backing`'s payload: the desktop's `LinkedTo` carries a
//!   `deployment_id: String` pointing at its canonical deployment; core's
//!   `BackingRelationship::LinkedTo` has no payload. Compared here only by
//!   variant name (`desktop_backing_str`/`core_backing_str`), not the id
//!   inside it. Section 9: "core `BackingRelationship::LinkedTo` carries no
//!   `deployment_id` payload".
//! - `plugin.marketplace`: core's `PluginSourceDto` names the plugin
//!   cache's marketplace directory; the desktop's `PluginInfo` has no
//!   equivalent field (`name`, `version`, `harness` only). Compared here
//!   only through `plugin`/`version`. Section 9: "core `plugin.marketplace`
//!   has no desktop wire equivalent (PR 2)".
//! - Token counts, `folder_bytes`, trial/invocation overlays: the desktop's
//!   `InstalledSkill` carries these; core's `ops::scan` (PR 2) doesn't
//!   compute them yet (PR 5). Not part of the deployment-row comparison
//!   shape, so nothing to exclude here.
//! - `oversize_skill_md`: core caps a `SKILL.md` read at 2 MiB
//!   (`ops::SKILL_MD_MAX_BYTES`) and truncates and keeps the skill rather
//!   than dropping it, via `ScopeFs::read_prefix`, with the truncation
//!   recorded as an `Observation`, not part of the deployment-row
//!   comparison shape. No exclusion needed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use skill_studio_core::dto::ScanRequest;
use skill_studio_core::harness::HarnessCatalog;
use skill_studio_core::identity::{
    CorrelationId, DeploymentMutability, LifecycleOwnerKind, RootScope, SourceKind,
};
use skill_studio_core::ops::scan;
use skill_studio_core::ports::{OpContext, Ports, Runtime};
use skill_studio_core::scope::{ProjectSelection, RuntimeScope};
use skill_studio_core::testing::golden::unique_temp_dir;
use skill_studio_core::testing::{fixtures, FakeClock, FakeIds, FakeLease, NoHistory};

use skill_studio_host::RealFs;

use skill_studio_lib::skill_dto::InstalledSkill;
use skill_studio_lib::skill_invocations::SkillInvocationIndex;
use skill_studio_lib::skill_refresh::{build_snapshot, BuildPaths};

/// One agent used by these fixtures, mapped from the desktop's display-name
/// `Deployment.agent` to a core-style `harness` id (`AgentId`'s invariant:
/// `open-code` is the shared wire name; `opencode` is only its CLI binary
/// name, never a harness id). `None` covers the universal-root
/// compatibility label `"shared"` and the parked-root label `"parked"`
/// (both harnessless roots in core, matching `RootRef::harness == None`).
fn harness_id_for_agent_label(agent: &str) -> Option<&'static str> {
    match agent {
        "Claude Code" => Some("claude-code"),
        "Codex" => Some("codex"),
        "OpenCode" => Some("open-code"),
        "shared" | "parked" => None,
        other => panic!("fixture used an agent this parity test doesn't map: {other}"),
    }
}

fn normalize_path(home: &Path, path: &str) -> String {
    match path.strip_prefix(&*home.to_string_lossy()) {
        Some(rest) => format!("$HOME{rest}"),
        None => path.to_string(),
    }
}

fn desktop_scope_str(home: &Path, agent_scope: &str, project_path: Option<&str>) -> String {
    match agent_scope {
        "project" => {
            let project = project_path.expect("project-scoped deployment has a project_path");
            format!("project:{}", normalize_path(home, project))
        }
        _ => "global".to_string(),
    }
}

/// Discriminant only: the desktop's `LinkedTo` carries a `deployment_id`
/// payload core's simpler `BackingRelationship` doesn't have room for (see
/// the header), so the comparison only checks which variant each side
/// picked, not the id inside it.
fn desktop_backing_str(
    backing: &skill_studio_lib::skill_deployment::BackingRelationship,
) -> &'static str {
    use skill_studio_lib::skill_deployment::BackingRelationship as B;
    match backing {
        B::Canonical => "canonical",
        B::LinkedTo { .. } => "linked_to",
        B::Independent => "independent",
    }
}

fn core_backing_str(backing: &skill_studio_core::identity::BackingRelationship) -> &'static str {
    use skill_studio_core::identity::BackingRelationship as B;
    match backing {
        B::Canonical => "canonical",
        B::LinkedTo => "linked_to",
        B::Independent => "independent",
    }
}

fn desktop_mutability_str(
    mutability: skill_studio_lib::skill_deployment::DeploymentMutability,
) -> &'static str {
    use skill_studio_lib::skill_deployment::DeploymentMutability as M;
    match mutability {
        M::Mutable => "mutable",
        M::ReadOnly => "read_only",
    }
}

fn core_scope_str(home: &Path, scope: &RootScope) -> String {
    match scope {
        RootScope::Global => "global".to_string(),
        RootScope::Project(project) => {
            format!(
                "project:{}",
                normalize_path(home, &project.0.to_string_lossy())
            )
        }
    }
}

/// Projects the desktop's `InstalledSkill`s to the comparison shape: skill
/// name -> deployment rows, sorted by `path` so the two sides line up
/// regardless of scan order.
fn project_desktop(skills: &[InstalledSkill], home: &Path) -> BTreeMap<String, Vec<Value>> {
    let mut out = BTreeMap::new();
    for skill in skills {
        let mut rows: Vec<Value> = skill
            .deployments
            .iter()
            .map(|d| {
                let broken = d.symlink_is_broken;
                json!({
                    "harness": harness_id_for_agent_label(&d.agent),
                    "scope": desktop_scope_str(home, &d.scope, d.project_path.as_deref()),
                    "path": normalize_path(home, &d.path),
                    "is_link": d.is_symlink,
                    "broken": broken,
                    "destination": d.destination.as_str().replace('-', "_"),
                    "backing": desktop_backing_str(&d.backing),
                    "mutability": desktop_mutability_str(d.mutability),
                    "owner_kind": d.owner_kind.as_str().replace('-', "_"),
                    "owner_id": d.owner_id,
                    "link_target": d.symlink_target.as_ref().map(|p| normalize_path(home, p)),
                    "disabled_by": d.disabled_by.map(|b| vec![format!("{b:?}")]).unwrap_or_default(),
                    "spec_violations": d.spec_violations,
                    "plugin": d.plugin.as_ref().map(|p| json!({"plugin": p.name, "version": p.version})),
                    "shared_via_whole_dir_link": d.shared_via_whole_dir_link,
                    "fingerprint": if d.content_hash.is_empty() { None } else { Some(format!("sha256:{}", d.content_hash)) },
                })
            })
            .collect();
        rows.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        out.insert(skill.name.clone(), rows);
    }
    out
}

fn project_core(
    inventory: &skill_studio_core::dto::Inventory,
    home: &Path,
) -> BTreeMap<String, Vec<Value>> {
    let mut out = BTreeMap::new();
    for skill in &inventory.skills {
        let mut rows: Vec<Value> = skill
            .deployments
            .iter()
            .map(|d| {
                let broken = d.link_target.is_some() && d.content_fingerprint.is_none();
                json!({
                    "harness": d.harness.as_ref().map(|a| a.as_str().to_string()),
                    "scope": core_scope_str(home, &d.root.scope),
                    "path": normalize_path(home, &d.path.to_string_lossy()),
                    "is_link": d.link_target.is_some(),
                    "broken": broken,
                    "destination": serde_json::to_value(d.destination).unwrap(),
                    "backing": core_backing_str(&d.backing),
                    "mutability": serde_json::to_value(d.mutability).unwrap(),
                    "owner_kind": serde_json::to_value(d.owner_kind).unwrap(),
                    "owner_id": d.owner_id.as_ref().map(|o| o.as_str().to_string()),
                    "link_target": d
                        .link_target
                        .as_ref()
                        .map(|p| normalize_path(home, &p.to_string_lossy())),
                    "disabled_by": d.disabled_by.map(|b| vec![format!("{b:?}")]).unwrap_or_default(),
                    "spec_violations": d.spec_violations,
                    "plugin": d.plugin.as_ref().map(|p| json!({"plugin": p.plugin, "version": p.version})),
                    "shared_via_whole_dir_link": d.shared_via_whole_dir_link,
                    "fingerprint": d.content_fingerprint.as_ref().map(|f| f.as_str().to_string()),
                })
            })
            .collect();
        rows.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        out.insert(skill.name.0.clone(), rows);
    }
    out
}

/// Guards every `HOME` env mutation below: `read_lock_file` is the one
/// source `build_snapshot` doesn't take `home` for, so `run_desktop` mutates
/// the process-global `HOME` var around each call. This file now has more
/// than one `#[test]` function calling it, and cargo runs those in
/// parallel by default, so the mutation needs a real lock - not just the
/// single-serial-function discipline the original parity test relied on.
fn home_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

fn run_desktop(name: &str, home: &Path) -> BTreeMap<String, Vec<Value>> {
    let extra_projects: Vec<PathBuf> = if name == "project" {
        vec![home.join("proj")]
    } else {
        Vec::new()
    };
    let cache_path = home.join(".cache/invocations.json");
    let runs_root = home.join(".data/runs");
    let update_check_path = home.join(".data/update-check.json");
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&runs_root).unwrap();

    let mut invocation_index = SkillInvocationIndex::default();
    let paths = BuildPaths::new(&cache_path, &runs_root, &update_check_path);

    let _guard = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let previous_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", home);
    }
    let (snapshot, _report) = build_snapshot(
        home,
        &extra_projects,
        &Default::default(),
        &mut invocation_index,
        paths,
        chrono::Utc::now(),
    );
    unsafe {
        match &previous_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    project_desktop(&snapshot.skills, home)
}

/// Runs core's scan directly, over an explicit project list, and returns the
/// raw `Inventory` - the shape ported single-fixture tests need when they
/// assert on more than `owner_kind` (mutability, `source_kind`, `owner_id`,
/// `id`) via [`deployment_at`], where the flattened JSON rows `run_core`
/// produces don't carry every field.
fn core_scan(home: &Path, projects: &[PathBuf]) -> skill_studio_core::dto::Inventory {
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
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
    let mut scope = RuntimeScope::fixture(home);
    scope.read_timeout_ms = 10_000;
    if !projects.is_empty() {
        scope.projects = ProjectSelection::Explicit {
            paths: projects.to_vec(),
        };
    }
    let rt = Runtime::new(&scope, ports).expect("runtime");
    let ctx = OpContext::uncancellable(CorrelationId("core-scan-parity".into()));
    scan(&rt, &ctx, &ScanRequest::default()).expect("scan")
}

fn run_core(name: &str, home: &Path) -> BTreeMap<String, Vec<Value>> {
    let projects: Vec<PathBuf> = if name == "project" {
        vec![home.join("proj")]
    } else {
        Vec::new()
    };
    project_core(&core_scan(home, &projects), home)
}

/// A minimal spec-valid `SKILL.md`, matching
/// `skill_studio_core::testing::fixtures`' own `skill_md` helper.
fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Helps with {name} for fixture-driven tests.\n---\nBody text for {name}.\n")
}

/// Writes a Universal-root skill at `home/.agents/skills/<name>` and returns
/// its directory (the same path core's deployment `path` name).
fn write_universal_skill(home: &Path, name: &str) -> PathBuf {
    let dir = home.join(".agents").join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), skill_md(name)).unwrap();
    dir
}

/// The single deployment row for `skill` at `path`, from a projected
/// scan map (`project_desktop`/`project_core`'s output).
fn owner_kind_at<'a>(
    scans: &'a BTreeMap<String, Vec<Value>>,
    skill: &str,
    home: &Path,
    path: &Path,
) -> &'a str {
    let rows = scans
        .get(skill)
        .unwrap_or_else(|| panic!("no scan rows for skill {skill}"));
    let want = normalize_path(home, &path.to_string_lossy());
    rows.iter()
        .find(|row| row["path"].as_str() == Some(want.as_str()))
        .unwrap_or_else(|| panic!("no deployment row for {skill} at {want}"))["owner_kind"]
        .as_str()
        .unwrap_or_else(|| panic!("owner_kind missing for {skill} at {want}"))
}

/// The raw deployment row for `skill` at `path`, straight from core's scan
/// (not the flattened JSON `project_core` produces), for tests that assert
/// on `mutability`, `source_kind`, `owner_id`, or `id` - fields
/// [`owner_kind_at`]'s projected map doesn't carry.
fn deployment_at<'a>(
    inventory: &'a skill_studio_core::dto::Inventory,
    skill: &str,
    home: &Path,
    path: &Path,
) -> &'a skill_studio_core::dto::DeploymentDto {
    let want = normalize_path(home, &path.to_string_lossy());
    inventory
        .skills
        .iter()
        .find(|s| s.name.0 == skill)
        .unwrap_or_else(|| panic!("no scan rows for skill {skill}"))
        .deployments
        .iter()
        .find(|d| normalize_path(home, &d.path.to_string_lossy()) == want)
        .unwrap_or_else(|| panic!("no deployment row for {skill} at {want}"))
}

/// Writes `~/.agents/.skill-lock.json` claiming `name`, matching the
/// deleted `skill_ownership.rs` tests' inline lock-file literals.
fn write_skills_sh_lock(home: &Path, name: &str) {
    let agents = home.join(".agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join(".skill-lock.json"),
        format!(
            r#"{{"version":3,"skills":{{"{name}":{{"source":"o/r","sourceType":"github","sourceUrl":"https://github.com/o/r","skillFolderHash":"a","installedAt":"t","updatedAt":"t"}}}}}}"#
        ),
    )
    .unwrap();
}

/// Writes a dotagents ledger pair (`agents.lock` + `agents.toml`) claiming
/// `name` under `root` (`<home>/.agents` or `<project>/.agents`), matching
/// the deleted tests' `write_dual_ledger`/inline literals.
fn write_dotagents_ledger(root: &Path, name: &str) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("agents.lock"),
        format!(
            "[skills.{name}]\nsource = \"o/r\"\nresolved_path = \"skills/{name}\"\nresolved_commit = \"abc\"\n"
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("agents.toml"),
        format!("[[skills]]\nname = \"{name}\"\nsource = \"o/r\"\n"),
    )
    .unwrap();
}

/// Proves `Dotagents` and `WildcardDotagents` classify distinctly: core reads
/// `agents.lock`/`agents.toml` through `ScopeFs` (a lock row with a matching
/// manifest row is `Dotagents`, mutable; a lock-only row from a wildcard
/// install is `WildcardDotagents`, read-only).
#[test]
fn named_and_wildcard_dotagents_rows_classify_distinctly() {
    let dir = unique_temp_dir("dotagents-owner");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    // `docs-writer` has both a `agents.lock` entry and a `agents.toml`
    // `[[skills]]` row - a full dotagents install.
    let docs_writer = write_universal_skill(&home, "docs-writer");
    // `wildcard-tool` has only the `agents.lock` entry: it was pulled in by
    // a wildcard (`dotagents install --all`), so it has no manifest row.
    let wildcard_tool = write_universal_skill(&home, "wildcard-tool");

    let agents_dir = home.join(".agents");
    std::fs::write(
        agents_dir.join("agents.lock"),
        r#"
[skills.docs-writer]
source = "someorg/docs-writer"
resolved_path = "skills/docs-writer"
resolved_commit = "1111111111111111111111111111111111aaaa"

[skills.wildcard-tool]
source = "someorg/wildcard-tool"
resolved_path = "skills/wildcard-tool"
resolved_commit = "2222222222222222222222222222222222bbbb"
"#,
    )
    .unwrap();
    std::fs::write(
        agents_dir.join("agents.toml"),
        r#"
[[skills]]
name = "docs-writer"
source = "someorg/docs-writer"
path = "skills/docs-writer"
"#,
    )
    .unwrap();

    let inventory = core_scan(&home, &[]);

    let docs_row = deployment_at(&inventory, "docs-writer", &home, &docs_writer);
    assert_eq!(docs_row.owner_kind, LifecycleOwnerKind::Dotagents);
    assert_eq!(docs_row.mutability, DeploymentMutability::Mutable);

    let wildcard_row = deployment_at(&inventory, "wildcard-tool", &home, &wildcard_tool);
    assert_eq!(
        wildcard_row.owner_kind,
        LifecycleOwnerKind::WildcardDotagents
    );
    assert_eq!(wildcard_row.mutability, DeploymentMutability::ReadOnly);

    std::fs::remove_dir_all(&dir).ok();
}

/// Proves `Ambiguous` parity: a name claimed by both the dotagents ledger
/// and the skills.sh lock file is `Ambiguous` on both sides, refusing
/// owner-wide actions rather than letting either ledger's weaker kind widen
/// what a repair is allowed to touch.
#[test]
fn dual_claimed_owner_is_ambiguous() {
    let dir = unique_temp_dir("dual-claim-owner");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    let dual_skill = write_universal_skill(&home, "dual-skill");

    let agents_dir = home.join(".agents");
    std::fs::write(
        agents_dir.join("agents.lock"),
        r#"
[skills.dual-skill]
source = "someorg/dual-skill"
resolved_path = "skills/dual-skill"
resolved_commit = "3333333333333333333333333333333333cccc"
"#,
    )
    .unwrap();
    std::fs::write(
        agents_dir.join("agents.toml"),
        r#"
[[skills]]
name = "dual-skill"
source = "someorg/dual-skill"
path = "skills/dual-skill"
"#,
    )
    .unwrap();
    std::fs::write(
        agents_dir.join(".skill-lock.json"),
        r#"{"version":3,"skills":{"dual-skill":{"source":"owner/dual-skill","sourceType":"github","sourceUrl":"https://github.com/owner/dual-skill","skillFolderHash":"deadbeef","installedAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}}}"#,
    )
    .unwrap();

    let core = run_core("dual-claim-owner", &home);

    assert_eq!(
        owner_kind_at(&core, "dual-skill", &home, &dual_skill),
        "ambiguous"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Proves `Copy` classification: core reads `~/.agents/skill-studio.json`'s
/// `copies` map through `ScopeFs` and matches a registered copy record by
/// scope, destination, project, content hash, name, path, and disabled
/// state.
#[test]
fn registered_copy_record_owns_the_copy() {
    let dir = unique_temp_dir("copy-owner");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    let copied_skill = write_universal_skill(&home, "copied-skill");

    // First pass (no copy record yet) just to read back the real content
    // hash core's own scan computed for this skill, the same way
    // `skill_add`/`skill_independent_copy` record it at install time -
    // reusing that value here instead of reimplementing the hash algorithm.
    let before = run_core("copy-owner", &home);
    let fingerprint = before["copied-skill"]
        .iter()
        .find(|row| {
            row["path"].as_str()
                == Some(normalize_path(&home, &copied_skill.to_string_lossy()).as_str())
        })
        .and_then(|row| row["fingerprint"].as_str())
        .unwrap_or_else(|| panic!("copied-skill has no fingerprint yet"))
        .to_string();
    let content_hash = fingerprint
        .strip_prefix("sha256:")
        .unwrap_or(&fingerprint)
        .to_string();

    let deployment_id = skill_studio_lib::skill_deployment::deployment_id(
        "copied-skill",
        "global",
        skill_studio_lib::skill_deployment::SkillDestination::Universal,
        "universal",
        None,
        &copied_skill,
    );

    let mut registry = skill_studio_lib::skill_fork_registry::ForkRegistry::default();
    registry.copies.insert(
        deployment_id.clone(),
        skill_studio_lib::skill_fork_registry::CopyDeploymentRecord {
            deployment_id,
            name: "copied-skill".to_string(),
            path: copied_skill.clone(),
            scope: skill_studio_lib::skill_dto::InstallScope::Global,
            destination: skill_studio_lib::skill_deployment::SkillDestination::Universal,
            slot: "universal".to_string(),
            project_path: None,
            content_hash,
            disabled: false,
        },
    );
    skill_studio_lib::skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

    let core = run_core("copy-owner", &home);

    assert_eq!(
        owner_kind_at(&core, "copied-skill", &home, &copied_skill),
        "copy"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Proves `Fork` parity: a global, per-harness-free (Universal), non-link
/// deployment whose name and directory match a `skill-studio.json` `forks`
/// record is `Fork` on both sides - the desktop assigns it in a later
/// overlay pass (`apply_skill_snapshot_overlays`), core assigns it inside
/// `classify_owner` itself, but the observable result must still agree.
#[test]
fn fork_record_owns_the_fork() {
    let dir = unique_temp_dir("fork-owner");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    let forked_skill = write_universal_skill(&home, "forked-skill");

    let deployment_id = skill_studio_lib::skill_deployment::deployment_id(
        "forked-skill",
        "global",
        skill_studio_lib::skill_deployment::SkillDestination::Universal,
        "universal",
        None,
        &forked_skill,
    );

    let mut registry = skill_studio_lib::skill_fork_registry::ForkRegistry::default();
    registry.forks.insert(
        "forked-skill".to_string(),
        skill_studio_lib::skill_fork_registry::ForkRecord {
            deployment_id,
            skill_dir: forked_skill.clone(),
            forked_at: "2026-01-01T00:00:00Z".to_string(),
            origin_tool: skill_studio_lib::skill_fork_registry::OriginTool::SkillsSh,
            origin_source: "owner/forked-skill".to_string(),
            repo: "owner/forked-skill".to_string(),
            path: String::new(),
            declared_ref: None,
            base_commit: "deadbeef".to_string(),
        },
    );
    skill_studio_lib::skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

    let core = run_core("fork-owner", &home);

    assert_eq!(
        owner_kind_at(&core, "forked-skill", &home, &forked_skill),
        "fork"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn desktop_assembly_matches_core_scan_for_every_fixture() {
    for (name, builder) in fixtures::all() {
        let dir = unique_temp_dir(name);
        std::fs::create_dir_all(&dir).unwrap();
        let home = dir.canonicalize().unwrap();
        builder
            .materialize(&home)
            .unwrap_or_else(|e| panic!("materialize {name}: {e}"));

        let desktop = run_desktop(name, &home);
        let core = run_core(name, &home);

        if desktop != core {
            let desktop_text = serde_json::to_string_pretty(&desktop).unwrap();
            let core_text = serde_json::to_string_pretty(&core).unwrap();
            let diff = similar::TextDiff::from_lines(&desktop_text, &core_text)
                .unified_diff()
                .header("desktop", "core")
                .to_string();
            panic!("{name}: desktop and core scans disagree:\n{diff}");
        }

        std::fs::remove_dir_all(&dir).ok();
    }
}

/// Proves the first of the desktop's two `Ambiguous` carve-outs: a universal
/// root beside an `agents.toml`/`agents.lock` that names no row for this
/// skill. The dotagents install owns the root, so the skill reads as
/// dotagents-managed, but nothing says who owns it - and `Ambiguous` permits
/// no repair where `Manual` permits a direct one, so a core that skipped the
/// carve-out would widen what a repair may touch.
#[test]
fn a_universal_root_beside_an_unnamed_dotagents_ledger_is_ambiguous() {
    let dir = unique_temp_dir("shared-root-ambiguous");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    let orphan = write_universal_skill(&home, "orphan-skill");

    // A dotagents install with a row for some *other* skill, so the ledger
    // files exist but never name `orphan-skill`.
    let agents_dir = home.join(".agents");
    std::fs::write(
        agents_dir.join("agents.toml"),
        "\n[[skills]]\nname = \"someone-else\"\nsource = \"someorg/someone-else\"\npath = \"skills/someone-else\"\n",
    )
    .unwrap();

    let core = run_core("shared-root-ambiguous", &home);

    assert_eq!(
        owner_kind_at(&core, "orphan-skill", &home, &orphan),
        "ambiguous"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Proves the second carve-out: a symlink that both lives in the universal
/// root and points back into it. The bytes belong to the deployment it
/// points at, whose own ledger entry may say otherwise, so this end of the
/// link claims no owner.
///
/// The link must sit inside `.agents/skills` to reach the carve-out at all:
/// a per-harness link is classified `Manual` well before it, since
/// `classify_owner` only applies a ledger to a deployment under the
/// universal root.
#[test]
fn a_link_inside_the_universal_root_pointing_into_it_is_ambiguous() {
    let dir = unique_temp_dir("link-ambiguous");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    let real = write_universal_skill(&home, "real-skill");
    let link = home.join(".agents").join("skills").join("alias-skill");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let core = run_core("link-ambiguous", &home);

    assert_eq!(
        owner_kind_at(&core, "alias-skill", &home, &link),
        "ambiguous"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Ported from the deleted `apps/desktop/src-tauri/src/skills/
// skill_ownership.rs` unit tests (removed when the desktop's own scanner was
// deleted, on the mistaken claim that this file already covered them). Each
// test below keeps the deleted test's name and fixture, asserting directly
// against core's `ops::scan` instead of the desktop's now-gone
// `classify_lifecycle_owner`.
// ---------------------------------------------------------------------------

/// Writes a skill directory at `skills_root/<name>`, for per-harness roots
/// `write_universal_skill` doesn't reach (it always targets
/// `<home>/.agents/skills`).
fn write_skill_at(skills_root: &Path, name: &str) -> PathBuf {
    let dir = skills_root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), skill_md(name)).unwrap();
    dir
}

/// Writes a `SKILL.md` with literal `content`, for the copy-ownership
/// fixtures below where the deleted tests wrote unparsed bytes
/// (`"original content"`, `"edited content"`, ...) rather than a spec-valid
/// file - ownership classification doesn't require a parseable frontmatter.
fn write_skill_content(dir: &Path, content: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), content).unwrap();
}

/// The bare hex of `skill`'s content fingerprint at `path`, for registering
/// a `CopyDeploymentRecord`'s `content_hash` against the value core's own
/// scan actually computed - the same "scan once to read back the real hash"
/// approach `registered_copy_record_owns_the_copy` uses.
fn bare_fingerprint(
    inventory: &skill_studio_core::dto::Inventory,
    skill: &str,
    home: &Path,
    path: &Path,
) -> String {
    deployment_at(inventory, skill, home, path)
        .content_fingerprint
        .as_ref()
        .unwrap_or_else(|| panic!("{skill} has no fingerprint yet"))
        .bare_hex()
        .to_string()
}

/// A `CopyDeploymentRecord` for a global, per-harness Codex deployment,
/// matching the deleted tests' `copy_record` helper.
fn copy_record(
    deployment_id: &str,
    name: &str,
    path: &Path,
    content_hash: String,
) -> skill_studio_lib::skill_fork_registry::CopyDeploymentRecord {
    skill_studio_lib::skill_fork_registry::CopyDeploymentRecord {
        deployment_id: deployment_id.to_string(),
        name: name.to_string(),
        path: path.to_path_buf(),
        scope: skill_studio_lib::skill_dto::InstallScope::Global,
        destination: skill_studio_lib::skill_deployment::SkillDestination::PerHarness,
        slot: "codex".to_string(),
        project_path: None,
        content_hash,
        disabled: false,
    }
}

/// Ports `skills_sh_lock_only_matches_same_agents_root`: a skills.sh lock
/// file recorded only at the home `.agents` root claims the global
/// deployment of the same name, but not a project-scoped deployment that
/// merely shares the skill's name - each scope's `.agents` root has its own
/// ledger.
#[test]
fn skills_sh_lock_only_matches_same_agents_root() {
    let dir = unique_temp_dir("lock-same-root");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();
    let project = home.join("proj");

    let global_skill = write_universal_skill(&home, "find-bugs");
    let project_skill = write_universal_skill(&project, "find-bugs");
    write_skills_sh_lock(&home, "find-bugs");

    let inventory = core_scan(&home, std::slice::from_ref(&project));

    let global = deployment_at(&inventory, "find-bugs", &home, &global_skill);
    assert_eq!(global.owner_kind, LifecycleOwnerKind::SkillsSh);
    assert_eq!(global.source_kind, SourceKind::SkillsSh);

    let project_row = deployment_at(&inventory, "find-bugs", &home, &project_skill);
    assert_eq!(project_row.owner_kind, LifecycleOwnerKind::Manual);
    assert_eq!(project_row.source_kind, SourceKind::Manual);

    std::fs::remove_dir_all(&dir).ok();
}

/// Ports `exact_project_dual_ledger_owner_is_ambiguous_and_read_only`. The
/// global-scope sibling (`exact_global_dual_ledger_owner_is_ambiguous_and_read_only`)
/// is not ported: `dual_claimed_owner_is_ambiguous`
/// above already exercises the identical setup - a global-scope skill under
/// `<home>/.agents/skills` with a dotagents `agents.lock` + `agents.toml`
/// pair *and* a `.skill-lock.json` entry, all for the same name, at the same
/// `.agents` root - and already asserts `owner_kind == "ambiguous"` for it.
/// This test is the project-scoped variant, which no existing test covers.
#[test]
fn exact_project_dual_ledger_owner_is_ambiguous_and_read_only() {
    let dir = unique_temp_dir("dual-ledger-project");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();
    let project = home.join("project");

    let skill_dir = write_universal_skill(&project, "find-bugs");
    write_dotagents_ledger(&project.join(".agents"), "find-bugs");
    write_skills_sh_lock(&project, "find-bugs");

    let inventory = core_scan(&home, std::slice::from_ref(&project));
    let row = deployment_at(&inventory, "find-bugs", &home, &skill_dir);

    assert_eq!(row.owner_kind, LifecycleOwnerKind::Ambiguous);
    assert!(row.owner_id.is_none());
    assert_eq!(row.mutability, DeploymentMutability::ReadOnly);

    std::fs::remove_dir_all(&dir).ok();
}

/// Ports `frontmatter_name_cannot_claim_a_skills_sh_owner`: a `SKILL.md`
/// frontmatter `name` that disagrees with its directory name cannot use that
/// frontmatter name to claim a skills.sh lock entry - classification keys
/// off the directory name (`foo`), which the lock file (keyed `bar`) never
/// claims, so the deployment falls to `Manual` and keeps its mismatch
/// spec violation.
#[test]
fn frontmatter_name_cannot_claim_a_skills_sh_owner() {
    let dir = unique_temp_dir("frontmatter-skills-sh");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    let skill_dir = home.join(".agents/skills/foo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: bar\ndescription: mismatch\n---\nbody",
    )
    .unwrap();
    write_skills_sh_lock(&home, "bar");

    let inventory = core_scan(&home, &[]);
    let row = deployment_at(&inventory, "foo", &home, &skill_dir);

    assert_eq!(row.owner_kind, LifecycleOwnerKind::Manual);
    assert!(row.owner_id.is_none());
    assert!(row
        .spec_violations
        .iter()
        .any(|violation| violation.contains("does not match its directory name \"foo\"")));

    std::fs::remove_dir_all(&dir).ok();
}

/// Ports `frontmatter_name_cannot_claim_a_dotagents_owner`: the same
/// frontmatter/directory mismatch, but against a dotagents ledger (`agents.toml`
/// row named `bar`) instead of the skills.sh lock. The universal root sits
/// beside dotagents ledger files that name no row for `foo`, so the
/// unnamed-root carve-out applies and the deployment is `Ambiguous`, not
/// `Manual`.
#[test]
fn frontmatter_name_cannot_claim_a_dotagents_owner() {
    let dir = unique_temp_dir("frontmatter-dotagents");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    let skill_dir = home.join(".agents/skills/foo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: bar\ndescription: mismatch\n---\nbody",
    )
    .unwrap();
    write_dotagents_ledger(&home.join(".agents"), "bar");

    let inventory = core_scan(&home, &[]);
    let row = deployment_at(&inventory, "foo", &home, &skill_dir);

    assert_eq!(row.owner_kind, LifecycleOwnerKind::Ambiguous);
    assert!(row.owner_id.is_none());

    std::fs::remove_dir_all(&dir).ok();
}

/// Ports `unrecorded_first_class_per_harness_folders_remain_manual_despite_universal_lock`:
/// a skills.sh lock entry recorded at the universal root never reaches a
/// same-named skill sitting in a first-class harness's own per-harness root
/// (the dotagents/skills.sh ledgers only ever apply to a deployment rooted
/// directly in the universal root) - every harness's own copy stays
/// `Manual`, read-only.
#[test]
fn unrecorded_first_class_per_harness_folders_remain_manual_despite_universal_lock() {
    let dir = unique_temp_dir("per-harness-manual");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();
    write_skills_sh_lock(&home, "find-bugs");

    let harness_roots = [
        ("Claude Code", ".claude/skills"),
        ("Codex", ".codex/skills"),
        ("OpenCode", ".config/opencode/skills"),
        ("pi", ".pi/agent/skills"),
        ("Cursor", ".cursor/skills"),
        ("Grok Build", ".grok/skills"),
    ];
    let skill_dirs: Vec<(&str, PathBuf)> = harness_roots
        .iter()
        .map(|(label, root)| (*label, write_skill_at(&home.join(root), "find-bugs")))
        .collect();

    let inventory = core_scan(&home, &[]);
    for (label, skill_dir) in skill_dirs {
        let row = deployment_at(&inventory, "find-bugs", &home, &skill_dir);
        assert_eq!(row.owner_kind, LifecycleOwnerKind::Manual, "{label}");
        assert!(row.owner_id.is_none(), "{label}");
        assert_eq!(row.source_kind, SourceKind::Manual, "{label}");
        assert_eq!(row.mutability, DeploymentMutability::ReadOnly, "{label}");
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// Ports `copy_ownership_requires_an_exact_recorded_deployment_identity`: a
/// registered `CopyDeploymentRecord` is only found by an exact deployment-id
/// map key. Registering it under the real id yields `Copy`; registering the
/// same record under a different harness slot's id means the real
/// deployment's own id no longer matches any map entry, so it falls back to
/// `Manual` rather than inheriting the record's kind.
#[test]
fn copy_ownership_requires_an_exact_recorded_deployment_identity() {
    let dir = unique_temp_dir("copy-exact-identity");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    let skill_dir = home.join(".codex/skills/find-bugs");
    write_skill_content(&skill_dir, "original content");

    let before = core_scan(&home, &[]);
    let content_hash = bare_fingerprint(&before, "find-bugs", &home, &skill_dir);

    let deployment_id = skill_studio_lib::skill_deployment::deployment_id(
        "find-bugs",
        "global",
        skill_studio_lib::skill_deployment::SkillDestination::PerHarness,
        "codex",
        None,
        &skill_dir,
    );

    let mut registry = skill_studio_lib::skill_fork_registry::ForkRegistry::default();
    registry.copies.insert(
        deployment_id.clone(),
        copy_record(
            &deployment_id,
            "find-bugs",
            &skill_dir,
            content_hash.clone(),
        ),
    );
    skill_studio_lib::skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

    let matched = core_scan(&home, &[]);
    assert_eq!(
        deployment_at(&matched, "find-bugs", &home, &skill_dir).owner_kind,
        LifecycleOwnerKind::Copy
    );

    let wrong_id = deployment_id.replace("/codex/", "/claude-code/");
    let mut mismatched_registry = skill_studio_lib::skill_fork_registry::ForkRegistry::default();
    mismatched_registry.copies.insert(
        wrong_id.clone(),
        copy_record(&wrong_id, "find-bugs", &skill_dir, content_hash),
    );
    skill_studio_lib::skill_fork_registry::write_fork_registry(&home, &mismatched_registry)
        .unwrap();

    let mismatched = core_scan(&home, &[]);
    assert_eq!(
        deployment_at(&mismatched, "find-bugs", &home, &skill_dir).owner_kind,
        LifecycleOwnerKind::Manual
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Ports `copy_ownership_rejects_edited_content`: the same pinned-hash
/// rejection as above, but the directory is never removed - only the
/// `SKILL.md` bytes are edited in place, which is enough to change the
/// fingerprint and drop the deployment to `Manual`.
#[test]
fn copy_ownership_rejects_edited_content() {
    let dir = unique_temp_dir("copy-rejects-edited");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    let skill_dir = home.join(".codex/skills/find-bugs");
    write_skill_content(&skill_dir, "original content");

    let before = core_scan(&home, &[]);
    let content_hash = bare_fingerprint(&before, "find-bugs", &home, &skill_dir);
    let deployment_id = skill_studio_lib::skill_deployment::deployment_id(
        "find-bugs",
        "global",
        skill_studio_lib::skill_deployment::SkillDestination::PerHarness,
        "codex",
        None,
        &skill_dir,
    );
    let mut registry = skill_studio_lib::skill_fork_registry::ForkRegistry::default();
    registry.copies.insert(
        deployment_id.clone(),
        copy_record(&deployment_id, "find-bugs", &skill_dir, content_hash),
    );
    skill_studio_lib::skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

    write_skill_content(&skill_dir, "edited content");

    let after = core_scan(&home, &[]);
    let row = deployment_at(&after, "find-bugs", &home, &skill_dir);
    assert_eq!(row.owner_kind, LifecycleOwnerKind::Manual);
    assert_eq!(row.mutability, DeploymentMutability::ReadOnly);

    std::fs::remove_dir_all(&dir).ok();
}

/// Ports `copy_ownership_rejects_an_empty_legacy_content_hash`: a legacy
/// record with an empty `content_hash` (predating the field) matches
/// nothing, on purpose - `classify_owner` requires a non-empty recorded hash
/// before comparing it, so the deployment reads as `Manual` even though
/// every other field on the record matches exactly.
#[test]
fn copy_ownership_rejects_an_empty_legacy_content_hash() {
    let dir = unique_temp_dir("copy-rejects-empty-hash");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    let skill_dir = home.join(".codex/skills/find-bugs");
    write_skill_content(&skill_dir, "original content");

    let deployment_id = skill_studio_lib::skill_deployment::deployment_id(
        "find-bugs",
        "global",
        skill_studio_lib::skill_deployment::SkillDestination::PerHarness,
        "codex",
        None,
        &skill_dir,
    );
    let mut registry = skill_studio_lib::skill_fork_registry::ForkRegistry::default();
    registry.copies.insert(
        deployment_id.clone(),
        copy_record(&deployment_id, "find-bugs", &skill_dir, String::new()),
    );
    skill_studio_lib::skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

    let inventory = core_scan(&home, &[]);
    let row = deployment_at(&inventory, "find-bugs", &home, &skill_dir);
    assert_eq!(row.owner_kind, LifecycleOwnerKind::Manual);
    assert_eq!(row.mutability, DeploymentMutability::ReadOnly);

    std::fs::remove_dir_all(&dir).ok();
}

/// Ports `named_dotagents_row_is_mutable_dotagents`: a universal-root skill
/// with a full dotagents install (an `agents.lock` row *and* a matching
/// `agents.toml` `[[skills]]` row) is `Dotagents`, mutable, with the
/// expected `owner:v1/global/<name>` id.
#[test]
fn named_dotagents_row_is_mutable_dotagents() {
    let dir = unique_temp_dir("named-dotagents-mutable");
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.canonicalize().unwrap();

    let skill_dir = write_universal_skill(&home, "find-bugs");
    write_dotagents_ledger(&home.join(".agents"), "find-bugs");

    let inventory = core_scan(&home, &[]);
    let row = deployment_at(&inventory, "find-bugs", &home, &skill_dir);

    assert_eq!(row.owner_kind, LifecycleOwnerKind::Dotagents);
    assert_eq!(row.source_kind, SourceKind::Dotagents);
    assert_eq!(
        row.owner_id.as_ref().map(|o| o.as_str()),
        Some("owner:v1/global/find-bugs")
    );
    assert_eq!(row.mutability, DeploymentMutability::Mutable);

    std::fs::remove_dir_all(&dir).ok();
}
