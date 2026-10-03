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
use skill_studio_core::dto::{Diagnosis, HarnessesRequest, Inventory, ScanRequest};
use skill_studio_core::harness::{HarnessCatalog, HarnessState};
use skill_studio_core::identity::{AgentId, ProjectRef, RootKind, RootRef, RootScope};
use skill_studio_core::lock_file::{read_lock_file, InstalledSkillEntry};
use skill_studio_core::ops;
use skill_studio_core::ports::{Ports, Runtime, ScopeFs};
use skill_studio_core::scope::{ProjectSelection, RuntimeScope};
use skill_studio_core::testing::golden::ctx;
use skill_studio_core::testing::{
    FakeClock, FakeIds, FakeLease, FakeToolLookup, FixtureBuilder, NoHistory, RecordingSink,
};
use skill_studio_core::testing_shapes as shapes;

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

        telemetry: std::sync::Arc::new(skill_studio_core::ports::NoopTelemetry),
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

/// `synced` is reserved by the vendor inside its skills root
/// (`docs/action-map/harnesses/claude-code.md`: "`synced` under
/// `~/.claude/skills` is reserved; the scanner must skip it"), so it is
/// skipped by name: the fixture plants a `SKILL.md` at the reserved folder
/// itself, which makes it a skill folder by the spec's shape test
/// (<https://agentskills.io/specification>) and leaves the reserved-name
/// rule as the only thing that can keep it out. The skills the bucket feeds
/// into the roots are ordinary rows, one each, even though the bucket holds
/// a second copy of their bytes.
#[test]
fn scan_reports_synced_bucket_skills_once_and_not_the_bucket_root_or_names_the_extra_row() {
    let inventory = scan_shape(shapes::with_synced_bucket(FixtureBuilder::new()));

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
        "`{}` is the vendor's own folder, so it must be skipped by name even with a \
         SKILL.md in it",
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

/// A dot-prefixed entry is hidden from every documented reader
/// (`docs/agent-skill-conventions.md`, Discovery paths), so a skills root
/// contributes nothing from one - not Codex's bundled `.system` tree
/// (`docs/action-map/harnesses/codex.md` lists those as the harness's own,
/// separate from the roots a user deploys into), and not a dot-prefixed
/// folder that holds a valid `SKILL.md` of its own.
#[test]
fn scan_skips_dot_prefixed_entries_in_a_skills_root_or_names_the_row() {
    let inventory = scan_shape(shapes::with_codex_system_skills(FixtureBuilder::new()));

    assert_eq!(
        rows_named(&inventory, shapes::CODEX_HIDDEN_SKILL_NAME),
        0,
        "`{}/{}` is a skill folder by shape, and only its leading dot keeps it out; rows: {:?}",
        shapes::CODEX_ROOT_RELATIVE,
        shapes::CODEX_HIDDEN_SKILL_DIR_NAME,
        inventory
            .skills
            .iter()
            .map(|s| s.name.0.clone())
            .collect::<Vec<_>>()
    );
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
        .filter(|path| {
            path.contains(shapes::CODEX_SYSTEM_DIR_NAME)
                || path.contains(shapes::CODEX_HIDDEN_SKILL_DIR_NAME)
        })
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
/// default (`docs/action-map/harnesses/shared-root.md`). A link whose
/// target is missing is the violation the invariant exists for, so the same
/// home carries one of each and the check must name only the dangling one.
#[test]
fn doctor_link_check_resolves_relative_links_inside_the_root_or_names_the_false_violation() {
    let dangling = format!("{}/dangling-skill", shapes::PI_ROOT_RELATIVE);
    let builder = shapes::with_pi_links_to_shared(
        FixtureBuilder::new(),
        &["linked-skill-1", "linked-skill-2"],
    )
    .alias(&dangling, "../../../.agents/skills/does-not-exist");
    let diagnosis = diagnose_shape(builder);

    let named: Vec<String> = check_link_resolves_in_root(&diagnosis)
        .iter()
        .map(|v| v.path.display().to_string())
        .collect();
    assert_eq!(
        named,
        vec![PathBuf::from(HOME).join(&dangling).display().to_string()],
        "only the link with no target may be a violation; the two links that resolve into \
         the shared root are the shape `npx skills` writes"
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

/// The Claude Code plugin rows `inventory` reports for
/// [`shapes::PLUGIN_SKILL_NAME`], as deployment paths.
fn claude_cached_plugin_paths(inventory: &Inventory) -> Vec<String> {
    let claude_cache = format!("{HOME}/.claude/plugins/cache/vendor-1/plugin-1");
    deployment_paths(inventory)
        .into_iter()
        .filter(|path| path.starts_with(&claude_cache))
        .filter(|path| path.ends_with(shapes::PLUGIN_SKILL_NAME))
        .collect()
}

/// Claude Code keeps an updated plugin's old version folder in the cache
/// until its ~14-day orphan prune (`docs/action-map/harnesses/plugins.md`),
/// and its enabled state is keyed `<plugin>@<marketplace>` with no version
/// (`docs/research/harness-primitives.md`). `installed_plugins.json`
/// (version 2) names the `installPath` Claude Code loads, so one skill row
/// comes from that folder only - whichever version string it carries, so
/// the lower version is also tried as the live one.
#[test]
fn scan_picks_one_version_per_cached_plugin_or_names_the_duplicate() {
    for live in shapes::PLUGIN_VERSIONS {
        let inventory = scan_shape(shapes::with_claude_installed_plugin(
            shapes::with_plugin_cache_nesting(FixtureBuilder::new()),
            HOME,
            live,
        ));

        let from_claude_cache = claude_cached_plugin_paths(&inventory);
        assert_eq!(
            from_claude_cache.len(),
            1,
            "versions {:?} of plugin-1 are cached side by side but installed_plugins.json \
             names only {live}, so `{}` must be reported once: {from_claude_cache:?}",
            shapes::PLUGIN_VERSIONS,
            shapes::PLUGIN_SKILL_NAME
        );
        assert!(
            from_claude_cache[0].contains(&format!("/plugin-1/{live}/")),
            "the row must come from the installPath folder {live}, got {from_claude_cache:?}"
        );
    }
}

/// Without a usable `installed_plugins.json`, or when its `installPath` is
/// not a cached folder, nothing says which version folder is live. Every
/// cached version then stays, so the plugin is never hidden.
#[test]
fn scan_without_a_matching_installed_plugins_record_keeps_every_cached_version_or_names_the_hidden_plugin(
) {
    let cases = [
        ("no installed_plugins.json", FixtureBuilder::new()),
        (
            "installPath names a version that is not cached",
            shapes::with_claude_installed_plugin(FixtureBuilder::new(), HOME, "9.9.9"),
        ),
        (
            "installed_plugins.json is version 1",
            FixtureBuilder::new().file(
                ".claude/plugins/installed_plugins.json",
                br#"{"version":1,"plugins":{}}"#,
            ),
        ),
    ];
    for (case, builder) in cases {
        let inventory = scan_shape(shapes::with_plugin_cache_nesting(builder));
        let from_claude_cache = claude_cached_plugin_paths(&inventory);
        assert_eq!(
            from_claude_cache.len(),
            shapes::PLUGIN_VERSIONS.len(),
            "{case}: no record names a cached folder, so every cached version must stay \
             listed: {from_claude_cache:?}"
        );
    }
}

/// The lock file belongs to `npx skills`, which writes keys this reader
/// does not model (`dismissed`, `lastSelectedAgents`) and agent ids the app
/// has no harness for. Reading it must keep every documented field
/// (`docs/action-map/harnesses/shared-root.md`) and must not drop the keys
/// it does not model, for the reason the sibling registry keeps a flatten
/// catch-all: an older build must never drop a newer build's keys.
#[test]
fn lockfile_v3_with_unknown_agents_parses_and_keeps_unknown_fields_or_names_the_field() {
    let builder = FixtureBuilder::new().file(
        shapes::LOCK_FILE_RELATIVE,
        shapes::LOCK_FILE_V3_UNKNOWN_AGENTS,
    );
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
