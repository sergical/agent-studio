// Integration test binaries aren't covered by the lib crate's
// `cfg_attr(test, allow(...))`: this file compiles as its own crate, so
// the same allow needs to be declared here too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! What `scan`, `diagnose`, and the doctor make of the home-directory
//! layouts a real machine carries.
//!
//! Every fixture comes from [`skill_studio_core::testing_shapes`], and
//! every expectation below is derived from `docs/agent-skill-conventions.md`
//! or `docs/action-map/harnesses/`, cited in the test's own doc comment -
//! never from what the scanner happens to do today.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_studio_core::doctor::check_link_resolves_in_root;
use skill_studio_core::dto::{
    Diagnosis, HarnessesRequest, Inventory, ScanRequest, SetHarnessEnabledRequest,
};
use skill_studio_core::harness::{HarnessCatalog, HarnessState};
use skill_studio_core::identity::{AgentId, ProjectRef, RootKind, RootRef, RootScope, SkillName};
use skill_studio_core::lock_file::{read_lock_file, InstalledSkillEntry};
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime, ScopeFs};
use skill_studio_core::scope::{ProjectSelection, RuntimeScope};
use skill_studio_core::testing::golden::{ctx, unique_temp_dir};
use skill_studio_core::testing::{
    FakeClock, FakeIds, FakeLease, FakeToolLookup, FixtureBuilder, NoHistory, RecordingSink,
};
use skill_studio_core::testing_shapes as shapes;

use skill_studio_host::{FileLease, RealFs, SqliteHistoryOpener};

/// Absolute home every in-memory fixture is rooted at. `FixtureFs` has no
/// directory of its own, and [`RuntimeScope`] needs an absolute home.
const HOME: &str = "/home";

/// A runtime over the in-memory fixture, with the projects and the `PATH`
/// lookup a given test needs.
fn in_memory_runtime(
    builder: FixtureBuilder,
    projects: Vec<PathBuf>,
    binaries: &[&str],
) -> Runtime {
    let fs: Arc<dyn ScopeFs> = Arc::new(builder.rooted_at(HOME).dir(HOME).build_fs());
    let mut tools = FakeToolLookup::default();
    for binary in binaries {
        tools.binaries.insert(
            (*binary).to_string(),
            PathBuf::from("/usr/local/bin").join(binary),
        );
    }
    let ports = Ports {
        fs,
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FakeLease::default()),
        history: Arc::new(NoHistory),
        sink: Arc::new(RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: Some(Arc::new(tools)),
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    let mut scope = RuntimeScope::fixture(Path::new(HOME));
    scope.read_timeout_ms = 10_000;
    if !projects.is_empty() {
        scope.projects = ProjectSelection::Explicit { paths: projects };
    }
    Runtime::new(&scope, ports).expect("runtime")
}

fn scan_shape(builder: FixtureBuilder) -> Inventory {
    let rt = in_memory_runtime(builder, Vec::new(), &[]);
    ops::scan(&rt, &ctx(), &ScanRequest::default()).expect("scan")
}

fn diagnose_shape(builder: FixtureBuilder) -> Diagnosis {
    let rt = in_memory_runtime(builder, Vec::new(), &[]);
    ops::diagnose(&rt, &ctx(), &ScanRequest::default()).expect("diagnose")
}

/// Every deployment path in the inventory, as a display string.
fn deployment_paths(inventory: &Inventory) -> Vec<String> {
    inventory
        .skills
        .iter()
        .flat_map(|skill| &skill.deployments)
        .map(|deployment| deployment.path.display().to_string())
        .collect()
}

/// How many rows carry `name`. A row per skill name is the inventory's own
/// unit, so "exactly once" is a count of 1 here, whatever the deployment
/// count under it.
fn rows_named(inventory: &Inventory, name: &str) -> usize {
    inventory
        .skills
        .iter()
        .filter(|skill| skill.name.0 == name)
        .count()
}

/// A synced bucket carries no `SKILL.md` at its own level, so it is not a
/// skill folder (<https://agentskills.io/specification>), and the folder
/// name `synced` is reserved by the vendor inside a skills root
/// (`docs/action-map/harnesses/claude-code.md`: "`synced` under
/// `~/.claude/skills` is reserved; the scanner must skip it"). The skills
/// the bucket feeds into the root itself are ordinary rows, one each, even
/// though the bucket holds a second copy of their bytes.
#[test]
fn scan_reports_synced_bucket_skills_once_and_not_the_bucket_root_or_names_the_extra_row() {
    let mut builder = FixtureBuilder::new();
    builder = shapes::with_synced_bucket(builder, ".agents/skills");
    builder = shapes::with_synced_bucket(builder, shapes::CLAUDE_ROOT_RELATIVE);
    let inventory = scan_shape(builder);

    for name in shapes::SYNCED_BUCKET_SKILLS {
        assert_eq!(
            rows_named(&inventory, name),
            1,
            "`{name}` is deployed in the shared root and in {}, which is one skill with two \
             deployments, so the inventory must carry exactly one row for it; rows: {:?}",
            shapes::CLAUDE_ROOT_RELATIVE,
            inventory
                .skills
                .iter()
                .map(|s| s.name.0.clone())
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(
        rows_named(&inventory, shapes::SYNCED_DIR_NAME),
        0,
        "`{}` holds no SKILL.md and is a reserved folder name, so it must not be a row",
        shapes::SYNCED_DIR_NAME
    );
    assert_eq!(
        rows_named(&inventory, shapes::SYNCED_BUCKET_ID),
        0,
        "the bucket folder `{}` holds a manifest.json, not a SKILL.md, so it must not be a row",
        shapes::SYNCED_BUCKET_ID
    );
    let bucket_segment = format!("/{}/", shapes::SYNCED_DIR_NAME);
    let from_bucket: Vec<String> = deployment_paths(&inventory)
        .into_iter()
        .filter(|path| path.contains(&bucket_segment))
        .collect();
    assert!(
        from_bucket.is_empty(),
        "no deployment may come from inside a synced bucket: {from_bucket:?}"
    );
}

/// The skills Codex bundles with itself sit behind a dot-prefixed `.system`
/// folder and a dot-prefixed marker file. `docs/action-map/harnesses/codex.md`
/// lists them as "bundled skills", separate from the roots a user deploys
/// into, and the core's harness facts record that every reader skips hidden
/// entries. A bundled skill Skill Studio can neither move nor remove must
/// therefore not appear as a deployment at all.
#[test]
fn scan_skips_codex_system_skills_or_marks_them_not_removable_or_names_the_row() {
    let inventory = scan_shape(shapes::with_codex_system_skills(FixtureBuilder::new()));

    for name in shapes::CODEX_SYSTEM_SKILLS {
        assert_eq!(
            rows_named(&inventory, name),
            0,
            "`{name}` is one of Codex's own bundled skills under {}/{}, which no user root \
             owns; rows: {:?}",
            shapes::CODEX_ROOT_RELATIVE,
            shapes::CODEX_SYSTEM_DIR_NAME,
            inventory
                .skills
                .iter()
                .map(|s| s.name.0.clone())
                .collect::<Vec<_>>()
        );
    }
    let hidden: Vec<String> = deployment_paths(&inventory)
        .into_iter()
        .filter(|path| path.contains(shapes::CODEX_SYSTEM_DIR_NAME))
        .collect();
    assert!(
        hidden.is_empty(),
        "a dot-prefixed folder is hidden from every documented reader, so no deployment may \
         come from one: {hidden:?}"
    );
}

/// `npx skills` in symlink mode writes each harness's entry as a relative
/// link into the shared root (`docs/action-map/harnesses/shared-root.md`),
/// and a symlinked skill folder "loads once, deduplicated by target"
/// (`docs/action-map/harnesses/sources.md`). One skill reached twice is one
/// row with two deployments, not two rows.
#[test]
fn scan_dedupes_a_pi_relative_link_onto_its_shared_deployment_or_names_the_duplicate() {
    let inventory = scan_shape(shapes::with_pi_links_to_shared(
        FixtureBuilder::new(),
        &["linked-skill-1"],
    ));

    assert_eq!(
        rows_named(&inventory, "linked-skill-1"),
        1,
        "the shared deployment and pi's relative link are the same folder, so they must \
         collapse onto one row; rows: {:?}",
        inventory
            .skills
            .iter()
            .map(|s| s.name.0.clone())
            .collect::<Vec<_>>()
    );
    let skill = &inventory.skills[0];
    let shared = PathBuf::from(HOME).join(".agents/skills/linked-skill-1");
    let pi_link = PathBuf::from(HOME)
        .join(shapes::PI_ROOT_RELATIVE)
        .join("linked-skill-1");
    let paths: Vec<PathBuf> = skill.deployments.iter().map(|d| d.path.clone()).collect();
    assert!(
        paths.contains(&shared) && paths.contains(&pi_link),
        "both the shared deployment {} and pi's link {} must be listed under the one row, \
         got {paths:?}",
        shared.display(),
        pi_link.display()
    );
    let link = skill
        .deployments
        .iter()
        .find(|d| d.path == pi_link)
        .expect("pi deployment");
    assert_eq!(
        link.resolved_path.as_ref(),
        Some(&shared),
        "pi's relative link must resolve onto the shared folder, not stay unresolved; \
         link_target: {:?}",
        link.link_target
    );
}

/// Doctor invariant 1 is "every link resolves inside its root". A relative
/// link whose target does resolve is not a violation; reporting one would
/// send the user to repair the deployment shape `npx skills` writes by
/// default (`docs/action-map/harnesses/shared-root.md`).
#[test]
fn doctor_link_check_resolves_relative_links_inside_the_root_or_names_the_false_violation() {
    let diagnosis = diagnose_shape(shapes::with_pi_links_to_shared(
        FixtureBuilder::new(),
        &["linked-skill-1", "linked-skill-2"],
    ));

    let violations = check_link_resolves_in_root(&diagnosis);
    assert!(
        violations.is_empty(),
        "a relative link into the shared root resolves, so invariant 1 must report nothing; \
         got: {:?}",
        violations
            .iter()
            .map(|v| (v.path.display().to_string(), v.message.clone()))
            .collect::<Vec<_>>()
    );
}

/// `OpenCode`'s documented roots are `~/.config/opencode/skills` and the
/// legacy `skill/` next to it (`docs/agent-skill-conventions.md`, Discovery
/// paths); `~/.opencode` is not one of them. The harness is still
/// Configured when its config file exists and its binary is on `PATH`
/// (`docs/action-map/harnesses/harness-detection.md`), so "no skills" and
/// "not set up" must not be confused.
#[test]
fn scan_reports_opencode_installed_with_zero_skills_and_ignores_dot_opencode_or_names_the_root() {
    let builder = shapes::with_opencode_installed_without_skill_root(FixtureBuilder::new());
    let rt = in_memory_runtime(builder, Vec::new(), &["opencode"]);

    let inventory = ops::scan(&rt, &ctx(), &ScanRequest::default()).expect("scan");
    assert!(
        inventory.skills.is_empty(),
        "neither `skills/` nor `skill/` exists under .config/opencode, so no row may come \
         out of this home; got {:?}",
        deployment_paths(&inventory)
    );
    let stray: Vec<String> = deployment_paths(&inventory)
        .into_iter()
        .filter(|path| path.contains("/.opencode"))
        .collect();
    assert!(
        stray.is_empty(),
        "`~/.opencode` is a node package folder, not a documented skills root: {stray:?}"
    );

    let report = ops::harnesses(&rt, &ctx(), &HarnessesRequest::default()).expect("harnesses");
    let opencode = report
        .harnesses
        .iter()
        .find(|h| h.id == AgentId::from(AgentId::OPEN_CODE))
        .expect("OpenCode row in the harness report");
    assert!(
        opencode.configured,
        "`.config/opencode/opencode.json` exists, which is the documented Configured signal"
    );
    assert_eq!(
        opencode.state,
        HarnessState::Configured,
        "`opencode` on PATH plus its config file is Configured, and no session store exists"
    );
}

/// A plugin's skills live at `skills/<name>/SKILL.md` inside the plugin
/// folder (`docs/action-map/harnesses/plugins.md`). A `node_modules/`
/// subtree holds the plugin's dependencies; a `SKILL.md` inside one belongs
/// to that package, and no documented reader loads it.
#[test]
fn scan_never_descends_into_node_modules_inside_a_plugin_cache_or_names_the_path() {
    let inventory = scan_shape(shapes::with_plugin_cache_nesting(FixtureBuilder::new()));

    assert_eq!(
        rows_named(&inventory, shapes::VENDORED_SKILL_NAME),
        0,
        "`{}` sits under a plugin's node_modules, not under its skills/ folder",
        shapes::VENDORED_SKILL_NAME
    );
    let vendored: Vec<String> = deployment_paths(&inventory)
        .into_iter()
        .filter(|path| path.contains("node_modules"))
        .collect();
    assert!(
        vendored.is_empty(),
        "the plugin cache walk must stop at the plugin root and never enter a dependency \
         tree: {vendored:?}"
    );
}

/// A cached plugin's enabled state is keyed `<plugin>@<marketplace>` with
/// no version in the key (`docs/research/harness-primitives.md`), and an
/// orphaned version is pruned about 14 days after it stops being used
/// (`docs/action-map/harnesses/plugins.md`). Two version folders on disk
/// are therefore one live plugin, so one skill row with one deployment -
/// not the same skill counted once per stale copy.
#[test]
#[ignore = "follow-up: enumerate_plugin_skills reports every cached version folder, and \
            no doc names which one is live (plugins.md leaves the Codex cache layout open \
            and gives Claude only the ~14-day orphan prune), so the dedupe needs a \
            liveness source rather than a version-string compare (issue-2.7-followup-a.md)"]
fn scan_picks_one_version_per_cached_plugin_or_names_the_duplicate() {
    let inventory = scan_shape(shapes::with_plugin_cache_nesting(FixtureBuilder::new()));

    let claude_cache = format!("{HOME}/.claude/plugins/cache/vendor-1/plugin-1");
    let from_claude_cache: Vec<String> = inventory
        .skills
        .iter()
        .filter(|skill| skill.name.0 == shapes::PLUGIN_SKILL_NAME)
        .flat_map(|skill| &skill.deployments)
        .map(|deployment| deployment.path.display().to_string())
        .filter(|path| path.starts_with(&claude_cache))
        .collect();
    assert_eq!(
        from_claude_cache.len(),
        1,
        "versions {:?} of plugin-1 are cached side by side but only one is live, so \
         `{}` must be reported once: {from_claude_cache:?}",
        shapes::PLUGIN_VERSIONS,
        shapes::PLUGIN_SKILL_NAME
    );
}

/// The lock file belongs to `npx skills`, which writes keys this reader
/// does not model (`dismissed`, `lastSelectedAgents`) and agent ids the app
/// has no harness for. Reading it must keep every documented field
/// (`docs/action-map/harnesses/shared-root.md`) and must not drop the keys
/// it does not model, for the reason the sibling registry keeps a flatten
/// catch-all: an older build must never drop a newer build's keys.
#[test]
#[ignore = "follow-up: InstalledSkillEntry has no flatten catch-all, so `dismissed` and \
            `lastSelectedAgents` are dropped on parse; adding one changes a struct the \
            desktop crate builds by literal (issue-2.7-followup-a.md)"]
fn lockfile_v3_with_unknown_agents_parses_and_keeps_unknown_fields_or_names_the_field() {
    let builder = shapes::with_lock_file_v3_unknown_agents(FixtureBuilder::new());
    let fs = builder.rooted_at(HOME).dir(HOME).build_fs();
    let lock = read_lock_file(
        &fs,
        &skill_studio_core::lock_file::lock_file_path(Path::new(HOME)),
    )
    .expect("a version-3 lock file with unmodelled keys must still parse");

    assert_eq!(lock.version, 3);
    let entry: &InstalledSkillEntry = lock
        .skills
        .get("skill-a")
        .expect("skill-a must survive the parse");
    assert_eq!(
        entry.skill_path.as_deref(),
        Some("skills/skill-a"),
        "`skillPath` is a documented optional field and must round-trip"
    );

    let round_tripped = serde_json::to_value(entry).expect("serialize the entry back");
    for field in ["dismissed", "lastSelectedAgents"] {
        assert!(
            round_tripped.get(field).is_some(),
            "`{field}` was written by the tool that owns this file and must survive a \
             read/write round trip; got {round_tripped}"
        );
    }
}

/// No harness's documented discovery path names a project's root-level
/// `skills/` folder, so nothing may be reported from it.
/// `.cursor/skills/` is Cursor's own project root
/// (`docs/agent-skill-conventions.md`, Discovery paths), so its skills are
/// reported under Cursor's bucket rather than ignored.
#[test]
fn scan_ignores_project_root_skills_dir_and_cursor_root_or_names_the_row() {
    let project = PathBuf::from(HOME).join("src/project-1");
    let builder = shapes::with_project_non_standard_roots(FixtureBuilder::new(), "src/project-1");
    let rt = in_memory_runtime(builder, vec![project.clone()], &[]);
    let inventory = ops::scan(&rt, &ctx(), &ScanRequest::default()).expect("scan");

    assert_eq!(
        rows_named(&inventory, shapes::PROJECT_ROOT_SKILL_NAME),
        0,
        "`{}/skills` is not a documented root for any harness; rows: {:?}",
        project.display(),
        inventory
            .skills
            .iter()
            .map(|s| s.name.0.clone())
            .collect::<Vec<_>>()
    );
    let cursor_rows: Vec<&RootKind> = inventory
        .skills
        .iter()
        .filter(|skill| skill.name.0 == shapes::CURSOR_SKILL_NAME)
        .flat_map(|skill| &skill.deployments)
        .map(|deployment| &deployment.root.kind)
        .collect();
    assert_eq!(
        cursor_rows,
        vec![&RootKind::Harness(AgentId::from(AgentId::CURSOR))],
        "`.cursor/skills` is Cursor's documented project root, so `{}` belongs to Cursor",
        shapes::CURSOR_SKILL_NAME
    );
    for (name, kind) in [
        ("project-shared-skill", RootKind::Universal),
        (
            "project-claude-skill",
            RootKind::Harness(AgentId::from(AgentId::CLAUDE_CODE)),
        ),
        (
            "project-codex-skill",
            RootKind::Harness(AgentId::from(AgentId::CODEX)),
        ),
    ] {
        let found = inventory
            .skills
            .iter()
            .find(|skill| skill.name.0 == name)
            .unwrap_or_else(|| panic!("`{name}` must be reported from its standard project root"));
        let expected_root = RootRef::new(
            RootScope::Project(ProjectRef(project.clone())),
            kind.clone(),
        )
        .expect("a project root");
        assert!(
            found.deployments.iter().any(|d| d.root == expected_root),
            "`{name}` must be filed under {kind:?} in {}; got {:?}",
            project.display(),
            found
                .deployments
                .iter()
                .map(|d| &d.root)
                .collect::<Vec<_>>()
        );
    }
}

/// pi has no native per-skill switch, so this build keeps its own
/// exclusion list under a `skill-studio` key in pi's `settings.json`
/// (`docs/agent-skill-conventions.md`: "pi, Cursor, Grok Build: no
/// per-skill disable"). Writing that key back must leave every other key in
/// the file untouched - the file belongs to pi, not to Skill Studio.
#[test]
fn harness_toggle_preserves_unknown_settings_keys_or_names_the_lost_key() {
    let home = unique_temp_dir("real_home_shapes_pi_settings");
    std::fs::create_dir_all(&home).unwrap();
    let home = home.canonicalize().unwrap();

    let skill_dir = home.join(".agents/skills/toggle-me");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: toggle-me\ndescription: A skill whose pi switch is flipped in this test.\n---\nBody.\n",
    )
    .unwrap();
    let settings_path = home.join(".pi/agent/settings.json");
    std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    let before = r#"{"theme":"dark","telemetry":false,"editor":{"tabWidth":2}}"#;
    std::fs::write(&settings_path, before).unwrap();

    let scope = RuntimeScope::fixture(&home);
    let ports = Ports {
        fs: Arc::new(RealFs::new()),
        clock: Arc::new(FakeClock::at(0)),
        ids: Arc::new(FakeIds::default()),
        leases: Arc::new(FileLease::new(home.join(".leases"))),
        history: Arc::new(SqliteHistoryOpener::new(
            home.join(".history/events.sqlite3"),
        )),
        sink: Arc::new(RecordingSink::default()),
        spawner: None,
        discovery: None,
        tools: None,
        catalog: Arc::new(HarnessCatalog::builtin()),
    };
    let rt = Runtime::new(&scope, ports).expect("runtime");

    ops::set_harness_enabled(
        &rt,
        &ctx(),
        &SetHarnessEnabledRequest {
            skill: SkillName("toggle-me".into()),
            harness: AgentId::from(AgentId::PI),
            enabled: false,
            project_path: None,
        },
    )
    .expect("pi switch");

    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    for (key, value) in [
        ("theme", serde_json::json!("dark")),
        ("telemetry", serde_json::json!(false)),
        ("editor", serde_json::json!({"tabWidth": 2})),
    ] {
        assert_eq!(
            after.get(key),
            Some(&value),
            "the pi switch rewrote {} and lost `{key}`; got {after}",
            settings_path.display()
        );
    }
    assert_eq!(
        after["skill-studio"]["disabledSkills"][0], "toggle-me",
        "the switch itself must still be written; got {after}"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// The composed home: `scan` and `diagnose` must both complete on it, and
/// each shape's rule must hold in company - the synced buckets contribute
/// their deployed skills once, Codex's bundled `.system` skills contribute
/// nothing, and each pi link collapses onto its shared deployment.
#[test]
fn scan_and_diagnose_complete_on_the_largest_real_shape_home_or_names_the_panic() {
    let rt = in_memory_runtime(
        shapes::largest_real_shape_home(),
        vec![PathBuf::from(HOME).join("src/project-1")],
        &[],
    );

    let diagnosis = ops::diagnose(&rt, &ctx(), &ScanRequest::default()).expect("diagnose");
    let inventory = &diagnosis.inventory;
    assert_eq!(
        inventory.completeness,
        skill_studio_core::dto::Completeness::Complete,
        "every root in the composed home is readable; observations: {:?}",
        inventory.observations
    );

    for name in shapes::SYNCED_BUCKET_SKILLS {
        assert_eq!(
            rows_named(inventory, name),
            1,
            "shape 1's `{name}` must appear exactly once in the composed home"
        );
    }
    for name in shapes::CODEX_SYSTEM_SKILLS {
        assert_eq!(
            rows_named(inventory, name),
            0,
            "shape 2's bundled `{name}` is hidden from every documented reader"
        );
    }
    for name in ["linked-skill-1", "linked-skill-2"] {
        assert_eq!(
            rows_named(inventory, name),
            1,
            "shape 3's `{name}` is one skill reached through two roots"
        );
        let skill = inventory
            .skills
            .iter()
            .find(|skill| skill.name.0 == name)
            .expect("the linked skill");
        assert_eq!(
            skill.deployments.len(),
            2,
            "`{name}` is deployed in the shared root and linked from pi's"
        );
    }
    assert_eq!(
        inventory.skills.len(),
        expected_row_count(),
        "the composed home's row count must be the sum of the shapes that contribute rows"
    );
}

/// Rows the composed home is expected to carry: the two synced-bucket
/// skills, the two pi-linked skills, one plugin skill per cache, the
/// project's four reported roots, and the scale shape's own skills.
fn expected_row_count() -> usize {
    let synced = shapes::SYNCED_BUCKET_SKILLS.len();
    let pi_linked = 2;
    let plugin = 1;
    let project = 4;
    let parked_and_scale = shapes::LARGEST_HOME_SKILLS_PER_ROOT * 2 + 1;
    synced + pi_linked + plugin + project + parked_and_scale
}
